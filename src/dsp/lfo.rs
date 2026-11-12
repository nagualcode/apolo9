//! Low-frequency oscillators, all built from the same CD4093 core.
//!
//! * **Vibrato bank** – two DIP 4093s, one gate per voice, 10 µF caps and
//!   30 k…56 k resistors (≈ 2.6–5 Hz, every voice different). The VIBRATO
//!   switch does not mute them: it switches their +12 V (confirmed by
//!   builders probing pin 14), so turning it on boots eight oscillators from
//!   discharged caps and turning it off lets their supply collapse.
//! * **Hyper LFO** – the "4093-BEAT" chip: two oscillators A and B on 47 µF,
//!   a LINK path (A's square into B's timing node via 910 k), a NAND/AND
//!   logic gate and a resistive OR summer with a staircase output. The delay
//!   modulation has its own "triangle" made by summing both capacitor
//!   voltages.

use super::schmitt::{Norton, SchmittOsc, r_on, thresholds};
use super::tolerance::{GateSpread, Unit};
use super::{Rc, VDD};

pub struct VibratoBank {
    osc: [SchmittOsc; 8],
    gate: [GateSpread; 8],
    r: [f64; 8],
    /// Switched supply of the two vibrato packages (100 µF bypass).
    vdd: Rc,
    coupling: [Rc; 8],
    pub out: [f64; 8],
}

impl VibratoBank {
    pub fn new(unit: &Unit, fs: f64) -> Self {
        let mut vdd = Rc::default();
        vdd.set_tau(0.03, fs);
        let mut coupling: [Rc; 8] = std::array::from_fn(|_| Rc::default());
        for c in &mut coupling {
            // Bipolar 10 µF into 47 k: τ ≈ 0.47 s.
            c.set_tau(0.47, fs);
        }
        Self {
            osc: std::array::from_fn(|i| SchmittOsc::new(unit.vib_c[i], true)),
            gate: unit.vib_gate,
            r: unit.vib_r,
            vdd,
            coupling,
            out: [0.0; 8],
        }
    }

    pub fn set_fs(&mut self, fs: f64) {
        self.vdd.set_tau(0.03, fs);
        for c in &mut self.coupling {
            c.set_tau(0.47, fs);
        }
    }

    #[inline]
    pub fn step(&mut self, on: bool, dt: f64) {
        let vdd = self.vdd.lp(if on { VDD } else { 0.0 });
        let ron = r_on(vdd);
        for i in 0..8 {
            let tri = if vdd > 2.5 {
                let (hi, lo) = thresholds(vdd, self.gate[i]);
                let g_out = 1.0 / (self.r[i] + ron);
                self.osc[i].step(dt, vdd, g_out, hi, lo, Norton::default()).tri
            } else {
                // CMOS below its operating voltage: the gate output floats,
                // the cap just leaks down.
                let o = &mut self.osc[i];
                o.vc *= 1.0 - dt / (o.c * 25e6);
                o.vc
            };
            self.out[i] = self.coupling[i].hp(tri);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LfoCombine {
    Or,
    And,
}

pub struct HyperLfo {
    a: SchmittOsc,
    b: SchmittOsc,
    gate: [GateSpread; 2],
    smooth: Rc,
    ac: Rc,
    /// Bipolar CV for the voices (volts around the virtual ground).
    pub cv: f64,
    /// Delay modulation, triangle mode (sum of both capacitor voltages).
    pub tri: f64,
    /// Delay modulation, square mode (the AND output).
    pub square: f64,
    pub a_high: bool,
    pub b_high: bool,
    centers: [f64; 2],
}

impl HyperLfo {
    pub fn new(unit: &Unit, fs: f64) -> Self {
        let mut a = SchmittOsc::new(unit.lfo_c[0], true);
        let mut b = SchmittOsc::new(unit.lfo_c[1], true);
        let (ha, la) = thresholds(VDD, unit.lfo_gate[0]);
        let (hb, lb) = thresholds(VDD, unit.lfo_gate[1]);
        a.reset(la);
        b.reset(0.5 * (hb + lb));
        let mut smooth = Rc::default();
        smooth.set_tau(1.0e-3, fs);
        let mut ac = Rc::default();
        ac.set_tau(4.7, fs);
        Self {
            a,
            b,
            gate: unit.lfo_gate,
            smooth,
            ac,
            cv: 0.0,
            tri: 0.0,
            square: 0.0,
            a_high: true,
            b_high: true,
            centers: [0.5 * (ha + la), 0.5 * (hb + lb)],
        }
    }

    pub fn set_fs(&mut self, fs: f64) {
        self.smooth.set_tau(1.0e-3, fs);
        self.ac.set_tau(4.7, fs);
    }

    /// `ra`, `rb`: resistance of each FREQ path (1 k + pot).
    #[inline]
    pub fn step(&mut self, ra: f64, rb: f64, link: bool, mode: LfoCombine, dt: f64) {
        let (ha, la) = thresholds(VDD, self.gate[0]);
        let (hb, lb) = thresholds(VDD, self.gate[1]);
        let ron = r_on(VDD);
        let oa = self.a.step(dt, VDD, 1.0 / (ra + ron), ha, la, Norton::default());
        let mut nb = Norton::default();
        if link {
            // A's output through 910 k into B's timing node: soft FM.
            let va = if self.a.high { VDD } else { 0.0 };
            nb.add_g(1.0 / 910e3, va);
        }
        let ob = self.b.step(dt, VDD, 1.0 / (rb + ron), hb, lb, nb);
        self.a_high = oa.sq > 0.0;
        self.b_high = ob.sq > 0.0;

        let va = if self.a_high { VDD } else { 0.0 };
        let vb = if self.b_high { VDD } else { 0.0 };
        let and = if self.a_high && self.b_high { VDD } else { 0.0 };
        let raw = match mode {
            // Resistive sum of the two squares: three-level staircase.
            LfoCombine::Or => 0.5 * (va + vb),
            LfoCombine::And => and,
        };
        // Small RC on the output: the edges keep their click (the one noise
        // Vlad says he doesn't like but couldn't remove cheaply).
        let lvl = self.smooth.lp(raw);
        self.cv = self.ac.hp(lvl - super::VBIAS);
        self.tri = (oa.tri - self.centers[0]) + (ob.tri - self.centers[1]);
        self.square = (and - 0.5 * VDD) * 0.3;
    }
}
