//! CD4093 (HEF4093B) Schmitt-trigger NAND relaxation oscillator.
//!
//! One NAND gate with its spare input tied high is an inverting Schmitt
//! trigger. Its output charges the timing capacitor through the TUNE path;
//! when the capacitor reaches the upper threshold VT+ the output drops to
//! 0 V and the capacitor discharges until VT-. The Lyra takes its "triangle"
//! from the capacitor node (SOMA FAQ), so the waveform is a pair of
//! exponential segments whose curvature depends on where VT± sit inside the
//! rail, i.e. on the supply and on the individual chip.
//!
//! Between switching events the node is a linear RC network, so it is
//! integrated **exactly** (closed-form exponential) and every crossing is
//! located with sub-sample precision. Everything else tied to the node
//! (FM injection, vibrato, envelope loading, leakage, dielectric absorption)
//! enters as a Norton equivalent, re-evaluated every internal sample.
//! The square output is band-limited with a polyBLEP residual (one sample of
//! look-ahead latency, applied to the triangle too so both stay aligned).

use super::fexp;
use super::tolerance::GateSpread;

/// HEF4093B transfer thresholds as a function of supply (typical curves of
/// the NXP datasheet: hysteresis widens with VDD, centre sits slightly
/// below VDD/2 because the n-channel devices are stronger).
#[inline]
pub fn thresholds(vdd: f64, s: GateSpread) -> (f64, f64) {
    let center = 0.47 * vdd * s.center;
    let h = (0.25 + 0.125 * vdd) * s.hyst;
    (center + 0.5 * h, center - 0.5 * h)
}

/// Output on-resistance of a 4000-series buffer vs supply (≈ 250 Ω at 12 V,
/// several kΩ when starved).
#[inline]
pub fn r_on(vdd: f64) -> f64 {
    250.0 * (12.0 / vdd.max(2.5)).powf(1.6)
}

/// Everything except the gate output that is connected to the timing node,
/// as a Norton equivalent: conductance `g` to an ideal source and a current
/// `i` (sum of G·V terms plus pure current injections).
#[derive(Clone, Copy, Default)]
pub struct Norton {
    pub g: f64,
    pub i: f64,
}

impl Norton {
    #[inline]
    pub fn add_g(&mut self, g: f64, v: f64) {
        self.g += g;
        self.i += g * v;
    }
    #[inline]
    pub fn add_i(&mut self, i: f64) {
        self.i += i;
    }
}

/// Per-control-period constants of the timing node (see `SchmittOsc::coef`).
#[derive(Clone, Copy, Default)]
pub struct OscCoef {
    /// Gate output → node conductance.
    pub g_out: f64,
    /// Total conductance on the node and its reciprocal.
    pub g: f64,
    pub inv_g: f64,
    /// Decay factor of the node over one full step.
    pub e_full: f64,
}

#[derive(Clone, Copy, Default)]
pub struct OscOut {
    /// Capacitor ("triangle") voltage, volts.
    pub tri: f64,
    /// Band-limited square, ±1 (+1 = gate output high).
    pub sq: f64,
    /// Signed count of output transitions this step (+1 = rising output).
    pub edges: i32,
}

#[derive(Clone)]
pub struct SchmittOsc {
    pub vc: f64,
    pub high: bool,
    pub c: f64,
    /// Electrolytic leakage to ground.
    pub g_leak: f64,
    /// Dielectric absorption: a small capacitor `c_da` behind `g_da`.
    pub c_da: f64,
    pub g_da: f64,
    pub v_da: f64,
    inv_c: f64,
    k_da: f64,
    sq_prev: f64,
    tri_prev: f64,
}

impl SchmittOsc {
    pub fn new(c: f64, electrolytic: bool) -> Self {
        let c_da = c * if electrolytic { 0.03 } else { 0.002 };
        let g_da = 1.0 / 2.0e6;
        Self {
            vc: 0.0,
            high: true,
            c,
            g_leak: if electrolytic { 1.0 / 25e6 } else { 1.0 / 2e9 },
            c_da,
            g_da,
            v_da: 0.0,
            inv_c: 1.0 / c,
            k_da: g_da / c_da,
            sq_prev: 1.0,
            tri_prev: 0.0,
        }
    }

    pub fn reset(&mut self, vc: f64) {
        self.vc = vc;
        self.v_da = vc;
        self.high = true;
        self.sq_prev = 1.0;
        self.tri_prev = vc;
    }

    /// Coefficients that only change when a knob or the envelope moves.
    #[inline]
    pub fn coef(&self, dt: f64, g_out: f64, g_net: f64) -> OscCoef {
        let g = g_out + g_net + self.g_leak + self.g_da;
        OscCoef { g_out, g, inv_g: 1.0 / g, e_full: fexp(-dt * g * self.inv_c) }
    }

    /// Advance by `dt` seconds.
    ///
    /// * `v_hi` – output-high level (the package supply);
    /// * `g_out` – conductance from the gate output to the node (TUNE path);
    /// * `vt_hi`, `vt_lo` – thresholds for this step (noise/crosstalk applied);
    /// * `net` – everything else tied to the node.
    #[inline]
    pub fn step(&mut self, dt: f64, v_hi: f64, g_out: f64, vt_hi: f64, vt_lo: f64, net: Norton) -> OscOut {
        let k = self.coef(dt, g_out, net.g);
        self.step_k(dt, v_hi, &k, vt_hi, vt_lo, net.i)
    }

    /// Same, with the conductances prepared by [`Self::coef`] (for the same
    /// `dt`) and only the injected current `i_net` changing per sample.
    #[inline]
    pub fn step_k(&mut self, dt: f64, v_hi: f64, k: &OscCoef, vt_hi: f64, vt_lo: f64, i_net: f64) -> OscOut {
        let i_rest = i_net + self.g_da * self.v_da;
        let mut t_left = dt;
        let mut edges = 0i32;
        // BLEP residuals: correction for the previous (not yet emitted) and
        // the current sample.
        let mut corr_prev = 0.0;
        let mut corr_now = 0.0;

        let inv_c = self.inv_c;
        for _ in 0..8 {
            let v_out = if self.high { v_hi } else { 0.0 };
            let v_inf = (k.g_out * v_out + i_rest) * k.inv_g;
            let vt = if self.high { vt_hi } else { vt_lo };
            let already_past = if self.high { self.vc >= vt } else { self.vc <= vt };
            // Free run to the end of the step; a crossing is only solved
            // for (one logarithm) when this run would pass the threshold.
            let e = if t_left == dt { k.e_full } else { fexp(-t_left * k.g * inv_c) };
            let v_end = v_inf + (self.vc - v_inf) * e;
            let passes = if self.high { v_end >= vt } else { v_end <= vt };
            if !already_past && !passes {
                self.vc = v_end;
                break;
            }
            let tc = if already_past {
                0.0
            } else {
                let ratio = (self.vc - v_inf) / (vt - v_inf);
                if ratio > 1.0 { (ratio.ln() / (k.g * inv_c)).min(t_left) } else { 0.0 }
            };
            // Move to the crossing, flip the output.
            if tc > 0.0 {
                self.vc = vt;
            }
            t_left -= tc;
            let h = if self.high { -2.0 } else { 2.0 };
            self.high = !self.high;
            edges += if self.high { 1 } else { -1 };
            // Fraction of the step elapsed at the edge.
            let f = ((dt - t_left) / dt).clamp(0.0, 1.0);
            corr_prev += h * (1.0 - f) * (1.0 - f) * 0.5;
            corr_now -= h * f * f * 0.5;
            if t_left <= 0.0 {
                break;
            }
        }

        // Dielectric absorption charge (slow, explicit is fine).
        self.v_da += self.k_da * (self.vc - self.v_da) * dt;

        let sq_now = if self.high { 1.0 } else { -1.0 } + corr_now;
        let out = OscOut {
            tri: self.tri_prev,
            sq: self.sq_prev + corr_prev,
            edges,
        };
        self.sq_prev = sq_now;
        self.tri_prev = self.vc;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(r: f64, c: f64, fs: f64) -> f64 {
        let spread = GateSpread { center: 1.0, hyst: 1.0 };
        let (hi, lo) = thresholds(12.0, spread);
        let mut o = SchmittOsc::new(c, false);
        o.g_leak = 0.0;
        o.g_da = 0.0;
        o.k_da = 0.0;
        o.reset(lo);
        let dt = 1.0 / fs;
        let mut rises = Vec::new();
        let n = (fs * 2.0) as usize;
        for k in 0..n {
            let out = o.step(dt, 12.0, 1.0 / r, hi, lo, Norton::default());
            if out.edges > 0 {
                rises.push(k);
            }
        }
        let periods = rises.len() - 2;
        let span = (rises[rises.len() - 1] - rises[1]) as f64 / fs;
        periods as f64 / span
    }

    #[test]
    fn period_matches_closed_form() {
        let spread = GateSpread { center: 1.0, hyst: 1.0 };
        let (hi, lo) = thresholds(12.0, spread);
        let k = ((12.0 - lo) / (12.0 - hi)).ln() + (hi / lo).ln();
        for &(r, c) in &[(10e3, 0.47e-6), (100e3, 2.2e-6), (1e3, 0.22e-6)] {
            let f_expect = 1.0 / (k * r * c);
            let f = measure(r, c, 192_000.0);
            let err = (f / f_expect - 1.0).abs();
            assert!(err < 0.002, "r={r} c={c} f={f} expect={f_expect}");
        }
    }
}
