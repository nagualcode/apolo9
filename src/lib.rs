//! Apolo9 — a circuit-level model of the SOMA Lyra-8 "organismic"
//! synthesizer, built on the truce framework.
//!
//! The DSP lives in [`dsp`] and works in volts at an oversampled rate; this
//! file only maps parameters, MIDI and the editor's touch pads onto it.

pub mod dsp;
pub mod params;
pub mod ui;

use dsp::engine::{Controls, Engine, ModSrc};
use params::{Apolo9Params, Apolo9ParamsParamId as P, DelayModMode, DelayWave, KeyMap, LfoLogic, ModSource, load_f32, store_f32};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use truce::prelude::*;
use truce_core::bus::{BusLayout, ChannelConfig};
use truce_egui::EguiEditor;

const MAX_CHUNK: usize = 1024;

pub struct Apolo9State {
    engine: Option<Engine>,
    mono: Vec<f32>,
    ext: Vec<f32>,
    /// Pressure from MIDI velocity per sensor.
    midi_vel: [f64; 8],
    /// Poly aftertouch per sensor (0..1).
    midi_at: [f64; 8],
    chan_pressure: f64,
    scope: Scope,
    last_panic: u32,
    panic_param: bool,
}

/// Peak-preserving decimation of the output into the editor's scope ring.
struct Scope {
    div: usize,
    acc: f32,
    n: usize,
}

impl Scope {
    fn push(&mut self, ui: &params::UiShared, out: &[f32]) {
        let mut pos = ui.scope_pos.load(Ordering::Relaxed) as usize;
        for &s in out {
            if s.abs() > self.acc.abs() {
                self.acc = s;
            }
            self.n += 1;
            if self.n >= self.div {
                store_f32(&ui.scope[pos % params::SCOPE_LEN], self.acc);
                pos = (pos + 1) % params::SCOPE_LEN;
                self.n = 0;
                self.acc = 0.0;
            }
        }
        ui.scope_pos.store(pos as u32, Ordering::Relaxed);
    }
}

impl Default for Apolo9State {
    fn default() -> Self {
        Self {
            engine: None,
            mono: vec![0.0; MAX_CHUNK],
            ext: vec![0.0; MAX_CHUNK],
            midi_vel: [0.0; 8],
            midi_at: [0.0; 8],
            chan_pressure: 0.0,
            scope: Scope { div: 4, acc: 0.0, n: 0 },
            last_panic: 0,
            panic_param: false,
        }
    }
}

fn mod_src(s: ModSource) -> ModSrc {
    match s {
        ModSource::Off => ModSrc::Off,
        ModSource::Voices => ModSrc::Voices,
        ModSource::Lfo => ModSrc::Lfo,
    }
}

/// Panel state from the parameters.
pub fn controls_from(p: &Apolo9Params) -> Controls {
    let f = |x: &FloatParam| f64::from(x.value());
    Controls {
        tune: [
            f(&p.tune1),
            f(&p.tune2),
            f(&p.tune3),
            f(&p.tune4),
            f(&p.tune5),
            f(&p.tune6),
            f(&p.tune7),
            f(&p.tune8),
        ],
        sharp: [f(&p.sharp12), f(&p.sharp34), f(&p.sharp56), f(&p.sharp78)],
        fm_mod: [f(&p.mod12), f(&p.mod34), f(&p.mod56), f(&p.mod78)],
        fm_src: [
            mod_src(p.src12.value()),
            mod_src(p.src34.value()),
            mod_src(p.src56.value()),
            mod_src(p.src78.value()),
        ],
        fast: [p.fast12.value(), p.fast34.value(), p.fast56.value(), p.fast78.value()],
        pitch: [f(&p.pitch_a), f(&p.pitch_b)],
        hold: [f(&p.hold_a), f(&p.hold_b)],
        vibrato: p.vibrato.value(),
        total_fb: p.total_fb.value(),
        fm_loop: p.fm_loop.value(),
        lfo_freq: [f(&p.lfo_a), f(&p.lfo_b)],
        lfo_and: p.lfo_logic.value() == LfoLogic::And,
        link: p.lfo_link.value(),
        time: [f(&p.time1), f(&p.time2)],
        dly_mod: [f(&p.dmod1), f(&p.dmod2)],
        dly_self: p.dly_mode.value() == DelayModMode::SelfMod,
        dly_square: p.dly_wave.value() == DelayWave::Square,
        fb: f(&p.feedback),
        dly_mix: f(&p.dly_mix),
        drive: f(&p.drive),
        dist_mix: f(&p.dist_mix),
        vol: f(&p.volume),
        hold_gate: p.hold_gate_on.value().then(|| f(&p.hold_gate)),
        cv_voices: p.cv_voices_on.value().then(|| f(&p.cv_voices)),
        cv_delay: p.cv_delay_on.value().then(|| f(&p.cv_delay)),
        humidity: f(&p.humidity),
        crosstalk: f(&p.crosstalk),
        noise: f(&p.noise),
        ext_gain: f(&p.ext_gain),
    }
}

/// MIDI note → sensor index.
pub fn sensor_for(note: u8, map: KeyMap, base: i64) -> Option<usize> {
    let rel = i64::from(note) - base;
    match map {
        KeyMap::Chromatic => (0..8).contains(&rel).then_some(rel as usize),
        KeyMap::White => [0, 2, 4, 5, 7, 9, 11, 12].iter().position(|&o| o == rel),
    }
}

/// Velocity → finger pressure (soft strokes stay light).
fn velocity_pressure(v: u8) -> f64 {
    0.22 + 0.78 * (f64::from(v) / 127.0).powf(0.8)
}

pub struct Apolo9;

impl Apolo9 {
    fn handle_event(state: &mut Apolo9State, params: &Apolo9Params, body: &EventBody) {
        let map = params.key_map.value();
        let base = params.base_note.value();
        match *body {
            EventBody::NoteOn { note, velocity, .. } => {
                if let Some(s) = sensor_for(note, map, base) {
                    state.midi_vel[s] = velocity_pressure(velocity);
                    state.midi_at[s] = 0.0;
                }
            }
            EventBody::NoteOff { note, .. } => {
                if let Some(s) = sensor_for(note, map, base) {
                    state.midi_vel[s] = 0.0;
                    state.midi_at[s] = 0.0;
                }
            }
            EventBody::Aftertouch { note, pressure, .. } => {
                if let Some(s) = sensor_for(note, map, base) {
                    state.midi_at[s] = f64::from(pressure) / 127.0;
                }
            }
            EventBody::ChannelPressure { pressure, .. } => {
                state.chan_pressure = f64::from(pressure) / 127.0;
            }
            // All Notes Off lifts the fingers; All Sound Off is a panic.
            EventBody::ControlChange { cc: 123, .. } => {
                state.midi_vel = [0.0; 8];
                state.midi_at = [0.0; 8];
            }
            EventBody::ControlChange { cc: 120, .. } => Self::panic(state, params),
            _ => {}
        }
    }

    /// Everything to silence: fingers, envelopes, delay memory, loops.
    fn panic(state: &mut Apolo9State, params: &Apolo9Params) {
        state.midi_vel = [0.0; 8];
        state.midi_at = [0.0; 8];
        state.chan_pressure = 0.0;
        for pad in &params.ui.pad {
            store_f32(pad, 0.0);
        }
        if let Some(e) = state.engine.as_mut() {
            e.panic();
        }
    }

    fn check_panic(state: &mut Apolo9State, params: &Apolo9Params) {
        let n = params.ui.panic.load(Ordering::Relaxed);
        let p = params.panic.value();
        let rising = p && !state.panic_param;
        state.panic_param = p;
        if n != state.last_panic || rising {
            state.last_panic = n;
            Self::panic(state, params);
        }
    }

    fn apply_touch(state: &mut Apolo9State, params: &Apolo9Params) {
        let Some(engine) = state.engine.as_mut() else { return };
        for s in 0..8 {
            let gui = f64::from(load_f32(&params.ui.pad[s]));
            let at = state.midi_at[s].max(state.chan_pressure);
            let midi = if state.midi_vel[s] > 0.0 {
                (state.midi_vel[s] * (0.85 + 0.45 * at)).min(1.0)
            } else {
                0.0
            };
            let t = gui.max(midi);
            engine.touch[s] = t;
            store_f32(&params.ui.touch[s], t as f32);
        }
    }

    fn publish(engine: &Engine, ui: &params::UiShared) {
        let t = &engine.telemetry;
        for i in 0..8 {
            store_f32(&ui.env[i], t.env[i]);
            store_f32(&ui.vib[i], t.vib[i]);
        }
        for p in 0..4 {
            store_f32(&ui.pair[p], t.pair_level[p]);
        }
        ui.lfo_a.store(t.lfo_a, Ordering::Relaxed);
        ui.lfo_b.store(t.lfo_b, Ordering::Relaxed);
        store_f32(&ui.delay_ms[0], t.delay_ms[0]);
        store_f32(&ui.delay_ms[1], t.delay_ms[1]);
    }
}

impl PluginLogic for Apolo9 {
    type Params = Apolo9Params;
    type DspState = Apolo9State;

    fn bus_layouts() -> Vec<BusLayout> {
        // Instruments must not declare inputs: AU v2 has no sidechain notion
        // and would report the EXT IN bus as a main input (auval -10877).
        // The engine still accepts EXT IN audio (see `Engine::process`).
        vec![
            BusLayout::new().with_output("Main", ChannelConfig::Stereo),
            BusLayout::new().with_output("Main", ChannelConfig::Mono),
        ]
    }

    fn reset(state: &mut Self::DspState, params: &Self::Params, config: &AudioConfig) {
        let serial = params.serial.value().clamp(1, 9999) as u32;
        let rebuild = state
            .engine
            .as_ref()
            .is_none_or(|e| (e.host_rate() - config.sample_rate).abs() > 0.5);
        if rebuild {
            state.engine = Some(Engine::new(config.sample_rate, serial));
        }
        state.last_panic = params.ui.panic.load(Ordering::Relaxed);
        state.scope.div = ((config.sample_rate / 12_000.0).round() as usize).max(1);
        if state.mono.len() < MAX_CHUNK {
            state.mono.resize(MAX_CHUNK, 0.0);
            state.ext.resize(MAX_CHUNK, 0.0);
        }
    }

    fn latency(state: &Self::DspState) -> u32 {
        state.engine.as_ref().map_or(0, |e| e.latency() as u32)
    }

    fn process(
        state: &mut Self::DspState,
        params: &Self::Params,
        buffer: &mut AudioBuffer,
        events: &EventList,
        context: &mut ProcessContext,
    ) -> ProcessStatus {
        let n = buffer.num_samples();
        if n == 0 || state.engine.is_none() {
            return ProcessStatus::Normal;
        }
        if let Some(e) = state.engine.as_mut() {
            e.set_serial(params.serial.value().clamp(1, 9999) as u32);
        }
        Self::check_panic(state, params);
        let controls = controls_from(params);
        let n_in = buffer.num_input_channels();
        let n_out = buffer.num_output_channels();

        let mut ev = 0;
        let mut done = 0;
        let mut peak = 0.0f32;
        while done < n {
            // Apply every event at or before `done`, then run to the next.
            while let Some(e) = events.get(ev) {
                if e.sample_offset as usize > done {
                    break;
                }
                Self::handle_event(state, params, &e.body);
                ev += 1;
            }
            let next = events.get(ev).map_or(n, |e| (e.sample_offset as usize).min(n));
            let end = next.max(done + 1).min(done + MAX_CHUNK).min(n);
            let len = end - done;
            Self::apply_touch(state, params);

            let has_ext = n_in > 0;
            if has_ext {
                let ext = &mut state.ext[..len];
                ext.fill(0.0);
                for ch in 0..n_in {
                    let src = &buffer.input(ch)[done..end];
                    for (d, s) in ext.iter_mut().zip(src) {
                        *d += *s / n_in as f32;
                    }
                }
            }
            let engine = state.engine.as_mut().expect("engine");
            let ext = if has_ext { Some(&state.ext[..len]) } else { None };
            engine.process(&controls, ext, &mut state.mono[..len]);
            for ch in 0..n_out {
                buffer.output(ch)[done..end].copy_from_slice(&state.mono[..len]);
            }
            for &s in &state.mono[..len] {
                peak = peak.max(s.abs());
            }
            Self::publish(engine, &params.ui);
            state.scope.push(&params.ui, &state.mono[..len]);
            done = end;
        }
        context.set_meter(P::MeterOut, peak.min(1.0));
        ProcessStatus::Normal
    }

    fn editor(params: Arc<Self::Params>) -> Box<dyn Editor> {
        EguiEditor::new(params, (ui::WINDOW_W, ui::WINDOW_H), ui::apolo_ui)
            .with_visuals(truce_egui::theme::dark())
            .resizable(true)
            .min_size((ui::MIN_W, ui::MIN_H))
            .max_size((ui::MAX_W, ui::MAX_H))
            .aspect_ratio(Some((ui::WINDOW_W, ui::WINDOW_H)))
            .into_editor()
    }
}

truce::plugin! {
    logic: Apolo9,
    params: Apolo9Params,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use truce_test::{InputSource, assert_no_audio_alloc, assertions, driver};

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn info_and_buses_are_valid() {
        truce_test::assert_valid_info::<Plugin>();
        truce_test::assert_bus_config_instrument::<Plugin>();
        truce_test::assert_no_duplicate_param_ids::<Plugin>();
        truce_test::assert_param_defaults_match::<Plugin>();
        truce_test::assert_au_type_codes_ascii::<Plugin>();
        truce_test::assert_fourcc_roundtrip::<Plugin>();
    }

    #[test]
    fn state_round_trips_and_survives_garbage() {
        truce_test::assert_state_round_trip::<Plugin>();
        truce_test::assert_corrupt_state_no_crash::<Plugin>();
        truce_test::assert_empty_state_no_crash::<Plugin>();
    }

    #[test]
    fn untouched_is_only_noise_floor() {
        let r = driver!(Plugin)
            .sample_rate(48_000.0)
            .channels(2)
            .block_size(256)
            .duration(Duration::from_millis(1500))
            .input(InputSource::Silence)
            .run();
        assertions::assert_no_nans(&r);
        let tail = &r.output[0][48_000..];
        let db = 20.0 * rms(tail).max(1e-9).log10();
        assert!(db < -50.0, "idle floor too loud: {db:.1} dBFS");
    }

    #[test]
    fn a_touch_sounds_and_decays() {
        let r = driver!(Plugin)
            .sample_rate(44_100.0)
            .channels(2)
            .block_size(128)
            .duration(Duration::from_millis(2500))
            .script(|s| {
                s.wait_ms(100);
                s.note_on(64, 1.0); // white-key map: E3 → sensor 3
                s.wait_ms(600);
                s.note_off(64);
            })
            .run();
        assertions::assert_no_nans(&r);
        let sr = 44_100;
        let on = 20.0 * rms(&r.output[0][sr / 2..sr * 7 / 10]).max(1e-9).log10();
        let before = 20.0 * rms(&r.output[0][..sr / 20]).max(1e-9).log10();
        assert!(on > -40.0, "touched voice too quiet: {on:.1} dBFS");
        assert!(on - before > 25.0, "touch barely changes the output: {before:.1} -> {on:.1}");
        // Both channels carry the same mono instrument.
        assert_eq!(r.output[0][sr], r.output[1][sr]);
    }

    #[test]
    fn panic_silences_everything_at_once() {
        // Self-oscillating delay, TOTAL FB, every sensor held, then panic.
        let r = driver!(Plugin)
            .sample_rate(48_000.0)
            .block_size(64)
            .duration(Duration::from_millis(2600))
            .script(|s| {
                s.set_param(P::Feedback, 0.8);
                s.set_param(P::DlyMix, 0.8);
                s.set_param(P::TotalFb, 1.0);
                s.set_param(P::Src12, 1.0);
                for n in [60, 62, 64, 65, 67, 69, 71, 72] {
                    s.note_on(n, 1.0);
                }
                s.wait_ms(2000);
                s.set_param(P::Panic, 1.0);
                s.wait_ms(20);
                s.set_param(P::Panic, 0.0);
                s.wait_ms(560);
            })
            .run();
        assertions::assert_no_nans(&r);
        let sr = 48_000;
        let before = rms(&r.output[0][sr * 18 / 10..sr * 2]);
        let after = r.output[0][sr * 2 + sr / 100..sr * 2 + sr * 14 / 100].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let later = rms(&r.output[0][sr * 24 / 10..]);
        assert!(before > 0.01, "nothing was sounding before the panic");
        assert_eq!(after, 0.0, "output not silent right after panic");
        // The sensors stay lifted (the notes were cut), so it stays quiet.
        assert!(later < before * 0.05, "sound came back: {before} -> {later}");
    }

    #[test]
    fn sensor_map() {
        assert_eq!(sensor_for(60, KeyMap::White, 60), Some(0));
        assert_eq!(sensor_for(72, KeyMap::White, 60), Some(7));
        assert_eq!(sensor_for(61, KeyMap::White, 60), None);
        assert_eq!(sensor_for(67, KeyMap::Chromatic, 60), Some(7));
        assert_eq!(sensor_for(68, KeyMap::Chromatic, 60), None);
    }

    #[test]
    fn process_is_allocation_free() {
        assert_no_audio_alloc(|| {
            driver!(Plugin)
                .duration(Duration::from_millis(80))
                .script(|s| {
                    s.note_on(60, 0.9);
                    s.note_on(67, 0.5);
                    s.set_param(P::Src12, 0.5);
                    s.set_param(P::Serial, 0.3);
                    s.wait_ms(30);
                    s.set_param(P::TotalFb, 1.0);
                    s.set_param(P::Feedback, 0.9);
                    s.wait_ms(30);
                    s.note_off(60);
                })
                .run()
        });
    }

    #[test]
    fn extreme_settings_stay_finite_and_bounded() {
        let r = driver!(Plugin)
            .sample_rate(48_000.0)
            .duration(Duration::from_millis(3000))
            .script(|s| {
                for p in [P::Src12, P::Src34, P::Src56, P::Src78] {
                    s.set_param(p, 1.0);
                }
                for p in [P::Mod12, P::Mod34, P::Mod56, P::Mod78, P::Feedback, P::Drive, P::DistMix, P::Volume] {
                    s.set_param(p, 1.0);
                }
                s.set_param(P::TotalFb, 1.0);
                s.set_param(P::HoldA, 1.0);
                s.set_param(P::HoldB, 1.0);
                s.set_param(P::Dmod1, 1.0);
                s.set_param(P::DlyMode, 1.0);
                s.set_param(P::Humidity, 1.0);
                for n in 60..73 {
                    s.note_on(n, 1.0);
                }
                s.wait_ms(2500);
            })
            .run();
        assertions::assert_no_nans(&r);
        assertions::assert_peak_below(&r, 0.95);
    }

    /// Renders the editor for the README.
    /// `cargo test --release --lib screenshots -- --ignored`
    #[test]
    #[ignore]
    fn screenshots() {
        let params = Arc::new(Apolo9Params::new());
        params.src12.set_value(ModSource::Voices);
        params.src56.set_value(ModSource::Lfo);
        params.fast34.set_value(true);
        params.vibrato.set_value(true);
        params.dmod1.set_value(0.4);
        params.hold_a.set_value(0.3);
        let ui = &params.ui;
        for (i, e) in [0.9f32, 0.2, 0.0, 0.65, 1.0, 0.0, 0.35, 0.05].iter().enumerate() {
            store_f32(&ui.env[i], *e);
        }
        store_f32(&ui.touch[4], 0.8);
        for p in 0..4 {
            store_f32(&ui.pair[p], [0.3, 0.1, 0.35, 0.05][p]);
        }
        store_f32(&ui.delay_ms[0], 214.0);
        store_f32(&ui.delay_ms[1], 377.0);
        ui.lfo_a.store(true, Ordering::Relaxed);
        for k in 0..params::SCOPE_LEN {
            let t = k as f32 / 9.0;
            store_f32(&ui.scope[k], 0.5 * t.sin() + 0.2 * (2.7 * t).sin());
        }
        std::fs::create_dir_all("docs").unwrap();
        for (theme, file) in [(0u8, "docs/screenshot.png"), (1, "docs/screenshot-proto.png")] {
            *params.theme.write().unwrap() = theme;
            let mut editor = Apolo9::editor(params.clone());
            if let Some((px, w, h)) = editor.screenshot(params.clone()) {
                truce_test::save_png(std::path::Path::new(file), &px, w, h);
            }
        }
    }

    #[test]
    fn default_tuning_lands_on_the_chord() {
        use crate::dsp::engine::measure_voice_freq;
        let targets = [55.0, 82.41, 110.0, 130.81, 164.81, 196.0, 261.63, 329.63];
        for (i, &t) in targets.iter().enumerate() {
            let f = measure_voice_freq(424, i, params::DEFAULT_TUNE[i], 0.92);
            let cents = 1200.0 * (f / t).log2();
            assert!(cents.abs() < 15.0, "voice {} at {f:.2} Hz, {cents:.1} cents off", i + 1);
        }
    }
}
