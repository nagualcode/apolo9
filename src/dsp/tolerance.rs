//! Component spread of one physical unit.
//!
//! No two Lyras sound the same: carbon resistors are ±5 %, the electrolytic
//! timing caps of voices 1–4 are ±20 %, transistor β varies 2:1 and the
//! CD4093 thresholds differ between production batches (SOMA's FAQ: "a
//! small batch of ICs gives a wide pitch range and deeper, brighter FM").
//! A `serial` number seeds every value, so a unit is reproducible.

use super::Rng;

#[derive(Clone, Copy)]
pub struct GateSpread {
    /// Multiplier on the threshold centre (≈ 0.47·VDD nominal).
    pub center: f64,
    /// Multiplier on the hysteresis width.
    pub hyst: f64,
}

#[derive(Clone)]
pub struct Unit {
    /// Timing capacitors, farads (voices 1..8).
    pub c_osc: [f64; 8],
    /// Per-voice resistor multipliers for the timing path.
    pub r_osc: [f64; 8],
    pub gate: [GateSpread; 8],
    /// Sensor/envelope transistor (BC557) current gain.
    pub beta: [f64; 8],
    pub c_env: [f64; 8],
    pub r_att: [f64; 8],
    pub r_slow: [f64; 8],
    pub r_fast: [f64; 8],
    pub r_hold: [f64; 8],
    /// Humidity leak resistance multiplier per sensor.
    pub leak: [f64; 8],
    /// Vibrato oscillators: R and C.
    pub vib_r: [f64; 8],
    pub vib_c: [f64; 8],
    pub vib_gate: [GateSpread; 8],
    /// Hyper LFO oscillators (A, B).
    pub lfo_c: [f64; 2],
    pub lfo_gate: [GateSpread; 2],
    /// PT2399 VCO constant per chip.
    pub pt_k: [f64; 2],
    /// Junction (VCA) temperature-voltage spread.
    pub vca_vt: [f64; 8],
    pub fm_r: [f64; 8],
}

/// Nominal timing caps from the v2.6 board (positions 1–8 on the tweaks
/// sheet): 2.2 µF electrolytic, 1 µF electrolytic, 0.47 µF and 0.22 µF film.
pub const C_OSC_NOMINAL: [f64; 8] = [2.2e-6, 2.2e-6, 1.0e-6, 1.0e-6, 0.47e-6, 0.47e-6, 0.22e-6, 0.22e-6];
/// Vibrato oscillator resistors (one CD4093 gate per voice, 10 µF caps).
pub const VIB_R_NOMINAL: [f64; 8] = [56e3, 51e3, 47e3, 43e3, 39e3, 36e3, 33e3, 30e3];

impl Unit {
    pub fn new(serial: u32) -> Self {
        let mut rng = Rng::new(0x4C59_5241_0000_0000 ^ u64::from(serial));
        let mut tol = |pct: f64| 1.0 + rng.bipolar() * pct;
        let c_osc = std::array::from_fn(|i| {
            let electrolytic = i < 4;
            C_OSC_NOMINAL[i] * if electrolytic { tol(0.20) } else { tol(0.05) }
        });
        let r_osc = std::array::from_fn(|_| tol(0.05));
        // Two SOIC 4093 packages: voices 1-4 and 5-8 share a die.
        let pkg = [(tol(0.03), tol(0.10)), (tol(0.03), tol(0.10))];
        let gate = std::array::from_fn(|i| {
            let (c, h) = pkg[i / 4];
            GateSpread {
                center: c * tol(0.006),
                hyst: h * tol(0.03),
            }
        });
        let beta = std::array::from_fn(|_| 280.0 * tol(0.35));
        let c_env = std::array::from_fn(|_| 2.2e-6 * tol(0.20));
        let r_att = std::array::from_fn(|_| 910e3 * tol(0.05));
        let r_slow = std::array::from_fn(|_| 3.0e6 * tol(0.05));
        let r_fast = std::array::from_fn(|_| 330e3 * tol(0.05));
        let r_hold = std::array::from_fn(|_| 750e3 * tol(0.05));
        let leak = std::array::from_fn(|_| tol(0.6));
        let vib_r = std::array::from_fn(|i| VIB_R_NOMINAL[i] * tol(0.05));
        let vib_c = std::array::from_fn(|_| 10e-6 * tol(0.20));
        let vpkg = [(tol(0.03), tol(0.10)), (tol(0.03), tol(0.10))];
        let vib_gate = std::array::from_fn(|i| {
            let (c, h) = vpkg[i / 4];
            GateSpread {
                center: c * tol(0.006),
                hyst: h * tol(0.03),
            }
        });
        let lfo_c = [47e-6 * tol(0.2), 47e-6 * tol(0.2)];
        let (lc, lh) = (tol(0.03), tol(0.10));
        let lfo_gate = [
            GateSpread { center: lc * tol(0.006), hyst: lh * tol(0.03) },
            GateSpread { center: lc * tol(0.006), hyst: lh * tol(0.03) },
        ];
        let pt_k = [tol(0.08), tol(0.08)];
        let vca_vt = std::array::from_fn(|_| tol(0.08));
        let fm_r = std::array::from_fn(|_| tol(0.05));
        Self {
            c_osc,
            r_osc,
            gate,
            beta,
            c_env,
            r_att,
            r_slow,
            r_fast,
            r_hold,
            leak,
            vib_r,
            vib_c,
            vib_gate,
            lfo_c,
            lfo_gate,
            pt_k,
            vca_vt,
            fm_r,
        }
    }
}
