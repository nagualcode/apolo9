//! Offline renderer for listening tests and analysis.
//!
//! `apolo9-render <out.wav> <scenario> [seconds] [sample_rate]`
//! Scenarios: organ, fm, drone, delay, chaos, touch, bleed, cv

use apolo9::dsp::engine::{Controls, Engine, ModSrc};
use apolo9::params::DEFAULT_TUNE;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "apolo9.wav".into());
    let scenario = args.get(2).cloned().unwrap_or_else(|| "organ".into());
    let secs: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8.0);
    let fs: f64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(48_000.0);

    if out == "calib" {
        calib();
        return;
    }
    if out == "tunes" {
        // apolo9-render tunes <pitchA> <pitchB> f1 .. f8   (0 = leave out)
        use apolo9::dsp::engine::tune_for;
        let pa: f64 = args[2].parse().unwrap();
        let pb: f64 = args[3].parse().unwrap();
        let v: Vec<String> = (0..8)
            .map(|i| {
                let f: f64 = args[4 + i].parse().unwrap();
                if f <= 0.0 { "-".into() } else { format!("{:.4}", tune_for(424, i, f, if i < 4 { pa } else { pb })) }
            })
            .collect();
        println!("{}", v.join(" "));
        return;
    }
    let mut c = Controls::default();
    c.tune = DEFAULT_TUNE;
    let preset = scenario
        .strip_prefix("preset:")
        .or_else(|| scenario.strip_prefix("demo:"))
        .map(load_preset);
    if let Some(pc) = preset {
        c = pc;
    }
    let mut engine = Engine::new(fs, 424);
    let n = (secs * fs) as usize;
    let block = 256;
    let mut buf = vec![0.0f32; block];
    let mut all = Vec::with_capacity(n);
    let start = std::time::Instant::now();
    let mut t = 0usize;
    while t < n {
        let time = t as f64 / fs;
        script(&scenario, time, &mut c, &mut engine);
        let len = block.min(n - t);
        engine.process(&c, None, &mut buf[..len]);
        all.extend_from_slice(&buf[..len]);
        t += len;
    }
    let el = start.elapsed().as_secs_f64();
    eprintln!("rendered {secs:.1}s in {el:.2}s ({:.1}x realtime)", secs / el);
    let peak = all.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = (all.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / all.len() as f64).sqrt();
    eprintln!("peak {:.3} ({:.1} dBFS) rms {:.1} dBFS", peak, 20.0 * (peak as f64).log10(), 20.0 * rms.log10());
    let spec = hound::WavSpec { channels: 1, sample_rate: fs as u32, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(&out, spec).expect("wav");
    for s in all {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

/// Touch sensor `i` with pressure `p` between `t0` and `t1`.
fn touch(e: &mut Engine, time: f64, i: usize, t0: f64, t1: f64, p: f64) {
    if time >= t0 && time < t1 {
        e.touch[i] = p;
    } else if (time - t1).abs() < 0.01 {
        e.touch[i] = 0.0;
    }
}

/// Minimal `.preset` reader: `[params]` key = value lines → panel state.
fn load_preset(path: &str) -> Controls {
    use apolo9::params::*;
    let p = Apolo9Params::new();
    let text = std::fs::read_to_string(path).expect("preset");
    let mut in_params = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap().trim();
        if line == "[params]" {
            in_params = true;
            continue;
        }
        if !in_params || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=').unwrap();
        let (k, v) = (k.trim(), v.trim().parse::<f64>().unwrap_or(0.0));
        macro_rules! f { ($($n:ident),*) => { match k { $(stringify!($n) => p.$n.set_value(v),)* _ => {} } } }
        macro_rules! b { ($($n:ident),*) => { match k { $(stringify!($n) => p.$n.set_value(v > 0.5),)* _ => {} } } }
        f!(tune1, tune2, tune3, tune4, tune5, tune6, tune7, tune8, sharp12, sharp34, sharp56, sharp78, mod12, mod34, mod56, mod78,
           pitch_a, pitch_b, hold_a, hold_b, lfo_a, lfo_b, time1, time2, dmod1, dmod2, feedback, dly_mix, drive, dist_mix, volume,
           hold_gate, cv_voices, cv_delay, ext_gain, humidity, crosstalk, noise);
        b!(fast12, fast34, fast56, fast78, vibrato, total_fb, fm_loop, lfo_link, hold_gate_on, cv_voices_on, cv_delay_on);
        let src = |x: f64| match x as i64 { 1 => ModSource::Voices, 2 => ModSource::Lfo, _ => ModSource::Off };
        match k {
            "src12" => p.src12.set_value(src(v)),
            "src34" => p.src34.set_value(src(v)),
            "src56" => p.src56.set_value(src(v)),
            "src78" => p.src78.set_value(src(v)),
            "lfo_logic" => p.lfo_logic.set_value(if v > 0.5 { LfoLogic::And } else { LfoLogic::Or }),
            "dly_mode" => p.dly_mode.set_value(if v > 0.5 { DelayModMode::SelfMod } else { DelayModMode::Lfo }),
            "dly_wave" => p.dly_wave.set_value(if v > 0.5 { DelayWave::Square } else { DelayWave::Tri }),
            _ => {}
        }
    }
    apolo9::controls_from(&p)
}

fn script(s: &str, time: f64, c: &mut Controls, e: &mut Engine) {
    if s.starts_with("demo:") {
        demo(time, c, e);
        return;
    }
    if s.starts_with("preset:") {
        // A short phrase on the sensors: a melody, then a chord, then silence.
        for (k, i) in [0usize, 2, 4, 5, 7, 3].iter().enumerate() {
            let t0 = 0.4 + k as f64 * 0.45;
            touch(e, time, *i, t0, t0 + 0.35, 0.85);
        }
        for i in [1usize, 4, 6] {
            touch(e, time, i, 3.5, 6.0, 0.7);
        }
        let _ = c;
        return;
    }
    match s {
        // Manual step 1: the organ.
        "organ" => {
            c.dly_mix = 0.3;
            for i in 0..8 {
                let t0 = 0.3 + i as f64 * 0.6;
                touch(e, time, i, t0, t0 + 2.5, 0.9);
            }
        }
        "single" => {
            c.dly_mix = 0.0;
            touch(e, time, 4, 0.2, 3.0, 1.0);
        }
        // Manual step 2: two cross-modulation loops, MOD at noon.
        "fm" => {
            c.fm_src = [ModSrc::Voices; 4];
            c.fm_mod = [0.5; 4];
            c.dly_mix = 0.25;
            for i in 0..8 {
                let t0 = 0.3 + i as f64 * 0.5;
                touch(e, time, i, t0, t0 + 3.0, 0.8);
            }
            if time > 5.0 {
                c.fm_mod = [0.85; 4];
            }
        }
        // HOLD drone, knobs only.
        "drone" => {
            c.hold = [0.75, 0.6];
            c.vibrato = time > 3.0;
            c.sharp = [0.2, 0.4, 0.0, 0.6];
            c.fb = 0.42;
            c.dly_mix = 0.4;
        }
        // Delay at the edge of self-oscillation, played by the voices.
        "delay" => {
            c.fb = 0.5;
            c.dly_mix = 0.6;
            c.time = [0.62, 0.74];
            c.dly_mod = [0.25, 0.3];
            touch(e, time, 2, 0.2, 0.6, 1.0);
            touch(e, time, 5, 1.2, 1.5, 1.0);
            if time > 4.0 {
                c.dly_self = true;
                c.dly_mod = [0.4, 0.5];
            }
        }
        // Everything into one structure: TOTAL FB + LFO + distortion.
        "chaos" => {
            c.fm_src = [ModSrc::Voices, ModSrc::Lfo, ModSrc::Voices, ModSrc::Lfo];
            c.fm_mod = [0.7, 0.6, 0.75, 0.55];
            c.fm_loop = true;
            c.total_fb = time > 3.0;
            c.hold = [0.6, 0.6];
            c.lfo_freq = [0.55, 0.62];
            c.link = true;
            c.fb = 0.47;
            c.dly_mix = 0.45;
            c.dly_mod = [0.3, 0.2];
            c.drive = 0.7;
            c.dist_mix = 0.6;
        }
        // Touch dynamics: firm, light, fast strokes.
        "touch" => {
            c.dly_mix = 0.0;
            touch(e, time, 3, 0.2, 1.2, 1.0);
            touch(e, time, 3, 2.0, 3.5, 0.35);
            for k in 0..10 {
                let t0 = 4.5 + k as f64 * 0.12;
                touch(e, time, 3, t0, t0 + 0.04, 0.5);
            }
        }
        "release" => {
            c.dly_mix = 0.0;
            touch(e, time, 3, 0.2, 0.6, 1.0);
            c.fast[2] = time > 9.0;
            touch(e, time, 5, 9.0, 9.4, 1.0);
        }
        "light" => {
            c.dly_mix = 0.0;
            touch(e, time, 3, 0.2, 2.2, 0.2);
            touch(e, time, 3, 3.0, 5.0, 0.3);
            touch(e, time, 3, 6.0, 8.0, 0.45);
        }
        // Silence: only bleed and noise.
        "bleed" => {
            c.dly_mix = 0.3;
            c.vol = 1.0;
        }
        "humid" => {
            c.humidity = 0.6;
        }
        _ => {}
    }
}

fn calib() {
    use apolo9::dsp::engine::{measure_voice_freq, tune_for};
    for i in 0..8 {
        let lo = measure_voice_freq(424, i, 0.0, 0.92);
        let mid = measure_voice_freq(424, i, 0.5, 0.92);
        let hi = measure_voice_freq(424, i, 1.0, 0.92);
        println!("voice {}: tune 0 -> {lo:8.2} Hz, 0.5 -> {mid:8.2} Hz, 1 -> {hi:9.2} Hz", i + 1);
    }
    // A1 E2 A2 C3 E3 G3 C4 E4
    let targets = [55.0, 82.41, 110.0, 130.81, 164.81, 196.0, 261.63, 329.63];
    let mut v = Vec::new();
    for (i, &t) in targets.iter().enumerate() {
        let k = tune_for(424, i, t, 0.92);
        v.push(format!("{k:.4}"));
    }
    println!("DEFAULT_TUNE = [{}]", v.join(", "));
}

/// The website demo: a slow improvisation on the Eno sheet (D pentatonic:
/// D2 A2 D3 E3 F#3 A3 B3 D4), then the pairs bend each other, then it fades.
fn demo(time: f64, c: &mut Controls, e: &mut Engine) {
    // (sensor, start, length, pressure)
    const NOTES: &[(usize, f64, f64, f64)] = &[
        (4, 0.5, 2.2, 0.75), (5, 2.4, 1.6, 0.6), (6, 3.8, 2.6, 0.8), (5, 6.2, 1.2, 0.5),
        (3, 7.2, 2.8, 0.7), (0, 9.5, 9.0, 0.9), (4, 10.4, 1.8, 0.6), (7, 12.0, 3.0, 0.85),
        (6, 14.6, 1.4, 0.55), (1, 15.5, 8.5, 0.8), (5, 16.0, 2.0, 0.7), (4, 18.2, 1.6, 0.6),
        (3, 19.6, 3.4, 0.75), (2, 21.0, 6.0, 0.7), (6, 23.0, 2.0, 0.8), (7, 25.0, 2.6, 0.9),
        (5, 27.4, 1.6, 0.6), (4, 28.8, 2.4, 0.7), (0, 29.0, 6.0, 0.85), (3, 31.0, 3.0, 0.65),
        (6, 33.4, 2.0, 0.55),
    ];
    for &(i, t0, len, p) in NOTES {
        if time >= t0 && time < t0 + len {
            e.touch[i] = p;
        } else if time >= t0 + len && time < t0 + len + 0.02 {
            e.touch[i] = 0.0;
        }
    }
    // Middle section: the pairs start to lean on each other, a little pepper.
    let k = ((time - 19.0) / 6.0).clamp(0.0, 1.0) * (1.0 - ((time - 31.0) / 4.0).clamp(0.0, 1.0));
    use apolo9::dsp::engine::ModSrc;
    c.fm_src = [ModSrc::Voices; 4];
    c.fm_mod = [0.32 * k, 0.4 * k, 0.4 * k, 0.3 * k];
    c.drive = 0.05 + 0.3 * k;
    c.dist_mix = 0.05 + 0.25 * k;
}
