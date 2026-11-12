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

use super::schmitt::{Norton, OscOut, SchmittOsc, thresholds};
use super::tolerance::{GateSpread, Unit};
use super::{Rc, Rng, VDD, sigmoid, softplus};

/// Series resistor between each gate output and its TUNE pot ("IC-x 100Ω").
pub const R_SERIES: f64 = 100.0;
/// Full TUNE pot track.
pub const R_TUNE_POT: f64 = 100e3;
/// Envelope-to-timing-node loading path.
const R_ENV_LOAD: f64 = 3.3e6;
/// Vibrato injection (47 k after the bipolar 10 µF, into a divider).
const R_VIB: f64 = 220e3;
const V_ZENER: f64 = 10.0;

/// Audio-rate inputs of the oscillator + VCA.
#[derive(Clone, Copy, Default)]
pub struct VoiceInputs {
    /// TUNE pot resistance in the timing path (ohms).
    pub r_tune: f64,
    /// PITCH: group-wide scaling of the charge current.
    pub pitch_k: f64,
    /// Pure current injected by FM / LFO / CV, amps.
    pub i_fm: f64,
    /// Conductance of the FM pot network towards VBIAS (pitch pull).
    pub g_fm: f64,
    /// Vibrato oscillator AC voltage.
    pub v_vib: f64,
    pub vib_r_scale: f64,
    /// SHARP (0..1, linear B pot).
    pub sharp: f64,
    /// Supply seen by this voice's 4093 package (rail sag included).
    pub vdd: f64,
    /// Output on-resistance of the gate at that supply.
    pub r_on: f64,
    /// Threshold disturbance (ground bounce + noise), volts.
    pub vt_offset: f64,
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
    r_hold: f64,
    r_trim: f64,
    vt_eff: f64,
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
            r_hold: unit.r_hold[idx],
            r_trim: unit.r_osc[idx],
            vt_eff: 0.11 * unit.vca_vt[idx],
            hp_out: Rc::new(6.0, fs),
            bleed_hp: Rc::new(6_000.0, fs),
            drift: 0.0,
            out: 0.0,
            bleed: 0.0,
            i_vca: 0.0,
            last: OscOut::default(),
            bias: 0.0,
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
        let ic_sat = ((VDD - 0.15 - self.ve) / 5.1e3).max(0.0);
        let ic = if ic_lin + ic_sat > 0.0 { ic_lin * ic_sat / (ic_lin + ic_sat) } else { 0.0 };

        // HOLD through 1N4148 + 750 k: one-way, soft diode knee.
        let vh = v_hold - 0.6;
        let i_hold = (vh - self.ve) / self.r_hold * sigmoid((vh - self.ve) / 0.04);

        let g_rel = self.g_slow + if fast { self.g_fast } else { 0.0 } + 1.0 / 20e6;
        // Zener: sinks current only above its knee (≈ 40 Ω dynamic R).
        let g_z = if self.ve > V_ZENER { 1.0 / 40.0 } else { 0.0 };
        let cdt = self.c_env / dt;
        self.ve = (cdt * self.ve + ic + i_hold.max(0.0) + g_z * V_ZENER) / (cdt + g_rel + g_z);
        // Junction bias current: forward-biased knee around 1.6 V of envelope.
        self.bias = 0.25 * softplus((self.ve - 1.6) / 0.25) / 8.4;
    }

    /// Advance one internal sample. Returns the VCA output voltage (AC).
    #[inline]
    pub fn step(&mut self, inp: &VoiceInputs, dt: f64, rng: &mut Rng, noise: f64) -> f64 {
        // Thermal drift of the TUNE network (±~0.1 %).
        self.drift = self.drift * 0.999_995 + rng.gauss() * 2.0e-6 * noise;

        let (vt_hi, vt_lo) = thresholds(inp.vdd, self.gate);
        let center = 0.5 * (vt_hi + vt_lo);
        let r_path = (inp.r_tune * self.r_trim + R_SERIES + inp.r_on) * (1.0 + self.drift);
        let g_out = inp.pitch_k / r_path;

        let mut net = Norton::default();
        // The VCA junction loads the node; the envelope lifts the far end of
        // that load, so opening the envelope raises the pitch a little and
        // tilts the triangle.
        let v_load = 0.65 + 0.5 * self.ve;
        net.add_g(1.0 / R_ENV_LOAD, v_load);
        // FM pot network: a pull towards the virtual ground plus the
        // injected modulation current.
        net.add_g(inp.g_fm, super::VBIAS);
        net.add_i(inp.i_fm);
        // Vibrato: AC-coupled, practically a current source.
        net.add_i(inp.v_vib / (R_VIB * inp.vib_r_scale));

        let o = self.osc.step(dt, inp.vdd, g_out, vt_hi + inp.vt_offset, vt_lo + inp.vt_offset, net);
        self.last = o;

        // ---- Junction VCA ------------------------------------------------
        // Triangle via 39 k into 5.1 k (×0.116) plus the square through the
        // SHARP pot; the envelope sets the junction bias current.
        let tri_ac = o.tri - center;
        let sq_v = o.sq * 0.5 * inp.vdd;
        let v_node = 0.1156 * tri_ac + 0.034 * inp.sharp * sq_v;
        let x = (v_node / self.vt_eff).clamp(-30.0, 30.0);
        let i = self.bias * x.exp();
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
