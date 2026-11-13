//! The whole instrument at the oversampled internal rate.
//!
//! Signal order per internal sample (what is wired to what, not a chain of
//! independent blocks):
//!
//! ```text
//!  Hyper LFO ─┬─────────────────────────────┐ (delay mod)
//!             │ LFO CV / TOTAL FB / CV VOICES│
//!  vibrato ×8 ┼──► 4093 timing nodes ◄── FM from pair buses (previous sample)
//!  sensors ──► BC557 envelopes ──┘   │  ▲ ground bounce / rail sag
//!                                    ▼  │
//!                         junction VCAs → pair mixers (FM buses)
//!                                    → master mix (+EXT IN, bleed, hiss)
//!                                    → PT2399 ×2 cross-feedback
//!                                    → TL074 distortion → EQ → VOL → out
//!                                        └──── TOTAL FB tap ────┘
//! ```

use super::delay::{DelayControls, DelayModSource, ModDelay};
use super::dist::Distortion;
use super::lfo::{HyperLfo, LfoCombine, VibratoBank};
use super::resample::Decimator;
use super::tolerance::Unit;
use super::voice::{R_TUNE_POT, R_VIB, Voice, VoiceInputs};
use super::{Rc, Rng, VBIAS, VDD, VOLTS_PER_FS, fexp, opamp_rail, sane, taper_a};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModSrc {
    Off,
    Voices,
    Lfo,
}

/// Raw panel state (knob positions 0..1, switch states), filled from the
/// plugin parameters once per block.
#[derive(Clone, Copy)]
pub struct Controls {
    pub tune: [f64; 8],
    pub sharp: [f64; 4],
    pub fm_mod: [f64; 4],
    pub fm_src: [ModSrc; 4],
    pub fast: [bool; 4],
    pub pitch: [f64; 2],
    pub hold: [f64; 2],
    pub vibrato: bool,
    pub total_fb: bool,
    /// FM structure switch up: 34→56, 78→12 (one closed loop).
    pub fm_loop: bool,
    pub lfo_freq: [f64; 2],
    pub lfo_and: bool,
    pub link: bool,
    pub time: [f64; 2],
    pub dly_mod: [f64; 2],
    pub dly_self: bool,
    pub dly_square: bool,
    pub fb: f64,
    pub dly_mix: f64,
    pub drive: f64,
    pub dist_mix: f64,
    pub vol: f64,
    pub hold_gate: Option<f64>,
    pub cv_voices: Option<f64>,
    pub cv_delay: Option<f64>,
    pub humidity: f64,
    pub crosstalk: f64,
    pub noise: f64,
    pub ext_gain: f64,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            tune: [0.5; 8],
            sharp: [0.0; 4],
            fm_mod: [0.0; 4],
            fm_src: [ModSrc::Off; 4],
            fast: [false; 4],
            pitch: [0.9; 2],
            hold: [0.0; 2],
            vibrato: false,
            total_fb: false,
            fm_loop: false,
            lfo_freq: [0.4; 2],
            lfo_and: false,
            link: false,
            time: [0.5; 2],
            dly_mod: [0.0; 2],
            dly_self: false,
            dly_square: false,
            fb: 0.3,
            dly_mix: 0.35,
            drive: 0.0,
            dist_mix: 0.0,
            vol: 0.8,
            hold_gate: None,
            cv_voices: None,
            cv_delay: None,
            humidity: 0.0,
            crosstalk: 0.35,
            noise: 1.0,
            ext_gain: 1.0,
        }
    }
}

/// PITCH knob → charge-current scale (normal position near the top).
pub fn pitch_k(x: f64) -> f64 {
    0.125 + 0.875 * x * x
}

/// TUNE knob → pot resistance (reverse-log wiring: clockwise = higher).
pub fn tune_r(x: f64) -> f64 {
    R_TUNE_POT * taper_a(1.0 - x) + 50.0
}

/// FREQ knob of the Hyper LFO → 1 k + C100k track.
pub fn lfo_r(x: f64) -> f64 {
    1e3 + 100e3 * taper_a(1.0 - x)
}

/// TIME knob → PT2399 pin-6 resistance in kΩ (0.47 k minimum).
pub fn time_r_k(x: f64) -> f64 {
    0.47 + 100.0 * taper_a(x)
}

/// Smoothed physical quantities (pots don't jump).
#[derive(Clone, Copy, Default)]
struct Smooth {
    ln_r_tune: [f64; 8],
    sharp: [f64; 4],
    fm: [f64; 4],
    pitch: [f64; 2],
    hold: [f64; 2],
    ln_lfo_r: [f64; 2],
    ln_time: [f64; 2],
    dmod: [f64; 2],
    fb: f64,
    dmix: f64,
    drive: f64,
    dist_mix: f64,
    vol: f64,
}

impl Smooth {
    fn targets(c: &Controls) -> Self {
        let hold_scale = c.hold_gate.map_or(1.0, |v| (v / 5.0).clamp(0.0, 1.2));
        Self {
            ln_r_tune: std::array::from_fn(|i| tune_r(c.tune[i]).ln()),
            sharp: c.sharp,
            fm: std::array::from_fn(|p| if c.fm_src[p] == ModSrc::Off { 0.0 } else { taper_a(c.fm_mod[p]) }),
            pitch: [pitch_k(c.pitch[0]), pitch_k(c.pitch[1])],
            hold: std::array::from_fn(|g| VDD * c.hold[g].clamp(0.0, 1.0).powf(1.5) * hold_scale),
            ln_lfo_r: [lfo_r(c.lfo_freq[0]).ln(), lfo_r(c.lfo_freq[1]).ln()],
            ln_time: [time_r_k(c.time[0]).ln(), time_r_k(c.time[1]).ln()],
            dmod: [taper_a(c.dly_mod[0]), taper_a(c.dly_mod[1])],
            fb: 2.05 * c.fb,
            dmix: c.dly_mix,
            drive: c.drive,
            dist_mix: c.dist_mix,
            vol: c.vol,
        }
    }

    #[inline]
    fn approach(&mut self, t: &Self, a: f64) {
        // Land exactly on the target once close: a knob resting at zero
        // must not leave its smoother decaying into subnormals forever.
        #[inline]
        fn one(s: &mut f64, t: f64, a: f64) {
            let d = t - *s;
            *s = if d.abs() < 1e-9 { t } else { *s + a * d };
        }
        fn arr<const N: usize>(s: &mut [f64; N], t: &[f64; N], a: f64) {
            for i in 0..N {
                one(&mut s[i], t[i], a);
            }
        }
        arr(&mut self.ln_r_tune, &t.ln_r_tune, a);
        arr(&mut self.sharp, &t.sharp, a);
        arr(&mut self.fm, &t.fm, a);
        arr(&mut self.pitch, &t.pitch, a);
        arr(&mut self.hold, &t.hold, a);
        arr(&mut self.ln_lfo_r, &t.ln_lfo_r, a);
        arr(&mut self.ln_time, &t.ln_time, a);
        arr(&mut self.dmod, &t.dmod, a);
        one(&mut self.fb, t.fb, a);
        one(&mut self.dmix, t.dmix, a);
        one(&mut self.drive, t.drive, a);
        one(&mut self.dist_mix, t.dist_mix, a);
        one(&mut self.vol, t.vol, a);
    }
}

/// What the editor shows (copied out once per block).
#[derive(Clone, Copy, Default)]
pub struct Telemetry {
    pub env: [f32; 8],
    pub pair_level: [f32; 4],
    pub lfo_a: bool,
    pub lfo_b: bool,
    pub delay_ms: [f32; 2],
    pub vib: [f32; 8],
}

pub struct Engine {
    fs_host: f64,
    os: usize,
    fs: f64,
    dt: f64,
    pub serial: u32,
    unit: Unit,
    voices: [Voice; 8],
    vib: VibratoBank,
    lfo: HyperLfo,
    delay: ModDelay,
    dist: Distortion,
    dec: Decimator,
    pair_lp: [Rc; 4],
    pair_out: [f64; 4],
    pair_env: [Rc; 4],
    sag: Rc,
    spikes: [f64; 2],
    spike_decay: f64,
    rng: Rng,
    /// Touch pressure per sensor, 0 = not touched (written by the plugin).
    pub touch: [f64; 8],
    g_skin: [f64; 8],
    tremor: [Rc; 8],
    sm: Smooth,
    initialised: bool,
    total_fb_tap: f64,
    ext_prev: f64,
    /// Host samples left before the output reopens after a panic.
    mute: usize,
    /// Output gain ramp (0 → 1 after a panic).
    gate: f64,
    pub telemetry: Telemetry,
}

impl Engine {
    pub fn new(fs_host: f64, serial: u32) -> Self {
        let os = oversampling_for(fs_host);
        let fs = fs_host * os as f64;
        let unit = Unit::new(serial);
        let mut e = Self {
            fs_host,
            os,
            fs,
            dt: 1.0 / fs,
            serial,
            voices: std::array::from_fn(|i| Voice::new(i, &unit, fs)),
            vib: VibratoBank::new(&unit, fs_host),
            lfo: HyperLfo::new(&unit, fs_host),
            delay: ModDelay::new(fs, unit.pt_k),
            dist: Distortion::new(fs),
            dec: Decimator::new(os),
            pair_lp: std::array::from_fn(|_| Rc::new(21_000.0, fs)),
            pair_out: [0.0; 4],
            pair_env: std::array::from_fn(|_| Rc::new(8.0, fs_host)),
            sag: Rc::new(40.0, fs),
            spikes: [0.0; 2],
            spike_decay: (-1.0 / (1.5e-6 * fs)).exp(),
            rng: Rng::new(0xA9010 ^ u64::from(serial)),
            touch: [0.0; 8],
            g_skin: [0.0; 8],
            tremor: std::array::from_fn(|_| Rc::new(9.0, fs_host)),
            sm: Smooth::default(),
            initialised: false,
            total_fb_tap: 0.0,
            ext_prev: 0.0,
            mute: 0,
            gate: 1.0,
            unit,
            telemetry: Telemetry::default(),
        };
        e.sm = Smooth::targets(&Controls::default());
        e
    }

    pub fn host_rate(&self) -> f64 {
        self.fs_host
    }

    pub fn latency(&self) -> usize {
        self.dec.latency()
    }

    /// Swap in another physical unit (allocation-free: the PT2399 RAM and
    /// the decimator are kept).
    pub fn set_serial(&mut self, serial: u32) {
        if serial == self.serial {
            return;
        }
        self.serial = serial;
        self.unit = Unit::new(serial);
        for (i, v) in self.voices.iter_mut().enumerate() {
            let ve = v.ve;
            *v = Voice::new(i, &self.unit, self.fs);
            v.ve = ve;
        }
        self.vib = VibratoBank::new(&self.unit, self.fs_host);
        self.lfo = HyperLfo::new(&self.unit, self.fs_host);
        self.rng = Rng::new(0xA9010 ^ u64::from(serial));
    }

    /// Panic: lift every finger, discharge every envelope, empty both
    /// PT2399s, break the feedback loops and silence the output for a
    /// moment. Knob settings are left alone, so a HOLD drone will bloom
    /// back slowly, the way it would after a power cycle.
    pub fn panic(&mut self) {
        self.touch = [0.0; 8];
        self.g_skin = [0.0; 8];
        for v in &mut self.voices {
            v.silence();
        }
        self.delay.clear();
        self.dist.reset();
        for r in &mut self.pair_lp {
            r.reset(0.0);
        }
        self.pair_out = [0.0; 4];
        self.sag.reset(0.0);
        self.spikes = [0.0; 2];
        self.total_fb_tap = 0.0;
        self.ext_prev = 0.0;
        self.dec.reset();
        self.mute = (self.fs_host * 0.15) as usize;
        self.gate = 0.0;
    }

    /// Render one host block. `ext`: EXT IN at host rate (mono).
    pub fn process(&mut self, c: &Controls, ext: Option<&[f32]>, out: &mut [f32]) {
        let _ftz = super::NoDenormals::new();
        let targets = Smooth::targets(c);
        if !self.initialised {
            self.sm = targets;
            self.initialised = true;
        }
        let dt = self.dt;
        let dt_h = 1.0 / self.fs_host;
        let a = 1.0 - (-dt_h / 0.012).exp();
        let inv_os = 1.0 / self.os as f64;
        let noise = c.noise;
        let xt = c.crosstalk;
        // Threshold noise scaled so its spectral density is rate-independent.
        let vt_noise = 0.4e-3 * noise * (self.fs / 192_000.0).sqrt();
        let hiss = 0.30e-3 * noise * (self.fs * 0.5 / 20_000.0).sqrt();
        // Thermal drift runs once per host sample: same pole and variance
        // as an update on every internal sample.
        let drift_pole = 0.999_995f64.powi(self.os as i32);
        let drift_gain = 2.0e-6 * noise * (self.os as f64).sqrt();
        let lfo_mode = if c.lfo_and { LfoCombine::And } else { LfoCombine::Or };
        let dsrc = if c.cv_delay.is_some() {
            DelayModSource::Cv
        } else if c.dly_self {
            DelayModSource::SelfMod
        } else {
            DelayModSource::Lfo
        };
        let cv_delay = c.cv_delay.unwrap_or(0.0);
        let skin_rise = 1.0 - (-dt_h / 0.008).exp();
        let skin_fall = 1.0 - (-dt_h / 0.025).exp();
        let r_on_12 = super::schmitt::r_on(VDD);
        let mut g_target = [0.0; 8];
        for i in 0..8 {
            let p = self.touch[i].clamp(0.0, 1.0);
            let g_touch = if p > 0.0 { 10f64.powf(-(10.5 - 5.7 * p)) } else { 0.0 };
            let h = c.humidity.clamp(0.0, 1.0);
            let g_hum = h * h * h / (80e6 * self.unit.leak[i]);
            g_target[i] = g_touch + g_hum;
        }
        // Vibrato: AC-coupled, practically a current source.
        let g_vib: [f64; 8] = std::array::from_fn(|i| 1.0 / (R_VIB * if i == 3 || i == 4 { 24.0 / 47.0 } else { 1.0 }));
        let fm_src: [usize; 4] = std::array::from_fn(|p| self.fm_source(p, c.fm_loop));
        let g_fm: [f64; 8] = std::array::from_fn(|i| 1.0 / (2.2e3 * self.unit.fm_r[i]));

        for (n, o) in out.iter_mut().enumerate() {
            // ===== Control rate (host sample) ===========================
            self.sm.approach(&targets, a);
            let sm = self.sm;
            let r_tune: [f64; 8] = std::array::from_fn(|i| fexp(sm.ln_r_tune[i]));
            let dc = DelayControls {
                r_time_k: [fexp(sm.ln_time[0]), fexp(sm.ln_time[1])],
                mod_depth: sm.dmod,
                fb: sm.fb,
                mix: sm.dmix,
            };
            // Slow oscillators: Hyper LFO (≤ 40 Hz) and the vibrato bank.
            self.lfo.step(fexp(sm.ln_lfo_r[0]), fexp(sm.ln_lfo_r[1]), c.link, lfo_mode, dt_h);
            self.vib.step(c.vibrato, dt_h);
            let lfo_dly = if c.dly_square { self.lfo.square } else { self.lfo.tri };
            // Sensors and envelopes.
            for i in 0..8 {
                let gt = g_target[i];
                // Skin: contact builds up fast, moisture lingers.
                let rate = if gt > self.g_skin[i] { skin_rise } else { skin_fall };
                self.g_skin[i] += rate * (gt - self.g_skin[i]);
                let tremor = 1.0 + 0.9 * noise * self.tremor[i].lp(self.rng.gauss());
                let g = self.g_skin[i] * tremor.max(0.0);
                self.voices[i].envelope(g, c.fast[i / 2], sm.hold[i / 4], dt_h);
            }
            let fm_k: [f64; 8] = std::array::from_fn(|i| sm.fm[i / 2] * g_fm[i]);
            for i in 0..8 {
                let inp = VoiceInputs {
                    r_tune: r_tune[i],
                    pitch_k: sm.pitch[i / 4],
                    i_slow: self.vib.out[i] * g_vib[i],
                    g_fm: fm_k[i] * 0.03,
                    sharp: sm.sharp[i / 2],
                    vdd: VDD,
                    r_on: r_on_12,
                    drift_pole,
                    drift_gain,
                };
                self.voices[i].prepare(&inp, dt, &mut self.rng);
            }
            let ext_now = ext.map_or(0.0, |e| f64::from(e[n])) * c.ext_gain;
            let mut y_host = 0.0;

            // ===== Audio rate (oversampled) =============================
            for k in 0..self.os {
                let ext_v = self.ext_prev + (ext_now - self.ext_prev) * (k + 1) as f64 * inv_os;

                // LFO CV bus for the voices: CV VOICES > TOTAL FB > LFO.
                let lfo_cv = if let Some(v) = c.cv_voices {
                    v
                } else if c.total_fb {
                    self.total_fb_tap * 2.0
                } else {
                    self.lfo.cv * 0.25
                };

                // --- Ground bounce / rail sag ----------------------------
                let mut i_tot = 0.0;
                for v in &self.voices {
                    i_tot += v.i_vca;
                }
                let sag = self.sag.lp(i_tot) * 0.02 * xt;
                self.spikes[0] *= self.spike_decay;
                self.spikes[1] *= self.spike_decay;
                let spikes = self.spikes;

                // --- Voices ---------------------------------------------
                let mut pair_sum = [0.0; 4];
                let mut bleed = 0.0;
                for i in 0..8 {
                    let p = i / 2;
                    let g = i / 4;
                    let v_src = match c.fm_src[p] {
                        ModSrc::Off => 0.0,
                        ModSrc::Voices => self.pair_out[fm_src[p]],
                        ModSrc::Lfo => lfo_cv,
                    };
                    let vt_off = sag * 0.5
                        + spikes[g]
                        + 0.3 * spikes[1 - g]
                        + self.rng.gauss() * vt_noise;
                    let v = self.voices[i].step(fm_k[i] * v_src, vt_off, dt);
                    pair_sum[p] += v;
                    bleed += self.voices[i].bleed;
                    let e = self.voices[i].last.edges;
                    if e != 0 {
                        self.spikes[g] += 0.018 * xt * f64::from(e);
                    }
                }
                for p in 0..4 {
                    // TL074 summing amp: 68 k in, 75 k ‖ 100 pF feedback.
                    let v = opamp_rail(-1.1 * pair_sum[p]);
                    self.pair_out[p] = self.pair_lp[p].lp(v);
                }

                // --- Master mixer ----------------------------------------
                let mut mix = 0.0;
                for p in 0..4 {
                    mix += self.pair_out[p];
                }
                let mix = opamp_rail(
                    -0.55 * mix + ext_v + bleed * 0.0006 * (0.3 + xt) + self.rng.gauss() * hiss,
                );

                // --- Delay ----------------------------------------------
                let (dly_out, _) = self.delay.process(mix, &dc, dsrc, lfo_dly, cv_delay, dt);

                // --- Distortion / output --------------------------------
                let (y, tap) = self.dist.process(dly_out, sm.drive, sm.dist_mix, sm.vol);
                self.total_fb_tap = tap;
                if let Some(d) = self.dec.push(sane(y)) {
                    y_host = d;
                }
            }
            for p in 0..4 {
                self.telemetry.pair_level[p] = self.pair_env[p].lp(self.pair_out[p].abs()) as f32;
            }
            self.ext_prev = ext_now;
            // Panic gate: hold silent, then reopen over ~50 ms.
            if self.mute > 0 {
                self.mute -= 1;
                y_host = 0.0;
            } else if self.gate < 1.0 {
                self.gate = (self.gate + dt_h / 0.05).min(1.0);
                y_host *= self.gate;
            }
            *o = (y_host / VOLTS_PER_FS) as f32;
        }

        // Telemetry for the editor.
        let t = &mut self.telemetry;
        for i in 0..8 {
            t.env[i] = (self.voices[i].ve / 10.0) as f32;
            t.vib[i] = self.vib.out[i] as f32;
        }
        t.lfo_a = self.lfo.a_high;
        t.lfo_b = self.lfo.b_high;
        for l in 0..2 {
            let fb = self.delay.f_bit[l].max(1.0);
            t.delay_ms[l] = (super::delay::BITS as f64 / fb * 1000.0) as f32;
        }
        let _ = VBIAS;
    }

    /// Which pair modulates pair `p` when its switch is on VOICES.
    #[inline]
    fn fm_source(&self, p: usize, fm_loop: bool) -> usize {
        match (p, fm_loop) {
            (0, false) => 1,
            (1, _) => 0,
            (2, false) => 3,
            (3, _) => 2,
            (0, true) => 3,
            (2, true) => 1,
            _ => 0,
        }
    }
}

/// Internal rate ≥ 176.4 kHz.
pub fn oversampling_for(fs: f64) -> usize {
    if fs >= 170_000.0 {
        1
    } else if fs >= 85_000.0 {
        2
    } else {
        4
    }
}

/// Measure the steady frequency of voice `idx` (envelope fully open) for a
/// TUNE / PITCH setting. Used to calibrate the default tunings and in tests.
pub fn measure_voice_freq(serial: u32, idx: usize, tune: f64, pitch: f64) -> f64 {
    let fs = 192_000.0;
    let unit = Unit::new(serial);
    let mut v = Voice::new(idx, &unit, fs);
    let mut rng = Rng::new(1);
    let inp = VoiceInputs {
        r_tune: tune_r(tune),
        pitch_k: pitch_k(pitch),
        vdd: VDD,
        r_on: super::schmitt::r_on(VDD),
        drift_pole: 1.0,
        ..Default::default()
    };
    let dt = 1.0 / fs;
    // Settle: envelope up and the oscillator running.
    for _ in 0..(fs * 0.3) as usize {
        v.envelope(1.0 / 60e3, false, 0.0, dt);
        v.prepare(&inp, dt, &mut rng);
        v.step(0.0, 0.0, dt);
    }
    let mut first = None;
    let mut last = 0usize;
    let mut count = 0usize;
    let max = (fs * 4.0) as usize;
    for k in 0..max {
        v.envelope(1.0 / 60e3, false, 0.0, dt);
        v.prepare(&inp, dt, &mut rng);
        v.step(0.0, 0.0, dt);
        if v.last.edges > 0 {
            if first.is_none() {
                first = Some(k);
            } else {
                count += 1;
                last = k;
            }
            if count >= 64 {
                break;
            }
        }
    }
    match first {
        Some(f) if count > 0 => count as f64 * fs / (last - f) as f64,
        _ => 0.0,
    }
}

/// Knob position giving `target` Hz on voice `idx` (bisection; pitch rises
/// monotonically with the knob).
pub fn tune_for(serial: u32, idx: usize, target: f64, pitch: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        if measure_voice_freq(serial, idx, mid, pitch) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Knobs resting at zero must settle exactly, not decay into subnormals
    /// (which are ruinously slow on x86).
    #[test]
    fn smoothers_settle_without_subnormals() {
        let mut e = Engine::new(48_000.0, 424);
        let mut c = Controls::default();
        let mut buf = [0.0f32; 480];
        c.sharp = [0.7; 4];
        c.drive = 0.5;
        c.dist_mix = 0.5;
        c.dly_mod = [0.5; 2];
        e.process(&c, None, &mut buf);
        c.sharp = [0.0; 4];
        c.drive = 0.0;
        c.dist_mix = 0.0;
        c.dly_mod = [0.0; 2];
        c.fb = 0.0;
        for _ in 0..3000 {
            e.process(&c, None, &mut buf);
        }
        let s = e.sm;
        let all = s.sharp.iter().chain(&s.fm).chain(&s.dmod).chain(&s.hold).chain([&s.fb, &s.drive, &s.dist_mix]);
        for v in all {
            assert!(*v == 0.0, "smoother stuck at {v:e}");
        }
    }

    #[test]
    fn fast_exp_matches_libm() {
        let mut worst = 0.0f64;
        let mut x = -40.0;
        while x < 40.0 {
            let (a, b) = (crate::dsp::fexp(x), x.exp());
            worst = worst.max(((a - b) / b).abs());
            x += 0.001_37;
        }
        assert!(worst < 4e-16, "worst relative error {worst:e}");
    }
}
