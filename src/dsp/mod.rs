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

/// 2^(j/128), j = 0..127.
static EXP_TAB: [u64; 128] = [
    0x3ff0000000000000, 0x3ff0163da9fb3335, 0x3ff02c9a3e778061, 0x3ff04315e86e7f85,
    0x3ff059b0d3158574, 0x3ff0706b29ddf6de, 0x3ff0874518759bc8, 0x3ff09e3ecac6f383,
    0x3ff0b5586cf9890f, 0x3ff0cc922b7247f7, 0x3ff0e3ec32d3d1a2, 0x3ff0fb66affed31b,
    0x3ff11301d0125b51, 0x3ff12abdc06c31cc, 0x3ff1429aaea92de0, 0x3ff15a98c8a58e51,
    0x3ff172b83c7d517b, 0x3ff18af9388c8dea, 0x3ff1a35beb6fcb75, 0x3ff1bbe084045cd4,
    0x3ff1d4873168b9aa, 0x3ff1ed5022fcd91d, 0x3ff2063b88628cd6, 0x3ff21f49917ddc96,
    0x3ff2387a6e756238, 0x3ff251ce4fb2a63f, 0x3ff26b4565e27cdd, 0x3ff284dfe1f56381,
    0x3ff29e9df51fdee1, 0x3ff2b87fd0dad990, 0x3ff2d285a6e4030b, 0x3ff2ecafa93e2f56,
    0x3ff306fe0a31b715, 0x3ff32170fc4cd831, 0x3ff33c08b26416ff, 0x3ff356c55f929ff1,
    0x3ff371a7373aa9cb, 0x3ff38cae6d05d866, 0x3ff3a7db34e59ff7, 0x3ff3c32dc313a8e5,
    0x3ff3dea64c123422, 0x3ff3fa4504ac801c, 0x3ff4160a21f72e2a, 0x3ff431f5d950a897,
    0x3ff44e086061892d, 0x3ff46a41ed1d0057, 0x3ff486a2b5c13cd0, 0x3ff4a32af0d7d3de,
    0x3ff4bfdad5362a27, 0x3ff4dcb299fddd0d, 0x3ff4f9b2769d2ca7, 0x3ff516daa2cf6642,
    0x3ff5342b569d4f82, 0x3ff551a4ca5d920f, 0x3ff56f4736b527da, 0x3ff58d12d497c7fd,
    0x3ff5ab07dd485429, 0x3ff5c9268a5946b7, 0x3ff5e76f15ad2148, 0x3ff605e1b976dc09,
    0x3ff6247eb03a5585, 0x3ff6434634ccc320, 0x3ff6623882552225, 0x3ff68155d44ca973,
    0x3ff6a09e667f3bcd, 0x3ff6c012750bdabf, 0x3ff6dfb23c651a2f, 0x3ff6ff7df9519484,
    0x3ff71f75e8ec5f74, 0x3ff73f9a48a58174, 0x3ff75feb564267c9, 0x3ff780694fde5d3f,
    0x3ff7a11473eb0187, 0x3ff7c1ed0130c132, 0x3ff7e2f336cf4e62, 0x3ff80427543e1a12,
    0x3ff82589994cce13, 0x3ff8471a4623c7ad, 0x3ff868d99b4492ed, 0x3ff88ac7d98a6699,
    0x3ff8ace5422aa0db, 0x3ff8cf3216b5448c, 0x3ff8f1ae99157736, 0x3ff9145b0b91ffc6,
    0x3ff93737b0cdc5e5, 0x3ff95a44cbc8520f, 0x3ff97d829fde4e50, 0x3ff9a0f170ca07ba,
    0x3ff9c49182a3f090, 0x3ff9e86319e32323, 0x3ffa0c667b5de565, 0x3ffa309bec4a2d33,
    0x3ffa5503b23e255d, 0x3ffa799e1330b358, 0x3ffa9e6b5579fdbf, 0x3ffac36bbfd3f37a,
    0x3ffae89f995ad3ad, 0x3ffb0e07298db666, 0x3ffb33a2b84f15fb, 0x3ffb59728de5593a,
    0x3ffb7f76f2fb5e47, 0x3ffba5b030a1064a, 0x3ffbcc1e904bc1d2, 0x3ffbf2c25bd71e09,
    0x3ffc199bdd85529c, 0x3ffc40ab5fffd07a, 0x3ffc67f12e57d14b, 0x3ffc8f6d9406e7b5,
    0x3ffcb720dcef9069, 0x3ffcdf0b555dc3fa, 0x3ffd072d4a07897c, 0x3ffd2f87080d89f2,
    0x3ffd5818dcfba487, 0x3ffd80e316c98398, 0x3ffda9e603db3285, 0x3ffdd321f301b460,
    0x3ffdfc97337b9b5f, 0x3ffe264614f5a129, 0x3ffe502ee78b3ff6, 0x3ffe7a51fbc74c83,
    0x3ffea4afa2a490da, 0x3ffecf482d8e67f1, 0x3ffefa1bee615a27, 0x3fff252b376bba97,
    0x3fff50765b6e4540, 0x3fff7bfdad9cbe14, 0x3fffa7c1819e90d8, 0x3fffd3c22b8f71f1,
];

/// `e^x` to within about 1 ulp, without the libm call: table of 128
/// binary fractions plus a degree-5 polynomial on the tiny remainder
/// (|r| < 0.0028, truncation error < 1e-18). The oscillators and VCAs call
/// this millions of times a second.
#[inline]
pub fn fexp(x: f64) -> f64 {
    if !(x > -700.0 && x < 700.0) {
        return x.exp();
    }
    const INV: f64 = 128.0 / std::f64::consts::LN_2;
    const C_HI: f64 = 0.693_147_180_369_123_816_49 / 128.0;
    const C_LO: f64 = 1.908_214_929_270_587_700_02e-10 / 128.0;
    // Round to nearest without calling `round` (libm on baseline x86-64).
    const MAGIC: f64 = 6_755_399_441_055_744.0;
    let kf = (x * INV + MAGIC) - MAGIC;
    let k = kf as i64;
    let r = (x - kf * C_HI) - kf * C_LO;
    let p = 1.0 + r * (1.0 + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0)))));
    let t = f64::from_bits(EXP_TAB[(k & 127) as usize]);
    let scale = f64::from_bits((((k >> 7) + 1023) as u64) << 52);
    t * p * scale
}

/// Logistic, used to smooth junction "switch-on" behaviour.
#[inline]
pub fn sigmoid(x: f64) -> f64 {
    if x > 30.0 {
        1.0
    } else if x < -30.0 {
        0.0
    } else {
        1.0 / (1.0 + fexp(-x))
    }
}

/// `ln(1 + e^x)`: the forward-biased junction current curve.
#[inline]
pub fn softplus(x: f64) -> f64 {
    if x > 30.0 {
        x
    } else if x.abs() > 6.0 {
        // ln(1 + e) as a series: e < 0.0025, error below 1e-15 relative.
        let e = fexp(-x.abs());
        let l = e * (1.0 - e * (0.5 - e * (1.0 / 3.0 - e * (0.25 - e * (0.2 - e / 6.0)))));
        if x > 0.0 { x + l } else { l }
    } else {
        fexp(x).ln_1p()
    }
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
    /// Approximately Gaussian (Irwin-Hall of 4), unit variance. The four
    /// uniforms are the 16-bit fields of a single draw.
    #[inline]
    pub fn gauss(&mut self) -> f64 {
        let r = self.next_u64();
        let sum = (r & 0xFFFF) + ((r >> 16) & 0xFFFF) + ((r >> 32) & 0xFFFF) + (r >> 48);
        (sum as i64 - 131_070) as f64 * (1.732_050_8 / 65_536.0)
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

/// Flush-to-zero / denormals-are-zero for the audio thread, restored on
/// drop. Decaying filters, envelopes and knob smoothers all end up in the
/// subnormal range; on x86 every operation on such a value traps to
/// microcode (a hundred times slower), which shows up as CPU load creeping
/// in after the instrument has been sitting for a while.
pub struct NoDenormals(u64);

impl NoDenormals {
    #[inline]
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let mut old: u32 = 0;
            // SAFETY: MXCSR read/write; SSE is baseline on x86-64.
            unsafe {
                core::arch::asm!("stmxcsr [{p}]", p = in(reg) &mut old, options(nostack, preserves_flags));
                let new = old | 0x8040; // FTZ | DAZ
                core::arch::asm!("ldmxcsr [{p}]", p = in(reg) &new, options(nostack, preserves_flags, readonly));
            }
            Self(u64::from(old))
        }
        #[cfg(target_arch = "aarch64")]
        {
            let old: u64;
            // SAFETY: FPCR is a user-accessible register; bit 24 is FZ.
            unsafe {
                core::arch::asm!("mrs {o}, fpcr", o = out(reg) old, options(nomem, nostack, preserves_flags));
                core::arch::asm!("msr fpcr, {n}", n = in(reg) old | (1 << 24), options(nomem, nostack, preserves_flags));
            }
            Self(old)
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self(0)
        }
    }
}

impl Default for NoDenormals {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NoDenormals {
    #[inline]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            let old = self.0 as u32;
            // SAFETY: restores the MXCSR value read in `new`.
            unsafe {
                core::arch::asm!("ldmxcsr [{p}]", p = in(reg) &old, options(nostack, preserves_flags, readonly));
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: restores the FPCR value read in `new`.
            unsafe {
                core::arch::asm!("msr fpcr, {n}", n = in(reg) self.0, options(nomem, nostack, preserves_flags));
            }
        }
    }
}

/// Flush denormals and runaway values (a stuck NaN would latch forever in
/// the feedback loops).
#[inline]
pub fn sane(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}
