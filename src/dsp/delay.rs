//! MOD DELAY: two PT2399 echo chips with cross-feedback.
//!
//! The PT2399 is a 1-bit converter wrapped around 44 kbit of RAM. Its VCO
//! (pin 6 current) sets the clock; the RAM is written at ≈ f_clk / 15.5, so
//! the delay is 44 000 bits / bit-rate ≈ 683 / f_MHz ms. We run that
//! bitstream for real: a second-order sigma-delta modulator writes one bit
//! per bit-clock tick into a 44 000-bit ring, the bit read back from the
//! same address is the delayed signal, and the external filters smooth it.
//! Slowing the clock to stretch the delay therefore does what it does on the
//! hardware: the oversampling ratio collapses, quantisation noise and idle
//! tones rise into the audio band, slope overload grinds the transients, and
//! sweeping the time Doppler-shifts whatever is already stored.
//!
//! Cross-feedback: line 1 is fed by line 2 and vice versa through a TL074
//! stage with a 1N4148 limiter (the four diodes next to MIX DRY on the
//! board), so self-oscillation settles into bounded standing waves.

use super::{Rc, Svf, sane, soft_clip_asym};

/// RAM size of the PT2399.
pub const BITS: usize = 44_000;
const WORDS: usize = BITS.div_ceil(64);
/// Clock cycles per stored bit.
const CLK_PER_BIT: f64 = 15.5;
/// Full-scale of the converter (2.5 V reference, internal op-amp swing).
const FS: f64 = 1.25;

pub struct Pt2399 {
    ram: Vec<u64>,
    pos: usize,
    phase: f64,
    integ: f64,
    integ2: f64,
    fb_bit: f64,
    held: f64,
    in_lp: Svf,
    out_lp: Svf,
    out_rc: Rc,
    hp: Rc,
}

impl Pt2399 {
    pub fn new(fs: f64, seed: u64) -> Self {
        // Idle pattern (…0101…, a zero-input bitstream). Real power-up
        // RAM is random, which gives a noise burst; a plugin reset shouldn't.
        let _ = seed;
        let ram = vec![0x5555_5555_5555_5555u64; WORDS];
        let mut p = Self {
            ram,
            pos: 0,
            phase: 0.0,
            integ: 0.0,
            integ2: 0.0,
            fb_bit: FS,
            held: 0.0,
            in_lp: Svf::default(),
            out_lp: Svf::default(),
            out_rc: Rc::default(),
            hp: Rc::default(),
        };
        p.set_fs(fs);
        p
    }

    pub fn set_fs(&mut self, fs: f64) {
        // Input anti-alias (2-pole) and output reconstruction filters;
        // the 1.2 nF ("Delay LPF 0-5600 pF" on the tweaks sheet) adds a pole.
        self.in_lp.set(6_500.0, 0.75, fs);
        self.out_lp.set(4_600.0, 0.72, fs);
        self.out_rc.set(8_800.0, fs);
        self.hp.set(28.0, fs);
    }

    /// Panic: idle RAM, discharged integrators, quiet filters.
    pub fn clear(&mut self) {
        self.ram.iter_mut().for_each(|w| *w = 0x5555_5555_5555_5555);
        self.integ = 0.0;
        self.integ2 = 0.0;
        self.fb_bit = FS;
        self.held = 0.0;
        self.phase = 0.0;
        self.in_lp.reset();
        self.out_lp.reset();
        self.out_rc.reset(0.0);
        self.hp.reset(0.0);
    }

    /// One bit-clock tick: modulate `x`, store the new bit, return the bit
    /// that was stored at this address one RAM-length ago.
    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        // Second-order sigma-delta (CIFB). The integrators saturate like the
        // chip's internal op-amps, so overload grinds instead of blowing up.
        self.integ = (self.integ * 0.9998 + (x - self.fb_bit)).clamp(-4.0 * FS, 4.0 * FS);
        self.integ2 = (self.integ2 * 0.9998 + (self.integ - self.fb_bit)).clamp(-8.0 * FS, 8.0 * FS);
        let bit = self.integ2 >= 0.0;
        self.fb_bit = if bit { FS } else { -FS };
        let w = self.pos >> 6;
        let m = 1u64 << (self.pos & 63);
        let old = self.ram[w] & m != 0;
        if bit {
            self.ram[w] |= m;
        } else {
            self.ram[w] &= !m;
        }
        self.pos += 1;
        if self.pos == BITS {
            self.pos = 0;
        }
        if old { FS } else { -FS }
    }

    /// `f_bit`: RAM bit rate in Hz. Returns the delayed signal (volts).
    ///
    /// The read-back bitstream is a piecewise-constant signal; it is
    /// integrated exactly over this sample's time span (continuous-time
    /// demodulation), so the shaped noise near f_bit/2 is not folded into
    /// the audio band by sampling.
    #[inline]
    pub fn process(&mut self, x: f64, f_bit: f64, dt: f64) -> f64 {
        // Internal op-amps of the 5 V chip clip softly.
        let x = soft_clip_asym(self.in_lp.lp(x), FS * 1.1, -FS * 1.1, 0.25);
        let span = f_bit * dt;
        let mut remaining = span;
        let mut acc = 0.0;
        let mut guard = 0;
        while remaining > 0.0 {
            let room = 1.0 - self.phase;
            if remaining < room {
                acc += self.held * remaining;
                self.phase += remaining;
                break;
            }
            acc += self.held * room;
            remaining -= room;
            self.phase = 0.0;
            self.held = self.tick(x);
            guard += 1;
            if guard > 256 {
                break;
            }
        }
        let avg = acc / span.max(1e-12);
        let y = self.out_rc.lp(self.out_lp.lp(avg));
        sane(self.hp.hp(y))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DelayModSource {
    Lfo,
    SelfMod,
    Cv,
}

#[derive(Clone, Copy, Default)]
pub struct DelayControls {
    /// TIME pot resistance in kΩ for each line.
    pub r_time_k: [f64; 2],
    /// MOD depth (0..1, after taper) for each line.
    pub mod_depth: [f64; 2],
    /// Feedback loop gain.
    pub fb: f64,
    /// MIX (0 = dry, 1 = wet).
    pub mix: f64,
}

pub struct ModDelay {
    pub lines: [Pt2399; 2],
    out: [f64; 2],
    self_lp: [Rc; 2],
    k: [f64; 2],
    /// Last bit rates (for the GUI).
    pub f_bit: [f64; 2],
}

impl ModDelay {
    pub fn new(fs: f64, pt_k: [f64; 2]) -> Self {
        let mut self_lp = [Rc::default(), Rc::default()];
        for r in &mut self_lp {
            r.set(1_500.0, fs);
        }
        Self {
            lines: [Pt2399::new(fs, 0x2399_0001), Pt2399::new(fs, 0x2399_0002)],
            out: [0.0; 2],
            self_lp,
            k: pt_k,
            f_bit: [0.0; 2],
        }
    }

    pub fn set_fs(&mut self, fs: f64) {
        for l in &mut self.lines {
            l.set_fs(fs);
        }
        for r in &mut self.self_lp {
            r.set(1_500.0, fs);
        }
    }

    /// Panic: both lines emptied, the feedback loop broken.
    pub fn clear(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.out = [0.0; 2];
        for r in &mut self.self_lp {
            r.reset(0.0);
        }
    }

    /// Delay time in seconds for a TIME resistance (no modulation).
    pub fn nominal_time(r_time_k: f64, k: f64) -> f64 {
        let f_clk = 59.6e6 * k / (r_time_k + 2.21);
        BITS as f64 * CLK_PER_BIT / f_clk
    }

    /// `x`: input (volts). `lfo_tri`/`lfo_sq` LFO voltages, `cv` CV DELAY
    /// (volts, only when plugged). Returns (mixed output, wet only).
    #[inline]
    pub fn process(
        &mut self,
        x: f64,
        c: &DelayControls,
        source: DelayModSource,
        lfo: f64,
        cv: f64,
        dt: f64,
    ) -> (f64, f64) {
        let mut wet = 0.0;
        let prev = self.out;
        for i in 0..2 {
            let v_mod = match source {
                DelayModSource::Lfo => lfo,
                DelayModSource::SelfMod => 9.0 * self.self_lp[i].lp(prev[i]),
                DelayModSource::Cv => cv,
            };
            // Pin-6 current: 1/(R+R0) plus the injected modulation current.
            let i_ctl = 1.0 / (c.r_time_k[i] + 2.21) + c.mod_depth[i] * v_mod * 0.004;
            let f_clk = 59.6e6 * self.k[i] * i_ctl.clamp(0.0055, 0.45);
            let f_bit = f_clk / CLK_PER_BIT;
            self.f_bit[i] = f_bit;
            // Cross-feedback through the diode-limited TL074 stage.
            let fb_src = prev[1 - i];
            let fb = c.fb * diode_limit(fb_src * 1.0);
            let y = self.lines[i].process(x + fb, f_bit, dt);
            self.out[i] = y;
            wet += y;
        }
        // Two-tap mixer.
        let wet = wet * 0.7;
        let out = x * (1.0 - c.mix) + wet * c.mix * 1.25;
        (out, wet)
    }
}

/// Two 1N4148 per polarity in the feedback of a TL074: soft limit at about
/// ±1.2 V, with the slight mismatch of real diodes.
#[inline]
fn diode_limit(v: f64) -> f64 {
    if v >= 0.0 {
        1.22 * (v / 1.22).tanh()
    } else {
        1.16 * (v / 1.16).tanh()
    }
}
