//! Editor: the instrument drawn as an exposed circuit board.
//!
//! Flat vector style: solder-mask substrate, copper traces whose brightness
//! follows the signal on each bus, knobs drawn as annular copper pads with a
//! drill hole, SMD slide switches, and the eight touch sensors as pairs of
//! copper plates (press to touch; the vertical position sets the finger
//! pressure, from a feather at the top to a firm press at the bottom).
//! A second theme uses the cream / yellow / green of the DIY board artwork.

use crate::params::{Apolo9Params, Apolo9ParamsParamId as P, SCOPE_LEN, load_f32, store_f32};
use std::sync::atomic::Ordering;
use truce::prelude::*;
use truce_core::editor::{PluginContext, PluginContextReadF32};

pub const WINDOW_W: u32 = 1180;
pub const WINDOW_H: u32 = 780;
pub const MIN_W: u32 = 826;
pub const MIN_H: u32 = 546;
pub const MAX_W: u32 = 2360;
pub const MAX_H: u32 = 1560;

type Ctx = PluginContext<Apolo9Params>;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

// ---------------------------------------------------------------------------
// Palettes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Pal {
    board: Color32,
    board_hi: Color32,
    trace: Color32,
    trace_lit: Color32,
    copper: Color32,
    copper_hi: Color32,
    drill: Color32,
    silk: Color32,
    silk_dim: Color32,
    led: Color32,
    led_off: Color32,
}

const MASK: Pal = Pal {
    board: Color32::from_rgb(14, 22, 18),
    board_hi: Color32::from_rgb(20, 31, 25),
    trace: Color32::from_rgb(44, 66, 50),
    trace_lit: Color32::from_rgb(214, 170, 82),
    copper: Color32::from_rgb(176, 136, 62),
    copper_hi: Color32::from_rgb(236, 196, 110),
    drill: Color32::from_rgb(8, 11, 9),
    silk: Color32::from_rgb(232, 228, 212),
    silk_dim: Color32::from_rgb(128, 140, 128),
    led: Color32::from_rgb(255, 184, 64),
    led_off: Color32::from_rgb(40, 54, 44),
};

const PROTO: Pal = Pal {
    board: Color32::from_rgb(251, 248, 230),
    board_hi: Color32::from_rgb(243, 238, 210),
    trace: Color32::from_rgb(246, 222, 140),
    trace_lit: Color32::from_rgb(240, 180, 30),
    copper: Color32::from_rgb(47, 154, 58),
    copper_hi: Color32::from_rgb(28, 120, 40),
    drill: Color32::from_rgb(251, 248, 230),
    silk: Color32::from_rgb(70, 70, 64),
    silk_dim: Color32::from_rgb(150, 146, 128),
    led: Color32::from_rgb(232, 120, 20),
    led_off: Color32::from_rgb(226, 220, 190),
};

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

// ---------------------------------------------------------------------------
// Layout frame: everything is authored in a 1180×780 space and scaled.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Frame {
    origin: Pos2,
    s: f32,
}

impl Frame {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x * self.s, y * self.s)
    }
    fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.p(x, y), vec2(w * self.s, h * self.s))
    }
    fn len(&self, v: f32) -> f32 {
        v * self.s
    }
    fn font(&self, size: f32) -> FontId {
        FontId::monospace(size * self.s)
    }
}

// ---------------------------------------------------------------------------
// Parameter helpers
// ---------------------------------------------------------------------------

fn default_norm(ctx: &Ctx, id: P) -> f32 {
    let id: u32 = id.into();
    ctx.params()
        .param_infos()
        .iter()
        .find(|i| i.id == id)
        .map(|i| i.range.normalize(i.default_plain) as f32)
        .unwrap_or(0.0)
}

/// Knob drawn as a copper pad. Vertical drag, Shift for fine, double-click
/// to reset.
fn knob(ui: &mut egui::Ui, ctx: &Ctx, f: &Frame, pal: &Pal, id: P, label: &str, cx: f32, cy: f32, r: f32) {
    let center = f.p(cx, cy);
    let rad = f.len(r);
    let rect = Rect::from_center_size(center, Vec2::splat(rad * 2.4));
    let uid = ui.id().with(("knob", u32::from(id)));
    let resp = ui.interact(rect, uid, Sense::click_and_drag());
    let mut v = ctx.get_param(id);
    if resp.drag_started() {
        ctx.begin_edit(id);
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let d = -resp.drag_delta().y / if fine { 900.0 } else { 180.0 } / f.s.max(0.3);
        v = (v + d).clamp(0.0, 1.0);
        ctx.set_param(id, f64::from(v));
    }
    if resp.drag_stopped() {
        ctx.end_edit(id);
    }
    if resp.double_clicked() {
        ctx.automate(id, f64::from(default_norm(ctx, id)));
    }
    let hot = resp.hovered() || resp.dragged();

    let pt = ui.painter();
    let start = std::f32::consts::FRAC_PI_4 * 3.0;
    let sweep = std::f32::consts::FRAC_PI_2 * 3.0;
    // Annular pad.
    pt.circle_filled(center, rad, if hot { pal.copper_hi } else { pal.copper });
    pt.circle_filled(center, rad * 0.62, pal.board);
    pt.circle_filled(center, rad * 0.3, pal.drill);
    // Value arc, just outside the pad.
    let arc = |from: f32, to: f32, r: f32| -> Vec<Pos2> {
        let n = 40;
        (0..=n)
            .map(|k| {
                let a = from + (to - from) * k as f32 / n as f32;
                center + vec2(a.cos(), a.sin()) * r
            })
            .collect()
    };
    pt.add(Shape::line(arc(start, start + sweep, rad * 1.18), Stroke::new(f.len(1.5), pal.trace)));
    if v > 0.002 {
        pt.add(Shape::line(arc(start, start + sweep * v, rad * 1.18), Stroke::new(f.len(2.6), pal.trace_lit)));
    }
    let a = start + sweep * v;
    let dir = vec2(a.cos(), a.sin());
    pt.line_segment([center + dir * rad * 0.35, center + dir * rad * 0.95], Stroke::new(f.len(2.4), pal.silk));
    // Silkscreen label and, when touched, the value.
    pt.text(center + vec2(0.0, rad * 1.42), Align2::CENTER_TOP, label, f.font(10.0), pal.silk);
    if hot {
        let txt = ctx.format_param(id);
        pt.text(center - vec2(0.0, rad * 1.36), Align2::CENTER_BOTTOM, txt, f.font(10.0), pal.copper_hi);
    }
}

/// Shared input for the toggle switches. The hit area covers the switch
/// body and its legends; a click steps (2 positions) or picks the position
/// under the pointer (3 positions), and a vertical swipe throws the lever
/// one position per notch, like flicking the real toggle.
/// `slot` is the lever position counted from the top. Returns the response
/// and the newly requested slot, if any.
fn toggle_input(ui: &mut egui::Ui, uid: egui::Id, f: &Frame, body: Rect, hit: Rect, slots: i32, slot: i32) -> (egui::Response, Option<i32>) {
    let resp = ui.interact(hit, uid, Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
    let mut want = None;
    if resp.drag_started() {
        ui.data_mut(|d| d.insert_temp(uid, slot));
    }
    if resp.dragged() {
        let start: i32 = ui.data(|d| d.get_temp(uid)).unwrap_or(slot);
        let origin = ui.input(|i| i.pointer.press_origin());
        if let (Some(o), Some(pos)) = (origin, resp.interact_pointer_pos()) {
            let notch = f.len(11.0).max(6.0);
            let moved = ((pos.y - o.y) / notch).round() as i32;
            let to = (start + moved).clamp(0, slots - 1);
            if to != slot {
                want = Some(to);
            }
        }
    } else if resp.clicked() {
        if slots == 2 {
            want = Some(1 - slot);
        } else if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.y - body.top()) / body.height()).clamp(0.0, 0.999);
            let to = (t * slots as f32) as i32;
            if to != slot {
                want = Some(to);
            }
        }
    }
    (resp, want)
}

/// Switch body: slot, outline (brighter under the pointer).
fn toggle_body(pt: &egui::Painter, f: &Frame, pal: &Pal, rect: Rect, hot: bool) {
    pt.rect_filled(rect, CornerRadius::same(3), pal.board_hi);
    let (w, c) = if hot { (1.8, pal.copper_hi) } else { (1.2, pal.copper) };
    pt.rect_stroke(rect, CornerRadius::same(3), Stroke::new(f.len(w), c), StrokeKind::Inside);
}

/// Hit area of a switch with legends on its right.
fn side_hit(f: &Frame, x: f32, y: f32, h: f32, longest: usize) -> Rect {
    f.r(x - 9.0, y - 7.0, 36.0 + longest as f32 * 5.4, h + 14.0)
}

/// Two-position SMD slide switch for a bool parameter.
fn switch2(ui: &mut egui::Ui, ctx: &Ctx, f: &Frame, pal: &Pal, id: P, label: &str, on_txt: &str, x: f32, y: f32) {
    let rect = f.r(x, y, 18.0, 34.0);
    let uid = ui.id().with(("sw2", u32::from(id)));
    let on = ctx.get_param(id) > 0.5;
    // Body plus the legends above and below it.
    let hit = f.r(x - 10.0, y - 14.0, 38.0, 68.0);
    let (resp, want) = toggle_input(ui, uid, f, rect, hit, 2, i32::from(!on));
    if let Some(slot) = want {
        ctx.automate(id, if slot == 0 { 1.0 } else { 0.0 });
    }
    let pt = ui.painter();
    toggle_body(pt, f, pal, rect, resp.hovered() || resp.dragged());
    let knob_r = Rect::from_min_size(
        if on { f.p(x + 3.0, y + 3.0) } else { f.p(x + 3.0, y + 17.0) },
        vec2(f.len(12.0), f.len(14.0)),
    );
    pt.rect_filled(knob_r, CornerRadius::same(2), if on { pal.trace_lit } else { pal.silk_dim });
    pt.text(f.p(x + 9.0, y + 40.0), Align2::CENTER_TOP, label, f.font(9.0), pal.silk);
    if on && !on_txt.is_empty() {
        pt.text(f.p(x + 9.0, y - 4.0), Align2::CENTER_BOTTOM, on_txt, f.font(8.0), pal.copper_hi);
    }
}

/// Three-position switch for a 3-variant enum: top / middle / bottom.
fn switch3(ui: &mut egui::Ui, ctx: &Ctx, f: &Frame, pal: &Pal, id: P, labels: [&str; 3], x: f32, y: f32) {
    // Enum indices 0,1,2 are drawn middle, top, bottom (Off / Voices / LFO),
    // matching the Lyra panel: up = voices, centre = off, down = LFO CV.
    let rect = f.r(x, y, 18.0, 48.0);
    let uid = ui.id().with(("sw3", u32::from(id)));
    let idx = (ctx.get_param(id) * 2.0).round() as i32;
    let slot_i = match idx {
        1 => 0,
        2 => 2,
        _ => 1,
    };
    let longest = labels.iter().map(|l| l.len()).max().unwrap_or(0);
    let (resp, want) = toggle_input(ui, uid, f, rect, side_hit(f, x, y, 48.0, longest), 3, slot_i);
    if let Some(to) = want {
        let enum_idx = [1.0, 0.0, 2.0][to as usize];
        ctx.automate(id, enum_idx / 2.0);
    }
    let slot = slot_i as f32;
    let pt = ui.painter();
    toggle_body(pt, f, pal, rect, resp.hovered() || resp.dragged());
    let kr = Rect::from_min_size(f.p(x + 3.0, y + 3.0 + slot * 14.0), vec2(f.len(12.0), f.len(14.0)));
    pt.rect_filled(kr, CornerRadius::same(2), if idx == 0 { pal.silk_dim } else { pal.trace_lit });
    for (k, (lbl, yy)) in [(labels[1], 0.0), (labels[0], 1.0), (labels[2], 2.0)].iter().enumerate() {
        let active = matches!((k, idx), (0, 1) | (1, 0) | (2, 2));
        pt.text(
            f.p(x + 23.0, y + 10.0 + yy * 14.0),
            Align2::LEFT_CENTER,
            *lbl,
            f.font(8.0),
            if active { pal.copper_hi } else { pal.silk_dim },
        );
    }
}

/// Two-state enum shown as a two-position switch with both legends.
fn switch_enum2(ui: &mut egui::Ui, ctx: &Ctx, f: &Frame, pal: &Pal, id: P, up: &str, down: &str, x: f32, y: f32) {
    let rect = f.r(x, y, 18.0, 34.0);
    let uid = ui.id().with(("swe", u32::from(id)));
    // Index 1 is drawn up.
    let is_up = ctx.get_param(id) > 0.5;
    let hit = side_hit(f, x, y, 34.0, up.len().max(down.len()));
    let (resp, want) = toggle_input(ui, uid, f, rect, hit, 2, i32::from(!is_up));
    if let Some(slot) = want {
        ctx.automate(id, if slot == 0 { 1.0 } else { 0.0 });
    }
    let pt = ui.painter();
    toggle_body(pt, f, pal, rect, resp.hovered() || resp.dragged());
    let kr = Rect::from_min_size(
        if is_up { f.p(x + 3.0, y + 3.0) } else { f.p(x + 3.0, y + 17.0) },
        vec2(f.len(12.0), f.len(14.0)),
    );
    pt.rect_filled(kr, CornerRadius::same(2), pal.trace_lit);
    pt.text(f.p(x + 23.0, y + 10.0), Align2::LEFT_CENTER, up, f.font(8.0), if is_up { pal.copper_hi } else { pal.silk_dim });
    pt.text(f.p(x + 23.0, y + 24.0), Align2::LEFT_CENTER, down, f.font(8.0), if is_up { pal.silk_dim } else { pal.copper_hi });
}

fn section(pt: &egui::Painter, f: &Frame, pal: &Pal, x: f32, y: f32, w: f32, h: f32, title: &str) {
    let r = f.r(x, y, w, h);
    pt.rect_stroke(r, CornerRadius::same(6), Stroke::new(f.len(1.0), pal.silk_dim), StrokeKind::Inside);
    let tw = title.len() as f32 * 7.0 + 12.0;
    pt.rect_filled(f.r(x + 12.0, y - 7.0, tw, 14.0), CornerRadius::ZERO, pal.board);
    pt.text(f.p(x + 18.0, y), Align2::LEFT_CENTER, title, f.font(11.0), pal.silk);
}

/// A copper trace through `pts`, lit by `level` (0..1).
fn trace(pt: &egui::Painter, f: &Frame, pal: &Pal, pts: &[(f32, f32)], level: f32, width: f32) {
    let c = mix(pal.trace, pal.trace_lit, level);
    let v: Vec<Pos2> = pts.iter().map(|&(x, y)| f.p(x, y)).collect();
    pt.add(Shape::line(v, Stroke::new(f.len(width), c)));
}

fn via(pt: &egui::Painter, f: &Frame, pal: &Pal, x: f32, y: f32, lit: f32) {
    pt.circle_filled(f.p(x, y), f.len(4.0), mix(pal.copper, pal.copper_hi, lit));
    pt.circle_filled(f.p(x, y), f.len(1.6), pal.drill);
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

const TUNES: [P; 8] = [P::Tune1, P::Tune2, P::Tune3, P::Tune4, P::Tune5, P::Tune6, P::Tune7, P::Tune8];
const SHARPS: [P; 4] = [P::Sharp12, P::Sharp34, P::Sharp56, P::Sharp78];
const MODS: [P; 4] = [P::Mod12, P::Mod34, P::Mod56, P::Mod78];
const SRCS: [P; 4] = [P::Src12, P::Src34, P::Src56, P::Src78];
const FASTS: [P; 4] = [P::Fast12, P::Fast34, P::Fast56, P::Fast78];
const PAIR_NAMES: [&str; 4] = ["1·2", "3·4", "5·6", "7·8"];
const SRC_LABEL: [&str; 4] = ["34", "12", "78", "56"];

pub fn apolo_ui(ui: &mut egui::Ui, ctx: &Ctx) {
    let params = ctx.params();
    let theme = params.theme.read().map(|t| *t).unwrap_or(0);
    let pal = if theme == 1 { PROTO } else { MASK };

    let full = ui.max_rect();
    let s = (full.width() / WINDOW_W as f32).min(full.height() / WINDOW_H as f32);
    let size = vec2(WINDOW_W as f32 * s, WINDOW_H as f32 * s);
    let origin = full.center() - size * 0.5;
    let f = Frame { origin, s };
    ui.painter().rect_filled(full, CornerRadius::ZERO, pal.board);

    let ui_sh = &params.ui;
    let env: [f32; 8] = std::array::from_fn(|i| load_f32(&ui_sh.env[i]).clamp(0.0, 1.0));
    let touch: [f32; 8] = std::array::from_fn(|i| load_f32(&ui_sh.touch[i]));
    let pair: [f32; 4] = std::array::from_fn(|p| (load_f32(&ui_sh.pair[p]) * 2.5).clamp(0.0, 1.0));
    let fm_on: [bool; 4] = std::array::from_fn(|p| (ctx.get_param(SRCS[p]) * 2.0).round() as i32 == 1);
    let fm_loop = ctx.get_param(P::FmLoop) > 0.5;
    let total: f32 = pair.iter().sum::<f32>().min(1.0);

    background(ui, &f, &pal, &env, &pair, &fm_on, fm_loop, total);

    // ---- Header -----------------------------------------------------------
    {
        let pt = ui.painter();
        pt.text(f.p(24.0, 26.0), Align2::LEFT_CENTER, "APOLO9", FontId::monospace(f.len(22.0)), pal.silk);
        pt.text(
            f.p(130.0, 28.0),
            Align2::LEFT_CENTER,
            "ORGANISMIC CIRCUIT MODEL · 8×CD4093 · 2×PT2399 · TL074",
            f.font(10.0),
            pal.silk_dim,
        );
        let serial = params.serial.value();
        pt.text(
            f.p(884.0, 28.0),
            Align2::RIGHT_CENTER,
            format!("SN {serial:04}"),
            f.font(11.0),
            pal.copper_hi,
        );
    }
    // Theme toggle (a jumper).
    {
        let r = f.r(900.0, 16.0, 70.0, 22.0);
        let resp = ui.interact(r, ui.id().with("theme"), Sense::click());
        if resp.clicked() {
            if let Ok(mut t) = params.theme.write() {
                *t = if *t == 1 { 0 } else { 1 };
            }
        }
        let pt = ui.painter();
        pt.rect_stroke(r, CornerRadius::same(4), Stroke::new(f.len(1.0), pal.copper), StrokeKind::Inside);
        pt.text(
            r.center(),
            Align2::CENTER_CENTER,
            if theme == 1 { "PROTO" } else { "MASK" },
            f.font(10.0),
            pal.silk,
        );
    }
    panic_button(ui, ctx, &f, &pal);
    scope(ui, ctx, &f, &pal);

    // ---- Row 1: Hyper LFO · Global · Mod Delay · Distortion -------------
    {
        let pt = ui.painter();
        section(pt, &f, &pal, 20.0, 64.0, 250.0, 176.0, "HYPER LFO");
        section(pt, &f, &pal, 282.0, 64.0, 168.0, 176.0, "GLOBAL");
        section(pt, &f, &pal, 462.0, 64.0, 470.0, 176.0, "MOD DELAY");
        section(pt, &f, &pal, 944.0, 64.0, 216.0, 176.0, "DISTORTION");
    }
    knob(ui, ctx, &f, &pal, P::LfoA, "FREQ A", 66.0, 128.0, 22.0);
    knob(ui, ctx, &f, &pal, P::LfoB, "FREQ B", 150.0, 128.0, 22.0);
    switch2(ui, ctx, &f, &pal, P::LfoLink, "LINK", "A→B", 212.0, 100.0);
    switch_enum2(ui, ctx, &f, &pal, P::LfoLogic, "AND", "OR", 196.0, 172.0);
    {
        let pt = ui.painter();
        let a = ui_sh.lfo_a.load(Ordering::Relaxed);
        let b = ui_sh.lfo_b.load(Ordering::Relaxed);
        for (k, on) in [a, b].iter().enumerate() {
            let c = f.p(52.0 + k as f32 * 84.0, 196.0);
            pt.circle_filled(c, f.len(5.0), if *on { pal.led } else { pal.led_off });
        }
        pt.text(f.p(80.0, 214.0), Align2::LEFT_CENTER, "A  ·  B", f.font(8.0), pal.silk_dim);
    }

    switch2(ui, ctx, &f, &pal, P::Vibrato, "VIBRATO", "ON", 300.0, 98.0);
    switch2(ui, ctx, &f, &pal, P::TotalFb, "TOTAL FB", "ON", 360.0, 98.0);
    switch_enum_bool_loop(ui, ctx, &f, &pal, 300.0, 170.0);

    knob(ui, ctx, &f, &pal, P::Time1, "TIME 1", 508.0, 118.0, 22.0);
    knob(ui, ctx, &f, &pal, P::Dmod1, "MOD 1", 508.0, 196.0, 15.0);
    knob(ui, ctx, &f, &pal, P::Time2, "TIME 2", 592.0, 118.0, 22.0);
    knob(ui, ctx, &f, &pal, P::Dmod2, "MOD 2", 592.0, 196.0, 15.0);
    switch_enum2(ui, ctx, &f, &pal, P::DlyMode, "SELF", "LFO", 640.0, 100.0);
    switch_enum2(ui, ctx, &f, &pal, P::DlyWave, "SQR", "TRI", 640.0, 172.0);
    knob(ui, ctx, &f, &pal, P::Feedback, "FB", 740.0, 128.0, 26.0);
    knob(ui, ctx, &f, &pal, P::DlyMix, "MIX", 836.0, 128.0, 22.0);
    {
        let pt = ui.painter();
        for l in 0..2 {
            let ms = load_f32(&ui_sh.delay_ms[l]);
            pt.text(
                f.p(700.0 + l as f32 * 108.0, 214.0),
                Align2::LEFT_CENTER,
                format!("τ{} {:>5.0} ms", l + 1, ms),
                f.font(9.0),
                pal.silk_dim,
            );
        }
    }

    knob(ui, ctx, &f, &pal, P::Drive, "DRIVE", 990.0, 128.0, 22.0);
    knob(ui, ctx, &f, &pal, P::DistMix, "MIX", 1052.0, 128.0, 18.0);
    knob(ui, ctx, &f, &pal, P::Volume, "VOL", 1112.0, 128.0, 22.0);
    {
        let m = ctx.get_meter(P::MeterOut).clamp(0.0, 1.0);
        let pt = ui.painter();
        let r = f.r(960.0, 200.0, 184.0, 8.0);
        pt.rect_filled(r, CornerRadius::same(2), pal.led_off);
        let mut rr = r;
        rr.set_width(r.width() * m.sqrt());
        pt.rect_filled(rr, CornerRadius::same(2), if m > 0.9 { Color32::from_rgb(230, 70, 50) } else { pal.led });
        pt.text(f.p(960.0, 220.0), Align2::LEFT_CENTER, "OUT", f.font(8.0), pal.silk_dim);
    }

    // ---- Row 2: the two voice groups --------------------------------------
    for g in 0..2 {
        let x0 = 20.0 + g as f32 * 580.0;
        section(ui.painter(), &f, &pal, x0, 262.0, 560.0, 126.0, if g == 0 { "VOICES 1234" } else { "VOICES 5678" });
        let hold = if g == 0 { P::HoldA } else { P::HoldB };
        let pitch = if g == 0 { P::PitchA } else { P::PitchB };
        knob(ui, ctx, &f, &pal, hold, "HOLD", x0 + 46.0, 320.0, 20.0);
        knob(ui, ctx, &f, &pal, pitch, "PITCH", x0 + 112.0, 320.0, 20.0);
        for k in 0..2 {
            let p = g * 2 + k;
            let px = x0 + 166.0 + k as f32 * 196.0;
            ui.painter().text(f.p(px + 76.0, 278.0), Align2::CENTER_CENTER, PAIR_NAMES[p], f.font(10.0), pal.silk_dim);
            knob(ui, ctx, &f, &pal, SHARPS[p], "SHARP", px + 24.0, 330.0, 17.0);
            knob(ui, ctx, &f, &pal, MODS[p], "MOD", px + 76.0, 330.0, 17.0);
            switch3(ui, ctx, &f, &pal, SRCS[p], ["OFF", SRC_LABEL[p], "LFO"], px + 108.0, 304.0);
            switch2(ui, ctx, &f, &pal, FASTS[p], "FAST", "", px + 160.0, 310.0);
        }
    }

    // ---- Row 3: tune knobs and sensors ------------------------------------
    for i in 0..8 {
        let cx = 20.0 + 70.0 + i as f32 * 142.5;
        knob(ui, ctx, &f, &pal, TUNES[i], &format!("TUNE {}", i + 1), cx, 454.0, 24.0);
        // Envelope LED.
        let pt = ui.painter();
        let c = f.p(cx + 42.0, 432.0);
        pt.circle_filled(c, f.len(5.0), mix(pal.led_off, pal.led, env[i].sqrt()));
        sensor(ui, ctx, &f, &pal, i, cx, 568.0, env[i], touch[i]);
    }

    // ---- Bottom strip: CV jacks, lab, MIDI -------------------------------
    {
        let pt = ui.painter();
        section(pt, &f, &pal, 20.0, 680.0, 440.0, 88.0, "REAR JACKS (CV)");
        section(pt, &f, &pal, 472.0, 680.0, 470.0, 88.0, "CIRCUIT LAB");
        section(pt, &f, &pal, 954.0, 680.0, 206.0, 88.0, "MIDI");
    }
    knob(ui, ctx, &f, &pal, P::HoldGate, "HOLD GATE", 64.0, 722.0, 13.0);
    switch2(ui, ctx, &f, &pal, P::HoldGateOn, "PLUG", "", 100.0, 702.0);
    knob(ui, ctx, &f, &pal, P::CvVoices, "CV VOICES", 186.0, 722.0, 13.0);
    switch2(ui, ctx, &f, &pal, P::CvVoicesOn, "PLUG", "", 222.0, 702.0);
    knob(ui, ctx, &f, &pal, P::CvDelay, "CV DELAY", 308.0, 722.0, 13.0);
    switch2(ui, ctx, &f, &pal, P::CvDelayOn, "PLUG", "", 344.0, 702.0);

    knob(ui, ctx, &f, &pal, P::Humidity, "HUMIDITY", 530.0, 722.0, 14.0);
    knob(ui, ctx, &f, &pal, P::Crosstalk, "CROSSTALK", 640.0, 722.0, 14.0);
    knob(ui, ctx, &f, &pal, P::Noise, "NOISE", 750.0, 722.0, 14.0);
    knob(ui, ctx, &f, &pal, P::Serial, "SERIAL", 860.0, 722.0, 14.0);

    switch_enum2(ui, ctx, &f, &pal, P::KeyMap, "CHROM", "WHITE", 972.0, 702.0);
    knob(ui, ctx, &f, &pal, P::BaseNote, "BASE NOTE", 1100.0, 722.0, 14.0);

    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// The FM structure switch: down = two loops (34>12, 12>34 …), up = one ring.
fn switch_enum_bool_loop(ui: &mut egui::Ui, ctx: &Ctx, f: &Frame, pal: &Pal, x: f32, y: f32) {
    switch_enum2(ui, ctx, f, pal, P::FmLoop, "34>56 78>12", "34>12 78>56", x, y);
}

/// Decorative copper: sensor → tune, pair buses under the sensors, FM
/// links in the channel between the rows, the master bus up the right edge
/// into the delay and the distortion. Each trace glows with its signal.
#[allow(clippy::too_many_arguments)]
fn background(ui: &egui::Ui, f: &Frame, pal: &Pal, env: &[f32; 8], pair: &[f32; 4], fm_on: &[bool; 4], fm_loop: bool, total: f32) {
    let pt = ui.painter();
    // Faint via grid.
    let mut y = 60.0;
    while y < 770.0 {
        let mut x = 12.0;
        while x < 1175.0 {
            pt.circle_filled(f.p(x, y), f.len(0.9), pal.board_hi);
            x += 24.0;
        }
        y += 24.0;
    }
    // Sensor ↔ tune (left side) and sensor → pair bus (below).
    for i in 0..8 {
        let cx = 90.0 + i as f32 * 142.5;
        let p = i / 2;
        let bus_y = 652.0 + (p % 2) as f32 * 7.0;
        trace(pt, f, pal, &[(cx - 52.0, 560.0), (cx - 62.0, 550.0), (cx - 62.0, 470.0), (cx - 38.0, 454.0)], env[i], 2.0);
        trace(pt, f, pal, &[(cx + 40.0, 620.0), (cx + 40.0, bus_y)], env[i] * 0.8, 2.0);
        via(pt, f, pal, cx + 40.0, bus_y, env[i]);
    }
    // Pair buses run right, then all four climb the right edge.
    for p in 0..4 {
        let x0 = 130.0 + (p * 2) as f32 * 142.5;
        let y = 652.0 + (p % 2) as f32 * 7.0;
        let xr = 1166.0 - p as f32 * 4.0;
        trace(pt, f, pal, &[(x0, y), (xr, y), (xr, 404.0)], pair[p], 2.4);
    }
    // FM links: from the source pair's MOD column to the modulated pair.
    let mod_x = |p: usize| 20.0 + (p / 2) as f32 * 580.0 + 166.0 + (p % 2) as f32 * 196.0 + 76.0;
    for p in 0..4 {
        if !fm_on[p] {
            continue;
        }
        let src = match (p, fm_loop) {
            (0, false) => 1,
            (0, true) => 3,
            (1, _) => 0,
            (2, false) => 3,
            (2, true) => 1,
            _ => 2,
        };
        let y = 398.0 + p as f32 * 4.0;
        trace(pt, f, pal, &[(mod_x(src), 388.0), (mod_x(src), y), (mod_x(p) + 6.0, y), (mod_x(p) + 6.0, 388.0)], pair[src], 1.6);
    }
    // Master bus up the right edge into the delay, delay → distortion.
    trace(pt, f, pal, &[(1166.0, 404.0), (1172.0, 396.0), (1172.0, 252.0), (920.0, 252.0), (920.0, 240.0)], total, 3.0);
    trace(pt, f, pal, &[(932.0, 150.0), (944.0, 150.0)], total, 3.0);
}

/// One touch sensor: two copper plates. Press between them to touch; the
/// vertical position sets the pressure (top = feather, bottom = firm).
#[allow(clippy::too_many_arguments)]
fn sensor(ui: &mut egui::Ui, ctx: &Ctx, f: &Frame, pal: &Pal, i: usize, cx: f32, cy: f32, env: f32, touch: f32) {
    let w = 100.0;
    let h = 100.0;
    let rect = f.r(cx - w / 2.0, cy - h / 2.0, w, h);
    let resp = ui.interact(rect, ui.id().with(("sensor", i)), Sense::click_and_drag());
    let pad = &ctx.params().ui.pad[i];
    let mut latched = ui.memory(|m| m.data.get_temp::<bool>(ui.id().with(("latch", i)))).unwrap_or(false);
    if latched && load_f32(pad) <= 0.0 && !resp.is_pointer_button_down_on() {
        // Something (a panic) lifted the latched finger.
        latched = false;
        ui.memory_mut(|m| m.data.insert_temp(ui.id().with(("latch", i)), false));
    }
    if resp.is_pointer_button_down_on() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            store_f32(pad, 0.12 + 0.88 * t);
        }
    } else if !latched && load_f32(pad) > 0.0 {
        store_f32(pad, 0.0);
    }
    if resp.secondary_clicked() {
        // Right click: leave a finger there (latch), right click again lifts it.
        let now = !latched;
        ui.memory_mut(|m| m.data.insert_temp(ui.id().with(("latch", i)), now));
        store_f32(pad, if now { 0.8 } else { 0.0 });
    }

    let pt = ui.painter();
    let glow = env.sqrt();
    // Halo from the envelope.
    if glow > 0.02 {
        pt.rect_filled(rect.expand(f.len(6.0 * glow)), CornerRadius::same(10), mix(pal.board, pal.led, glow * 0.35));
    }
    pt.rect_filled(rect, CornerRadius::same(8), pal.board_hi);
    // Upper (sensitive) and lower (contact) plates.
    let plate = |y0: f32| f.r(cx - w / 2.0 + 10.0, y0, w - 20.0, 34.0);
    let up = plate(cy - h / 2.0 + 10.0);
    let lo = plate(cy + h / 2.0 - 44.0);
    let tc = mix(pal.copper, pal.copper_hi, touch);
    pt.rect_filled(up, CornerRadius::same(5), tc);
    pt.rect_filled(lo, CornerRadius::same(5), tc);
    // Interleaved fingers of the plates.
    for k in 0..4 {
        let x = up.left() + f.len(10.0 + k as f32 * 20.0);
        pt.rect_filled(
            Rect::from_min_size(pos2(x, up.bottom()), vec2(f.len(5.0), f.len(8.0))),
            CornerRadius::ZERO,
            tc,
        );
        pt.rect_filled(
            Rect::from_min_size(pos2(x + f.len(10.0), lo.top() - f.len(8.0)), vec2(f.len(5.0), f.len(8.0))),
            CornerRadius::ZERO,
            tc,
        );
    }
    if touch > 0.0 {
        pt.circle_filled(rect.center(), f.len(6.0 + 10.0 * touch), mix(pal.board, pal.led, 0.85));
    }
    pt.text(f.p(cx, cy + h / 2.0 + 12.0), Align2::CENTER_CENTER, format!("{}", i + 1), f.font(12.0), pal.silk);
    if latched {
        pt.text(f.p(cx + 40.0, cy + h / 2.0 + 12.0), Align2::CENTER_CENTER, "●", f.font(9.0), pal.led);
    }
}

/// PANIC: a red pad in the header. Kills every sound at once (see
/// `Engine::panic`); also reachable as an automatable parameter and MIDI CC 120.
fn panic_button(ui: &mut egui::Ui, ctx: &Ctx, f: &Frame, pal: &Pal) {
    let r = f.r(704.0, 16.0, 84.0, 22.0);
    let resp = ui.interact(r, ui.id().with("panic"), Sense::click());
    if resp.clicked() {
        ctx.params().ui.panic.fetch_add(1, Ordering::Relaxed);
        // Lift any latched fingers on the panel too.
        for i in 0..8 {
            ui.memory_mut(|m| m.data.insert_temp(ui.id().with(("latch", i)), false));
        }
    }
    let red = Color32::from_rgb(214, 64, 52);
    let hot = resp.hovered();
    let pt = ui.painter();
    pt.rect_filled(r, CornerRadius::same(4), if hot { red } else { mix(pal.board, red, 0.25) });
    pt.rect_stroke(r, CornerRadius::same(4), Stroke::new(f.len(1.2), red), StrokeKind::Inside);
    pt.text(r.center(), Align2::CENTER_CENTER, "PANIC", f.font(11.0), if hot { Color32::WHITE } else { red });
}

/// Output oscilloscope in the header.
fn scope(ui: &mut egui::Ui, ctx: &Ctx, f: &Frame, pal: &Pal) {
    let ui_sh = &ctx.params().ui;
    let r = f.r(984.0, 10.0, 176.0, 36.0);
    let pt = ui.painter();
    pt.rect_filled(r, CornerRadius::same(4), pal.board_hi);
    let pos = ui_sh.scope_pos.load(Ordering::Relaxed) as usize;
    let n = 120;
    let pts: Vec<Pos2> = (0..n)
        .map(|k| {
            let idx = (pos + SCOPE_LEN - n + k) % SCOPE_LEN;
            let v = load_f32(&ui_sh.scope[idx]).clamp(-1.0, 1.0);
            pos2(r.left() + r.width() * k as f32 / (n - 1) as f32, r.center().y - v * r.height() * 0.48)
        })
        .collect();
    pt.add(Shape::line(pts, Stroke::new(f.len(1.3), pal.trace_lit)));
}
