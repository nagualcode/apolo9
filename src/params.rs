//! Panel controls of the Lyra-8 (names follow the hardware), the rear CV
//! jacks, the "lab" controls of the circuit model, and lock-free status
//! published by the audio thread for the editor.

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use truce::prelude::*;

#[derive(ParamEnum)]
pub enum ModSource {
    Off,
    #[name = "Voices"]
    Voices,
    #[name = "LFO CV"]
    Lfo,
}

#[derive(ParamEnum)]
pub enum LfoLogic {
    #[name = "OR (sum)"]
    Or,
    #[name = "AND"]
    And,
}

#[derive(ParamEnum)]
pub enum DelayModMode {
    #[name = "LFO"]
    Lfo,
    #[name = "SELF"]
    SelfMod,
}

#[derive(ParamEnum)]
pub enum DelayWave {
    #[name = "Triangle"]
    Tri,
    #[name = "Square"]
    Square,
}

#[derive(ParamEnum)]
pub enum KeyMap {
    #[name = "White keys"]
    White,
    #[name = "Chromatic"]
    Chromatic,
}

/// Default TUNE positions: an open A-minor voicing on the default unit
/// (A1 E2 A2 C3 E3 G3 C4 E4), computed by inverting the circuit model.
pub const DEFAULT_TUNE: [f64; 8] = [0.4177, 0.5376, 0.4965, 0.4642, 0.3663, 0.3802, 0.2945, 0.3444];

#[derive(Params)]
pub struct Apolo9Params {
    // ---- Voices -----------------------------------------------------------
    #[param(name = "Tune 1", short_name = "T1", group = "Voices", range = "linear(0, 1)", default = 0.4177)]
    pub tune1: FloatParam,
    #[param(name = "Tune 2", short_name = "T2", group = "Voices", range = "linear(0, 1)", default = 0.5376)]
    pub tune2: FloatParam,
    #[param(name = "Tune 3", short_name = "T3", group = "Voices", range = "linear(0, 1)", default = 0.4965)]
    pub tune3: FloatParam,
    #[param(name = "Tune 4", short_name = "T4", group = "Voices", range = "linear(0, 1)", default = 0.4642)]
    pub tune4: FloatParam,
    #[param(name = "Tune 5", short_name = "T5", group = "Voices", range = "linear(0, 1)", default = 0.3663)]
    pub tune5: FloatParam,
    #[param(name = "Tune 6", short_name = "T6", group = "Voices", range = "linear(0, 1)", default = 0.3802)]
    pub tune6: FloatParam,
    #[param(name = "Tune 7", short_name = "T7", group = "Voices", range = "linear(0, 1)", default = 0.2945)]
    pub tune7: FloatParam,
    #[param(name = "Tune 8", short_name = "T8", group = "Voices", range = "linear(0, 1)", default = 0.3444)]
    pub tune8: FloatParam,

    #[param(name = "Sharp 1-2", short_name = "Shp12", group = "Voices", range = "linear(0, 1)", default = 0.0)]
    pub sharp12: FloatParam,
    #[param(name = "Sharp 3-4", short_name = "Shp34", group = "Voices", range = "linear(0, 1)", default = 0.0)]
    pub sharp34: FloatParam,
    #[param(name = "Sharp 5-6", short_name = "Shp56", group = "Voices", range = "linear(0, 1)", default = 0.0)]
    pub sharp56: FloatParam,
    #[param(name = "Sharp 7-8", short_name = "Shp78", group = "Voices", range = "linear(0, 1)", default = 0.0)]
    pub sharp78: FloatParam,

    #[param(name = "Mod 1-2", short_name = "Mod12", group = "FM", range = "linear(0, 1)", default = 0.5)]
    pub mod12: FloatParam,
    #[param(name = "Mod 3-4", short_name = "Mod34", group = "FM", range = "linear(0, 1)", default = 0.5)]
    pub mod34: FloatParam,
    #[param(name = "Mod 5-6", short_name = "Mod56", group = "FM", range = "linear(0, 1)", default = 0.5)]
    pub mod56: FloatParam,
    #[param(name = "Mod 7-8", short_name = "Mod78", group = "FM", range = "linear(0, 1)", default = 0.5)]
    pub mod78: FloatParam,

    #[param(name = "Mod Source 1-2", short_name = "Src12", group = "FM")]
    pub src12: EnumParam<ModSource>,
    #[param(name = "Mod Source 3-4", short_name = "Src34", group = "FM")]
    pub src34: EnumParam<ModSource>,
    #[param(name = "Mod Source 5-6", short_name = "Src56", group = "FM")]
    pub src56: EnumParam<ModSource>,
    #[param(name = "Mod Source 7-8", short_name = "Src78", group = "FM")]
    pub src78: EnumParam<ModSource>,

    #[param(name = "Fast 1-2", short_name = "Fast12", group = "Voices", default = 0)]
    pub fast12: BoolParam,
    #[param(name = "Fast 3-4", short_name = "Fast34", group = "Voices", default = 0)]
    pub fast34: BoolParam,
    #[param(name = "Fast 5-6", short_name = "Fast56", group = "Voices", default = 0)]
    pub fast56: BoolParam,
    #[param(name = "Fast 7-8", short_name = "Fast78", group = "Voices", default = 0)]
    pub fast78: BoolParam,

    #[param(name = "Pitch 1234", short_name = "PitchA", group = "Voices", range = "linear(0, 1)", default = 0.92)]
    pub pitch_a: FloatParam,
    #[param(name = "Pitch 5678", short_name = "PitchB", group = "Voices", range = "linear(0, 1)", default = 0.92)]
    pub pitch_b: FloatParam,
    #[param(name = "Hold 1234", short_name = "HoldA", group = "Voices", range = "linear(0, 1)", default = 0.0)]
    pub hold_a: FloatParam,
    #[param(name = "Hold 5678", short_name = "HoldB", group = "Voices", range = "linear(0, 1)", default = 0.0)]
    pub hold_b: FloatParam,

    #[param(name = "Vibrato", group = "Global", default = 0)]
    pub vibrato: BoolParam,
    #[param(name = "Total FB", group = "Global", default = 0)]
    pub total_fb: BoolParam,
    /// FM structure switch: on = 34>56 78>12 (one closed FM loop).
    #[param(name = "FM Loop 34>56 78>12", short_name = "FMLoop", group = "Global", default = 0)]
    pub fm_loop: BoolParam,

    // ---- Hyper LFO --------------------------------------------------------
    #[param(name = "LFO Freq A", short_name = "FreqA", group = "Hyper LFO", range = "linear(0, 1)", default = 0.35)]
    pub lfo_a: FloatParam,
    #[param(name = "LFO Freq B", short_name = "FreqB", group = "Hyper LFO", range = "linear(0, 1)", default = 0.5)]
    pub lfo_b: FloatParam,
    #[param(name = "LFO Logic", short_name = "Logic", group = "Hyper LFO")]
    pub lfo_logic: EnumParam<LfoLogic>,
    #[param(name = "LFO Link", short_name = "Link", group = "Hyper LFO", default = 0)]
    pub lfo_link: BoolParam,

    // ---- Mod delay --------------------------------------------------------
    #[param(name = "Time 1", group = "Mod Delay", range = "linear(0, 1)", default = 0.55)]
    pub time1: FloatParam,
    #[param(name = "Time 2", group = "Mod Delay", range = "linear(0, 1)", default = 0.68)]
    pub time2: FloatParam,
    #[param(name = "Delay Mod 1", short_name = "DMod1", group = "Mod Delay", range = "linear(0, 1)", default = 0.0)]
    pub dmod1: FloatParam,
    #[param(name = "Delay Mod 2", short_name = "DMod2", group = "Mod Delay", range = "linear(0, 1)", default = 0.0)]
    pub dmod2: FloatParam,
    #[param(name = "Delay Mod Mode", short_name = "DMode", group = "Mod Delay")]
    pub dly_mode: EnumParam<DelayModMode>,
    #[param(name = "Delay LFO Wave", short_name = "DWave", group = "Mod Delay")]
    pub dly_wave: EnumParam<DelayWave>,
    #[param(name = "Feedback", short_name = "FB", group = "Mod Delay", range = "linear(0, 1)", default = 0.35)]
    pub feedback: FloatParam,
    #[param(name = "Delay Mix", short_name = "DMix", group = "Mod Delay", range = "linear(0, 1)", default = 0.35)]
    pub dly_mix: FloatParam,

    // ---- Distortion / output ---------------------------------------------
    #[param(name = "Drive", group = "Distortion", range = "linear(0, 1)", default = 0.3)]
    pub drive: FloatParam,
    #[param(name = "Dist Mix", short_name = "DistMix", group = "Distortion", range = "linear(0, 1)", default = 0.0)]
    pub dist_mix: FloatParam,
    #[param(name = "Volume", short_name = "Vol", group = "Distortion", range = "linear(0, 1)", default = 0.75)]
    pub volume: FloatParam,

    // ---- Rear-panel jacks -------------------------------------------------
    #[param(name = "Hold Gate", group = "CV", range = "linear(0, 5)", default = 0.0)]
    pub hold_gate: FloatParam,
    #[param(name = "Hold Gate Plugged", short_name = "HGPlug", group = "CV", default = 0)]
    pub hold_gate_on: BoolParam,
    #[param(name = "CV Voices", group = "CV", range = "linear(0, 5)", default = 0.0)]
    pub cv_voices: FloatParam,
    #[param(name = "CV Voices Plugged", short_name = "CVVPlug", group = "CV", default = 0)]
    pub cv_voices_on: BoolParam,
    #[param(name = "CV Delay", group = "CV", range = "linear(0, 5)", default = 0.0)]
    pub cv_delay: FloatParam,
    #[param(name = "CV Delay Plugged", short_name = "CVDPlug", group = "CV", default = 0)]
    pub cv_delay_on: BoolParam,
    #[param(name = "Ext In Level", short_name = "ExtIn", group = "CV", range = "linear(0, 2)", default = 1.0)]
    pub ext_gain: FloatParam,

    // ---- Circuit lab ------------------------------------------------------
    /// Moisture across the sensor pads: at high values some voices leak on.
    #[param(name = "Humidity", group = "Lab", range = "linear(0, 1)", default = 0.0)]
    pub humidity: FloatParam,
    /// Ground bounce / rail sag between the oscillators (0.35 = stock).
    #[param(name = "Crosstalk", group = "Lab", range = "linear(0, 1)", default = 0.35)]
    pub crosstalk: FloatParam,
    /// Component noise (threshold jitter, hiss, drift, finger tremor).
    #[param(name = "Noise", group = "Lab", range = "linear(0, 2)", default = 1.0)]
    pub noise: FloatParam,
    /// Which physical unit: seeds every component tolerance.
    #[param(name = "Serial", group = "Lab", range = "discrete(1, 9999)", default = 424)]
    pub serial: IntParam,

    // ---- MIDI ------------------------------------------------------------
    #[param(name = "Key Map", group = "MIDI")]
    pub key_map: EnumParam<KeyMap>,
    #[param(name = "Base Note", group = "MIDI", range = "discrete(0, 120)", default = 60)]
    pub base_note: IntParam,

    /// Momentary: rising edge = panic (also on the panel and MIDI CC 120).
    #[param(name = "Panic", group = "Global", default = 0)]
    pub panic: BoolParam,

    #[meter]
    pub meter_out: MeterSlot,

    /// 0 = solder mask (dark), 1 = proto board (cream).
    #[persist = "theme"]
    pub theme: RwLock<u8>,

    #[skip]
    pub ui: UiShared,
}

pub const SCOPE_LEN: usize = 256;

/// Editor ⇄ audio thread, lock-free.
pub struct UiShared {
    /// Mouse touch pressure per sensor (f32 bits, 0 = released).
    pub pad: [AtomicU32; 8],
    /// Envelope capacitor voltage / 10 V (f32 bits).
    pub env: [AtomicU32; 8],
    /// Combined touch pressure actually applied (f32 bits).
    pub touch: [AtomicU32; 8],
    pub vib: [AtomicU32; 8],
    pub pair: [AtomicU32; 4],
    pub lfo_a: AtomicBool,
    pub lfo_b: AtomicBool,
    pub delay_ms: [AtomicU32; 2],
    pub scope: [AtomicU32; SCOPE_LEN],
    pub scope_pos: AtomicU32,
    /// Panel PANIC presses (a change means "panic now").
    pub panic: AtomicU32,
}

impl Default for UiShared {
    fn default() -> Self {
        Self {
            pad: std::array::from_fn(|_| AtomicU32::new(0)),
            env: std::array::from_fn(|_| AtomicU32::new(0)),
            touch: std::array::from_fn(|_| AtomicU32::new(0)),
            vib: std::array::from_fn(|_| AtomicU32::new(0)),
            pair: std::array::from_fn(|_| AtomicU32::new(0)),
            lfo_a: AtomicBool::new(false),
            lfo_b: AtomicBool::new(false),
            delay_ms: std::array::from_fn(|_| AtomicU32::new(0)),
            scope: std::array::from_fn(|_| AtomicU32::new(0)),
            scope_pos: AtomicU32::new(0),
            panic: AtomicU32::new(0),
        }
    }
}

#[inline]
pub fn store_f32(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

#[inline]
pub fn load_f32(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}
