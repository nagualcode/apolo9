//! Apolo9 circuit model.
//!
//! Every block works in **volts and seconds**, at an oversampled internal
//! rate, so component values from the Lyra-8 DIY board (see
//! `docs/CIRCUIT.md`) can be dropped in directly. The host-rate wrapper
//! (`engine.rs`) only converts the final output voltage to digital full
//! scale and decimates.

pub mod delay;
pub mod dist;
pub mod engine;
pub mod lfo;
pub mod resample;
pub mod schmitt;
pub mod tolerance;
pub mod voice;

/// Main supply rail (DC 12 V adapter, "VCC+++" on the IN/OUT board).
pub const VDD: f64 = 12.0;
/// Virtual ground of the single-supply TL074 stages (test point "5.87V").
pub const VBIAS: f64 = 5.87;
/// TL074 on a 12 V single rail: usable output swing around `VBIAS`.
pub const OPAMP_HI: f64 = 12.0 - 1.4 - VBIAS; // ≈ +4.73 V
pub const OPAMP_LO: f64 = 1.75 - VBIAS; // ≈ -4.12 V (TL07x can't reach V-)
/// Output voltage that maps to digital full scale (spec: 2 V 0-to-peak max).
pub const VOLTS_PER_FS: f64 = 2.5;

/// Log ("A") potentiometer law: 10 % of the track at mid rotation.
#[inline]
pub fn taper_a(x: f64) -> f64 {
    const K: f64 = 4.4;
    let x = x.clamp(0.0, 1.0);
    (((K * x).exp()) - 1.0) / (K.exp() - 1.0)
}

/// Reverse-log ("C") potentiometer law.
#[inline]
pub fn taper_c(x: f64) -> f64 {
    1.0 - taper_a(1.0 - x)
}

/// Op-amp output stage of a single-supply TL074: hard rails with a narrow
/// soft knee (the output transistors leave saturation gradually), and the
/// swing is asymmetric because the TL07x cannot pull close to V-.
#[inline]
pub fn opamp_rail(v: f64) -> f64 {
    soft_clip_asym(v, OPAMP_HI, OPAMP_LO, 0.35)
}

/// Asymmetric clip with a quadratic knee of width `knee` volts.
#[inline]
pub fn soft_clip_asym(v: f64, hi: f64, lo: f64, knee: f64) -> f64 {
    if v > hi - knee {
        let over = v - (hi - knee);
        if over >= 2.0 * knee {
            hi
        } else {
            hi - knee + over - over * over / (4.0 * knee)
        }
    } else if v < lo + knee {
        let under = (lo + knee) - v;
        if under >= 2.0 * knee {
            lo
        } else {
            lo + knee - under + under * under / (4.0 * knee)
        }
    } else {
        v
    }
}

/// Logistic, used to smooth junction "switch-on" behaviour.
#[inline]
pub fn sigmoid(x: f64) -> f64 {
    if x > 30.0 {
        1.0
    } else if x < -30.0 {
        0.0
    } else {
        1.0 / (1.0 + (-x).exp())
    }
}

/// `ln(1 + e^x)`: the forward-biased junction current curve.
#[inline]
pub fn softplus(x: f64) -> f64 {
    if x > 30.0 { x } else { x.exp().ln_1p() }
}

/// Small, fast, allocation-free PRNG (xorshift64*).
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [0, 1).
    #[inline]
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
    /// Uniform in [-1, 1).
    #[inline]
    pub fn bipolar(&mut self) -> f64 {
        self.uniform() * 2.0 - 1.0
    }
    /// Approximately Gaussian (Irwin-Hall of 4), unit variance.
    #[inline]
    pub fn gauss(&mut self) -> f64 {
        (self.uniform() + self.uniform() + self.uniform() + self.uniform() - 2.0) * 1.732_050_8
    }
}

/// One-pole low-pass (RC), with a matching high-pass output.
#[derive(Clone, Default)]
pub struct Rc {
    a: f64,
    z: f64,
}

impl Rc {
    pub fn new(fc: f64, fs: f64) -> Self {
        let mut f = Self::default();
        f.set(fc, fs);
        f
    }
    pub fn set(&mut self, fc: f64, fs: f64) {
        self.a = 1.0 - (-std::f64::consts::TAU * fc / fs).exp();
    }
    /// Set from a time constant τ = RC.
    pub fn set_tau(&mut self, tau: f64, fs: f64) {
        self.a = 1.0 - (-1.0 / (tau * fs)).exp();
    }
    #[inline]
    pub fn lp(&mut self, x: f64) -> f64 {
        self.z += self.a * (x - self.z);
        self.z
    }
    #[inline]
    pub fn hp(&mut self, x: f64) -> f64 {
        x - self.lp(x)
    }
    #[inline]
    pub fn value(&self) -> f64 {
        self.z
    }
    pub fn reset(&mut self, v: f64) {
        self.z = v;
    }
}

/// Topology-preserving state-variable filter (Sallen-Key-like 2-pole stages
/// around the PT2399 and the output EQ).
#[derive(Clone, Default)]
pub struct Svf {
    g: f64,
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
    ic1: f64,
    ic2: f64,
}

pub struct SvfOut {
    pub lp: f64,
    pub bp: f64,
    pub hp: f64,
}

impl Svf {
    pub fn new(fc: f64, q: f64, fs: f64) -> Self {
        let mut s = Self::default();
        s.set(fc, q, fs);
        s
    }
    pub fn set(&mut self, fc: f64, q: f64, fs: f64) {
        let fc = fc.min(fs * 0.45);
        self.g = (std::f64::consts::PI * fc / fs).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }
    #[inline]
    pub fn tick(&mut self, x: f64) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut {
            lp: v2,
            bp: v1,
            hp: x - self.k * v1 - v2,
        }
    }
    #[inline]
    pub fn lp(&mut self, x: f64) -> f64 {
        self.tick(x).lp
    }
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
}

/// Flush denormals and runaway values (a stuck NaN would latch forever in
/// the feedback loops).
#[inline]
pub fn sane(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}
