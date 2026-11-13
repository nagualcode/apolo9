//! DISTORTION and output stage (bottom-right TL074 on the main board).
//!
//! DRIVE is a dual-gang pot (DRV1/DRV2) setting the gain of two cascaded
//! non-inverting TL074 stages. The clipping element is the op-amp itself:
//! a TL074 on a single 12 V rail clips harder and earlier on the negative
//! side, which makes the fuzz asymmetric (even harmonics). The 510 pF across
//! the first stage's 91 k feedback resistor (tweaks sheet: "distortion
//! brightness 0-0.01 µF") rolls off the gained-up part above ≈ 3.4 kHz.
//! The TL07x also has its famous phase reversal when an input is driven
//! below its common-mode range; with the drive fully up and a hot input that
//! flips the output to the positive rail for a moment - kept here.
//!
//! The output stage after the dry/dist MIX lifts the lows a little and
//! softens the highs (manual: tuned for live PA work), then VOL and a last
//! op-amp buffer.

use super::{Rc, Svf, opamp_rail, taper_a, OPAMP_LO};

pub struct Distortion {
    lp1: Rc,
    lp2: Rc,
    post: Rc,
    dc: Rc,
    low: Svf,
    high: Svf,
    hp: Rc,
    /// Knob-dependent gains, recomputed only when a knob moves.
    key: (f64, f64),
    g1: f64,
    g2: f64,
    g_vol: f64,
}

impl Distortion {
    pub fn new(fs: f64) -> Self {
        let mut d = Self {
            lp1: Rc::default(),
            lp2: Rc::default(),
            post: Rc::default(),
            dc: Rc::default(),
            low: Svf::default(),
            high: Svf::default(),
            hp: Rc::default(),
            key: (f64::NAN, f64::NAN),
            g1: 0.0,
            g2: 0.0,
            g_vol: 0.0,
        };
        d.set_fs(fs);
        d
    }

    pub fn set_fs(&mut self, fs: f64) {
        // 91 k ‖ 510 pF.
        self.lp1.set(1.0 / (std::f64::consts::TAU * 91e3 * 510e-12), fs);
        self.lp2.set(7_200.0, fs);
        // 15 k into 510 pF at the output of the clipper.
        self.post.set(1.0 / (std::f64::consts::TAU * 15e3 * 510e-12), fs);
        self.dc.set(12.0, fs);
        self.low.set(140.0, 0.6, fs);
        self.high.set(6_500.0, 0.6, fs);
        self.hp.set(16.0, fs);
    }

    /// Panic: every filter state back to rest.
    pub fn reset(&mut self) {
        for r in [&mut self.lp1, &mut self.lp2, &mut self.post, &mut self.dc, &mut self.hp] {
            r.reset(0.0);
        }
        self.low.reset();
        self.high.reset();
    }

    /// `x` volts in; `drive`, `mix`, `vol` are raw knob positions 0..1.
    /// Returns (output volts, total-feedback tap volts).
    #[inline]
    pub fn process(&mut self, x: f64, drive: f64, mix: f64, vol: f64) -> (f64, f64) {
        if self.key != (drive, vol) {
            self.key = (drive, vol);
            // Stage 1: gain 1 + 91k / (1k + 100k·A(1-d)), cap across the 91 k.
            self.g1 = 91e3 / (1e3 + 100e3 * taper_a(1.0 - drive));
            // Stage 2: gain 1 + 30k / (10k + 90k·(1-d)).
            self.g2 = 30e3 / (10e3 + 90e3 * (1.0 - drive));
            self.g_vol = taper_a(vol) * 4.0;
        }
        let (g1, g2) = (self.g1, self.g2);
        let mut s1 = x + g1 * self.lp1.lp(x);
        if x < OPAMP_LO * 1.08 && drive > 0.85 {
            // TL07x phase reversal (input below common-mode range).
            s1 = 10.0;
        }
        let s1 = opamp_rail(s1);
        let s2 = opamp_rail(s1 + g2 * self.lp2.lp(s1));
        // Back-to-back 10 V zeners never conduct inside the op-amp swing;
        // what is left is the RC and the coupling cap.
        let d = self.dc.hp(self.post.lp(s2)) * 0.42;

        let m = mix.clamp(0.0, 1.0);
        let mixed = x * (1.0 - m) + d * m;

        // Output EQ: +3.5 dB low shelf, -3 dB high shelf, subsonic HP.
        let lo = self.low.tick(mixed);
        let hi = self.high.tick(mixed);
        let eq = mixed + 0.5 * lo.lp - 0.3 * hi.hp;
        let eq = self.hp.hp(eq);
        let tap = eq;
        // Last buffer, then the divider that sets the 2 V 0-to-peak spec
        // (the output never exceeds ≈ 1.9 V however hard it is driven).
        let out = opamp_rail(eq * self.g_vol) * 0.40;
        (out, tap)
    }
}
