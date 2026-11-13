//! One Lyra voice: touch sensor → BC557 charge pump → 2.2 µF envelope
//! capacitor → junction VCA, wrapped around a CD4093 oscillator whose timing
//! node is loaded by that same envelope ("the envelope generators are part
//! of the oscillators and have an impact on their pitch and waveform" —
//! V. Kreimer).
//!
//! Component roles (see docs/CIRCUIT.md for the evidence behind each):
//! * 910 k  – base resistor between the sensitive pad and the BC557
//!   (tweaks sheet: "attack time 300 k – 1 MΩ");
//! * empty slot – base-emitter shunt, 3–10 MΩ lowers sensitivity;
//! * 5.1 k  – collector resistor, sets the fastest attack;
//! * 3 MΩ   – SLOW release ("1 MΩ – infinity");
//! * 330 k  – FAST release, switched in parallel ("220 k – 750 k");
//! * 750 k + 1N4148 – HOLD feed, can only lift the envelope;
//! * 10 V zener – envelope ceiling;
//! * 39 k / 5.1 k – triangle tap into the VCA junction; 47 µF output cap.

use super::schmitt::{Norton, OscCoef, OscOut, SchmittOsc, thresholds};
use super::tolerance::{GateSpread, Unit};
use super::{Rc, Rng, VDD, fexp, sigmoid, softplus};

/// Series resistor between each gate output and its TUNE pot ("IC-x 100Ω").
pub const R_SERIES: f64 = 100.0;
/// Full TUNE pot track.
pub const R_TUNE_POT: f64 = 100e3;
/// Envelope-to-timing-node loading path.
const G_ENV_LOAD: f64 = 1.0 / 3.3e6;
/// Vibrato injection (47 k after the bipolar 10 µF, into a divider).
pub const R_VIB: f64 = 220e3;
const V_ZENER: f64 = 10.0;

/// Control-rate inputs of the oscillator + VCA (knobs, vibrato, supply).
#[derive(Clone, Copy, Default)]
pub struct VoiceInputs {
    /// TUNE pot resistance in the timing path (ohms).
    pub r_tune: f64,
    /// PITCH: group-wide scaling of the charge current.
    pub pitch_k: f64,
    /// Slow current into the timing node (vibrato), amps.
    pub i_slow: f64,
    /// Conductance of the FM pot network towards VBIAS (pitch pull).
    pub g_fm: f64,
    /// SHARP (0..1, linear B pot).
    pub sharp: f64,
    /// Supply seen by this voice's 4093 package.
    pub vdd: f64,
    /// Output on-resistance of the gate at that supply.
    pub r_on: f64,
    /// Thermal drift for this control period: pole and noise gain.
    pub drift_pole: f64,
    pub drift_gain: f64,
}

#[derive(Clone)]
pub struct Voice {
    pub osc: SchmittOsc,
    pub gate: GateSpread,
    pub ve: f64,
    c_env: f64,
    beta: f64,
    r_att: f64,
    g_slow: f64,
    g_fast: f64,
    g_hold: f64,
    r_trim: f64,
    inv_vt_eff: f64,
    hp_out: Rc,
    bleed_hp: Rc,
    /// Slow thermal drift of the timing resistance (log-ratio).
    drift: f64,
    pub out: f64,
    /// Capacitive bleed of the square edges (to the master bus).
    pub bleed: f64,
    /// Bias current of the VCA junction (for rail-sag coupling).
    pub i_vca: f64,
    pub last: OscOut,
    /// VCA junction bias (from the envelope, updated at control rate).
    bias: f64,
    // Control-rate oscillator state (see `prepare`).
    k: OscCoef,
    i_net: f64,
    vt_hi: f64,
    vt_lo: f64,
    center: f64,
    vdd: f64,
    sq_gain: f64,
}

impl Voice {
    pub fn new(idx: usize, unit: &Unit, fs: f64) -> Self {
        let mut osc = SchmittOsc::new(unit.c_osc[idx], idx < 4);
        let (_, lo) = thresholds(VDD, unit.gate[idx]);
        osc.reset(lo);
        Self {
            osc,
            gate: unit.gate[idx],
            ve: 0.0,
            c_env: unit.c_env[idx],
            beta: unit.beta[idx],
            r_att: unit.r_att[idx],
            g_slow: 1.0 / unit.r_slow[idx],
            g_fast: 1.0 / unit.r_fast[idx],
            g_hold: 1.0 / unit.r_hold[idx],
            r_trim: unit.r_osc[idx],
            inv_vt_eff: 1.0 / (0.11 * unit.vca_vt[idx]),
            hp_out: Rc::new(6.0, fs),
            bleed_hp: Rc::new(6_000.0, fs),
            drift: 0.0,
            out: 0.0,
            bleed: 0.0,
            i_vca: 0.0,
            last: OscOut::default(),
            bias: 0.0,
            k: OscCoef::default(),
            i_net: 0.0,
            vt_hi: 0.0,
            vt_lo: 0.0,
            center: 0.0,
            vdd: VDD,
            sq_gain: 0.0,
        }
    }

    /// Panic: discharge the envelope and quiet the output stage. The
    /// oscillator keeps running, as it does on the hardware.
    pub fn silence(&mut self) {
        self.ve = 0.0;
        self.bias = 0.0;
        self.i_vca = 0.0;
        self.out = 0.0;
        self.bleed = 0.0;
        self.hp_out.reset(0.0);
        self.bleed_hp.reset(0.0);
    }

    pub fn set_fs(&mut self, fs: f64) {
        self.hp_out.set(6.0, fs);
        self.bleed_hp.set(6_000.0, fs);
    }

    /// Envelope capacitor: BC557 charge pump against the release resistors,
    /// HOLD diode feed and the zener ceiling. Semi-implicit (linearised
    /// conductances, implicit Euler) so the stiff zener knee stays stable.
    ///
    /// Runs at the host rate (the fastest attack is ≈ 10 ms).
    /// * `g_skin` – skin conductance across the pads, siemens;
    /// * `v_hold` – HOLD voltage of the group (after HOLD GATE scaling).
    #[inline]
    pub fn envelope(&mut self, g_skin: f64, fast: bool, v_hold: f64, dt: f64) {
        // Base current: pad (lower contact at 0 V) → skin → 910 k → base.
        let ib = if g_skin > 0.0 {
            let r = 1.0 / g_skin + self.r_att;
            ((VDD - 0.65) / r).max(0.0)
        } else {
            0.0
        };
        let ic_lin = self.beta * ib;
        // Collector through 5.1 k: saturates as the cap approaches the rail.
        let ic_sat = ((VDD - 0.15 - self.ve) * (1.0 / 5.1e3)).max(0.0);
        let ic = if ic_lin + ic_sat > 0.0 { ic_lin * ic_sat / (ic_lin + ic_sat) } else { 0.0 };

        // HOLD through 1N4148 + 750 k: one-way, soft diode knee.
        let vh = v_hold - 0.6;
        let dv = vh - self.ve;
        let i_hold = if dv > 0.0 { dv * self.g_hold * sigmoid(dv * 25.0) } else { 0.0 };

        let g_rel = self.g_slow + if fast { self.g_fast } else { 0.0 } + 1.0 / 20e6;
        // Zener: sinks current only above its knee (≈ 40 Ω dynamic R).
        let g_z = if self.ve > V_ZENER { 1.0 / 40.0 } else { 0.0 };
        let cdt = self.c_env / dt;
        self.ve = (cdt * self.ve + ic + i_hold + g_z * V_ZENER) / (cdt + g_rel + g_z);
        // Junction bias current: forward-biased knee around 1.6 V of envelope.
        self.bias = (0.25 / 8.4) * softplus((self.ve - 1.6) * 4.0);
    }

    /// Control-rate part of the oscillator: everything on the timing node
    /// that only moves with knobs, the envelope and the slow oscillators.
    #[inline]
    pub fn prepare(&mut self, inp: &VoiceInputs, dt: f64, rng: &mut Rng) {
        // Thermal drift of the TUNE network (±~0.1 %).
        self.drift = self.drift * inp.drift_pole + rng.gauss() * inp.drift_gain;

        let (vt_hi, vt_lo) = thresholds(inp.vdd, self.gate);
        self.vt_hi = vt_hi;
        self.vt_lo = vt_lo;
        self.center = 0.5 * (vt_hi + vt_lo);
        self.vdd = inp.vdd;
        self.sq_gain = 0.034 * inp.sharp * 0.5 * inp.vdd;
        let r_path = (inp.r_tune * self.r_trim + R_SERIES + inp.r_on) * (1.0 + self.drift);
        let g_out = inp.pitch_k / r_path;

        let mut net = Norton::default();
        // The VCA junction loads the node; the envelope lifts the far end of
        // that load, so opening the envelope raises the pitch a little and
        // tilts the triangle.
        let v_load = 0.65 + 0.5 * self.ve;
        net.add_g(G_ENV_LOAD, v_load);
        // FM pot network: a pull towards the virtual ground (the injected
        // modulation current arrives per sample).
        net.add_g(inp.g_fm, super::VBIAS);
        net.add_i(inp.i_slow);
        self.k = self.osc.coef(dt, g_out, net.g);
        self.i_net = net.i;
    }

    /// Advance one internal sample (after [`Self::prepare`] with the same
    /// `dt`). `i_fm`: FM / LFO / CV current into the timing node, amps;
    /// `vt_offset`: threshold disturbance (ground bounce + noise), volts.
    /// Returns the VCA output voltage (AC).
    #[inline]
    pub fn step(&mut self, i_fm: f64, vt_offset: f64, dt: f64) -> f64 {
        let o = self.osc.step_k(dt, self.vdd, &self.k, self.vt_hi + vt_offset, self.vt_lo + vt_offset, self.i_net + i_fm);
        self.last = o;

        // ---- Junction VCA ------------------------------------------------
        // Triangle via 39 k into 5.1 k (×0.116) plus the square through the
        // SHARP pot; the envelope sets the junction bias current.
        let tri_ac = o.tri - self.center;
        let v_node = 0.1156 * tri_ac + self.sq_gain * o.sq;
        let x = (v_node * self.inv_vt_eff).clamp(-30.0, 30.0);
        let i = self.bias * fexp(x);
        let i_sat = i / (1.0 + i / 4.0);
        self.i_vca = i_sat;
        // Capacitive feed-through of the node (always-open path).
        let feed = 0.0015 * v_node;
        let v = 1.0 * i_sat + feed;
        self.out = self.hp_out.hp(v);

        // Square-edge bleed into the master bus: a differentiated square,
        // louder for higher tunings, independent of the envelope.
        self.bleed = self.bleed_hp.hp(o.sq);
        self.out
    }
}
