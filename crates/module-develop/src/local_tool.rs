//! The local tools (plan v0.2.0 §4.2–4.4): Red Eye, Gradient and Brush. One
//! struct serves all three (switching between them keeps the draft): it
//! holds a *draft* of the red-eye spots and local adjustments that renders
//! live, and Apply commits it as one history step. Everything placed on the
//! image is stored in source coordinates; the pointer and the drawing go
//! through `Geometry`.

use egui::{Color32, CursorIcon, Key, Pos2, Rect, Response, Stroke as Line, Ui, Vec2};
use viberoom_services::engine::EditParams;
use viberoom_services::engine::ParamSpec;
use viberoom_services::engine::geometry::Geometry;
use viberoom_services::engine::local::{LinearMask, LocalAdjustment, MAX_LOCAL, MaskDef, Stroke};
use viberoom_services::engine::ops::{EyeMode, EyeSpot, MAX_SPOTS, RedEye, RedEyeParams};

use crate::basic::spec_slider;
use crate::tool::{CanvasTool, Outcome, ToolAction, ToolId};

const PIN: f32 = 10.0;
const KNOB_DIST: f32 = 90.0;
const EDGE_GRAB: f32 = 6.0;
const DRAG_MIN: f32 = 3.0;

const SIZE: ParamSpec = ParamSpec::linear("size", "Size", 0.1, 10.0, 1.0, 0.05, 0.5);
const DARKEN: ParamSpec = ParamSpec::linear("darken", "Darken", 0.0, 100.0, 50.0, 1.0, 5.0);
const BRUSH_SIZE: ParamSpec = ParamSpec::linear("size", "Size", 0.2, 30.0, 3.0, 0.1, 1.0);
const FEATHER: ParamSpec = ParamSpec::linear("feather", "Feather", 0.0, 100.0, 50.0, 1.0, 5.0);
const FLOW: ParamSpec = ParamSpec::linear("flow", "Flow", 1.0, 100.0, 100.0, 1.0, 5.0);
const TONE: [ParamSpec; 6] = [
    ParamSpec {
        unit: "EV",
        precision: 2,
        ..ParamSpec::linear("exposure", "Exposure", -5.0, 5.0, 0.0, 0.01, 0.1)
    },
    ParamSpec::linear("contrast", "Contrast", -100.0, 100.0, 0.0, 1.0, 5.0),
    ParamSpec::linear("highlights", "Highlights", -100.0, 100.0, 0.0, 1.0, 5.0),
    ParamSpec::linear("shadows", "Shadows", -100.0, 100.0, 0.0, 1.0, 5.0),
    ParamSpec::linear("whites", "Whites", -100.0, 100.0, 0.0, 1.0, 5.0),
    ParamSpec::linear("blacks", "Blacks", -100.0, 100.0, 0.0, 1.0, 5.0),
];

#[derive(Debug, Clone)]
enum Drag {
    SpotMove { idx: usize, grab: [f64; 2] },
    SpotSize { idx: usize, created: bool },
    GradMove,
    GradRotate,
    GradFeather,
    GradNew { origin: Pos2 },
    Paint { last: Pos2 },
}

#[derive(Debug)]
pub struct LocalTool {
    kind: ToolId,
    spots: Vec<EyeSpot>,
    adjustments: Vec<LocalAdjustment>,
    orig_spots: Vec<EyeSpot>,
    orig: Vec<LocalAdjustment>,
    sel_spot: Option<usize>,
    sel: Option<String>,
    show_mask: bool,
    eye_mode: EyeMode,
    eye_darken: f64,
    eye_catchlight: bool,
    /// Radius of a newly placed circle, fraction of the diagonal.
    eye_r: f64,
    brush: Stroke,
    drag: Option<Drag>,
    counter: u32,
    note: Option<&'static str>,
}

// --- Geometry helpers --------------------------------------------------------

struct View<'a> {
    img: Rect,
    geom: &'a Geometry,
}

impl View<'_> {
    /// Screen pixels per source pixel.
    fn scale(&self) -> f64 {
        f64::from(self.img.width()) / self.geom.crop_px().0
    }

    fn diag(&self) -> f64 {
        f64::from(self.geom.canvas.0).hypot(f64::from(self.geom.canvas.1))
    }

    /// A fraction of the source diagonal, in screen pixels.
    fn len_px(&self, frac: f64) -> f32 {
        (frac * self.diag() * self.scale()) as f32
    }

    fn frac(&self, px: f32) -> f64 {
        f64::from(px) / (self.diag() * self.scale())
    }

    fn to_src(&self, pos: Pos2) -> Option<[f64; 2]> {
        let u = f64::from((pos.x - self.img.left()) / self.img.width());
        let v = f64::from((pos.y - self.img.top()) / self.img.height());
        self.geom.output_to_source(u, v)
    }

    fn to_screen(&self, p: [f64; 2]) -> Pos2 {
        let [u, v] = self.geom.source_to_output(p[0], p[1]);
        Pos2::new(
            self.img.left() + u as f32 * self.img.width(),
            self.img.top() + v as f32 * self.img.height(),
        )
    }

    /// The gradient's unit normal and line direction on screen.
    fn line_frame(&self, m: &LinearMask) -> (Vec2, Vec2) {
        let (s, c) = m.angle.to_radians().sin_cos();
        let n = self.geom.source_vector_to_output([-s, c]);
        let d = self.geom.source_vector_to_output([c, s]);
        (
            Vec2::new(n[0] as f32, n[1] as f32),
            Vec2::new(d[0] as f32, d[1] as f32),
        )
    }

    /// The source angle (degrees) whose normal points along `n` on screen.
    fn angle_for_normal(&self, n: Vec2) -> f64 {
        let s = self
            .geom
            .output_vector_to_source([f64::from(n.x), f64::from(n.y)]);
        (-s[0]).atan2(s[1]).to_degrees()
    }
}

fn line(width: f32, color: Color32) -> Line {
    Line::new(width, color)
}

fn dist_to_line(p: Pos2, through: Pos2, normal: Vec2) -> f32 {
    (p - through).dot(normal).abs()
}

fn dashed(painter: &egui::Painter, a: Pos2, b: Pos2, line: Line) {
    painter.extend(egui::Shape::dashed_line(&[a, b], line, 6.0, 5.0));
}

fn tone_get(a: &LocalAdjustment, i: usize) -> f64 {
    let t = &a.adjust;
    [
        t.exposure,
        t.contrast,
        t.highlights,
        t.shadows,
        t.whites,
        t.blacks,
    ][i]
}

fn tone_set(a: &mut LocalAdjustment, i: usize, v: f64) {
    let t = &mut a.adjust;
    *[
        &mut t.exposure,
        &mut t.contrast,
        &mut t.highlights,
        &mut t.shadows,
        &mut t.whites,
        &mut t.blacks,
    ][i] = v;
}

impl LocalTool {
    pub fn new(kind: ToolId, doc: &EditParams) -> Self {
        let spots = doc.get::<RedEye>().spots;
        let adjustments = doc.local_adjustments();
        Self {
            kind,
            orig_spots: spots.clone(),
            orig: adjustments.clone(),
            spots,
            adjustments,
            sel_spot: None,
            sel: None,
            show_mask: false,
            eye_mode: EyeMode::Red,
            eye_darken: 50.0,
            eye_catchlight: false,
            eye_r: 0.01,
            brush: Stroke::default(),
            drag: None,
            counter: 0,
            note: None,
        }
    }

    fn selected(&self) -> Option<usize> {
        let id = self.sel.as_ref()?;
        self.adjustments.iter().position(|a| &a.id == id)
    }

    fn new_id(&mut self) -> String {
        loop {
            self.counter += 1;
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let id = format!("l{:x}", nanos ^ self.counter.wrapping_mul(0x9e37_79b9));
            if self.adjustments.iter().all(|a| a.id != id) {
                return id;
            }
        }
    }

    fn name_for(&self, prefix: &str, brush: bool) -> String {
        let n = self
            .adjustments
            .iter()
            .filter(|a| matches!(a.mask, MaskDef::Brush { .. }) == brush)
            .count();
        format!("{prefix} {}", n + 1)
    }

    fn at_limit(&mut self) -> bool {
        let full = self.adjustments.len() >= MAX_LOCAL;
        self.note = full.then_some("Limit of 16 local adjustments reached");
        full
    }

    fn add(&mut self, name: String, mask: MaskDef) -> usize {
        let id = self.new_id();
        self.adjustments
            .push(LocalAdjustment::new(id.clone(), name, mask));
        self.sel = Some(id);
        self.adjustments.len() - 1
    }

    fn flip(&mut self) {
        if let Some(i) = self.selected()
            && let MaskDef::Linear(m) = &mut self.adjustments[i].mask
        {
            m.angle = (m.angle + 180.0).rem_euclid(360.0);
        }
    }

    fn delete(&mut self) {
        match self.kind {
            ToolId::RedEye => {
                if let Some(i) = self.sel_spot.take()
                    && i < self.spots.len()
                {
                    self.spots.remove(i);
                }
            }
            _ => {
                if let Some(i) = self.selected() {
                    self.adjustments.remove(i);
                    self.sel = None;
                }
            }
        }
    }

    /// The history label for committing this draft.
    fn label(&self, result: &[LocalAdjustment]) -> &'static str {
        if let Some(added) = result
            .iter()
            .find(|a| self.orig.iter().all(|o| o.id != a.id))
        {
            return match added.mask {
                MaskDef::Brush { .. } => "Add Brush Zone",
                MaskDef::Linear(_) => "Add Gradient",
            };
        }
        if self
            .orig
            .iter()
            .any(|o| result.iter().all(|a| a.id != o.id))
        {
            return "Delete Local Adjustment";
        }
        if result != self.orig {
            let brush_changed = result
                .iter()
                .zip(&self.orig)
                .any(|(a, o)| matches!(a.mask, MaskDef::Brush { .. }) && a.mask != o.mask);
            return if brush_changed {
                "Brush"
            } else {
                "Edit Gradient"
            };
        }
        "Red Eye"
    }

    // --- Canvas: red eye -----------------------------------------------------

    fn spot_hit(&self, v: &View, pos: Pos2) -> Option<(usize, bool)> {
        self.spots.iter().enumerate().rev().find_map(|(i, s)| {
            let c = v.to_screen([s.x, s.y]);
            let r = v.len_px(s.r);
            let d = c.distance(pos);
            ((d - r).abs() <= EDGE_GRAB)
                .then_some((i, true))
                .or((d < r).then_some((i, false)))
        })
    }

    fn red_eye_canvas(&mut self, ui: &Ui, v: &View, resp: &Response) {
        let painter = ui.painter_at(v.img.expand(2.0));
        for (i, s) in self.spots.iter().enumerate() {
            let c = v.to_screen([s.x, s.y]);
            let col = match s.mode {
                EyeMode::Red => Color32::WHITE,
                EyeMode::Pet => Color32::from_rgb(255, 190, 40),
            };
            let w: f32 = if Some(i) == self.sel_spot { 2.5 } else { 1.2 };
            painter.circle_stroke(c, v.len_px(s.r), line(w + 1.5, Color32::BLACK));
            painter.circle_stroke(c, v.len_px(s.r), line(w, col));
        }
        let hover = resp.hover_pos().filter(|p| v.img.contains(*p));
        if let Some(pos) = hover {
            match self.spot_hit(v, pos) {
                Some((_, true)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe),
                Some((_, false)) => ui.ctx().set_cursor_icon(CursorIcon::Move),
                None => {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                    painter.circle_stroke(
                        pos,
                        v.len_px(self.eye_r),
                        line(1.0, Color32::from_white_alpha(160)),
                    );
                }
            }
        }

        let origin = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started()
            && let Some(pos) = origin
        {
            self.drag = match self.spot_hit(v, pos) {
                Some((idx, true)) => {
                    self.sel_spot = Some(idx);
                    Some(Drag::SpotSize {
                        idx,
                        created: false,
                    })
                }
                Some((idx, false)) => {
                    self.sel_spot = Some(idx);
                    let s = &self.spots[idx];
                    v.to_src(pos).map(|p| Drag::SpotMove {
                        idx,
                        grab: [p[0] - s.x, p[1] - s.y],
                    })
                }
                None => self
                    .place_spot(v, pos)
                    .map(|idx| Drag::SpotSize { idx, created: true }),
            };
        }
        if resp.dragged()
            && let (Some(drag), Some(pos)) = (self.drag.clone(), resp.interact_pointer_pos())
        {
            match drag {
                Drag::SpotMove { idx, grab } => {
                    if let (Some(p), Some(s)) = (v.to_src(pos), self.spots.get_mut(idx)) {
                        (s.x, s.y) = (p[0] - grab[0], p[1] - grab[1]);
                    }
                }
                Drag::SpotSize { idx, created } => {
                    if let Some(s) = self.spots.get_mut(idx) {
                        let d = v.to_screen([s.x, s.y]).distance(pos);
                        if d > DRAG_MIN || !created {
                            s.r = v.frac(d).clamp(0.001, 0.1);
                        }
                    }
                }
                _ => {}
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::SpotSize { idx, .. }) = self.drag
                && let Some(s) = self.spots.get(idx)
            {
                self.eye_r = s.r;
            }
            self.drag = None;
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
            && v.img.contains(pos)
        {
            match self.spot_hit(v, pos) {
                Some((idx, _)) => self.sel_spot = Some(idx),
                None => {
                    self.place_spot(v, pos);
                }
            }
        }
    }

    fn place_spot(&mut self, v: &View, pos: Pos2) -> Option<usize> {
        if self.spots.len() >= MAX_SPOTS {
            return None;
        }
        let p = v.to_src(pos)?;
        self.spots.push(EyeSpot {
            x: p[0],
            y: p[1],
            r: self.eye_r,
            mode: self.eye_mode,
            darken: self.eye_darken,
            catchlight: self.eye_catchlight,
        });
        self.sel_spot = Some(self.spots.len() - 1);
        Some(self.spots.len() - 1)
    }

    // --- Canvas: gradient ----------------------------------------------------

    fn gradient_canvas(&mut self, ui: &Ui, v: &View, resp: &Response) {
        let painter = ui.painter_at(v.img.expand(2.0));
        let reach = v.img.size().length();
        let selected = self.selected();
        for (i, a) in self.adjustments.iter().enumerate() {
            let MaskDef::Linear(m) = &a.mask else {
                continue;
            };
            let pin = v.to_screen([m.x, m.y]);
            let sel = Some(i) == selected;
            let pin_col = if sel {
                Color32::WHITE
            } else {
                Color32::from_white_alpha(150)
            };
            painter.circle_filled(pin, PIN * 0.6, Color32::BLACK);
            painter.circle_stroke(pin, PIN * 0.6, line(2.0, pin_col));
            if !sel {
                continue;
            }
            let (n, d) = v.line_frame(m);
            let half = v.len_px(m.feather) / 2.0;
            let strong = line(1.5, Color32::WHITE);
            let shadow = line(3.0, Color32::from_black_alpha(140));
            for line in [shadow, strong] {
                painter.line_segment([pin - d * reach, pin + d * reach], line);
            }
            for side in [-1.0, 1.0] {
                let o = n * (half * side);
                dashed(&painter, pin + o - d * reach, pin + o + d * reach, strong);
            }
            // The affected side.
            let tip = pin + n * 26.0;
            painter.line_segment([pin, tip], line(2.0, Color32::WHITE));
            painter.circle_filled(tip, 3.5, Color32::WHITE);
            // Rotation knob.
            let knob = pin + d * KNOB_DIST;
            painter.circle_filled(knob, 7.0, Color32::BLACK);
            painter.circle_stroke(knob, 7.0, line(2.0, Color32::WHITE));
        }

        let hover = resp.hover_pos().filter(|p| v.img.contains(*p));
        if let Some(pos) = hover {
            let icon = match self.gradient_hit(v, pos) {
                Some(Drag::GradMove) => CursorIcon::Move,
                Some(Drag::GradRotate) => CursorIcon::Grab,
                Some(Drag::GradFeather) => CursorIcon::ResizeVertical,
                _ => CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        let origin = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started()
            && let Some(pos) = origin
        {
            self.drag = match self.gradient_hit(v, pos) {
                Some(d) => Some(d),
                None => {
                    if self.at_limit() {
                        None
                    } else {
                        v.to_src(pos).map(|p| {
                            let name = self.name_for("Gradient", false);
                            self.add(
                                name,
                                MaskDef::Linear(LinearMask {
                                    x: p[0],
                                    y: p[1],
                                    angle: 0.0,
                                    feather: 0.1,
                                }),
                            );
                            Drag::GradNew { origin: pos }
                        })
                    }
                }
            };
        }
        if resp.dragged()
            && let (Some(drag), Some(pos), Some(i)) = (
                self.drag.clone(),
                resp.interact_pointer_pos(),
                self.selected(),
            )
        {
            let shift = ui.input(|i| i.modifiers.shift);
            let src_pos = v.to_src(pos);
            let probe = match &self.adjustments[i].mask {
                MaskDef::Linear(m) => Some(*m),
                MaskDef::Brush { .. } => None,
            };
            if let Some(m0) = probe {
                let pin = v.to_screen([m0.x, m0.y]);
                let new = match drag {
                    Drag::GradMove => src_pos.map(|p| LinearMask {
                        x: p[0],
                        y: p[1],
                        ..m0
                    }),
                    Drag::GradNew { origin } => {
                        let delta = pos - origin;
                        (delta.length() > DRAG_MIN).then(|| LinearMask {
                            angle: v.angle_for_normal(delta.normalized()),
                            feather: (v.frac(delta.length() * 2.0)).clamp(0.01, 1.0),
                            ..m0
                        })
                    }
                    Drag::GradRotate => {
                        let mut dir = pos - pin;
                        if dir.length() < DRAG_MIN {
                            None
                        } else {
                            if shift {
                                let a = (dir.y.atan2(dir.x).to_degrees() / 15.0).round() * 15.0;
                                dir = Vec2::angled(a.to_radians());
                            }
                            let dir = dir.normalized();
                            // Keep the affected side on the same hand.
                            let (n0, d0) = v.line_frame(&m0);
                            let hand = (d0.x * n0.y - d0.y * n0.x).signum();
                            let n = Vec2::new(-dir.y, dir.x) * hand;
                            Some(LinearMask {
                                angle: v.angle_for_normal(n),
                                ..m0
                            })
                        }
                    }
                    Drag::GradFeather => {
                        let (n, _) = v.line_frame(&m0);
                        let d = (pos - pin).dot(n).abs();
                        Some(LinearMask {
                            feather: v.frac(d * 2.0).clamp(0.01, 1.0),
                            ..m0
                        })
                    }
                    _ => None,
                };
                if let Some(m) = new {
                    self.adjustments[i].mask = MaskDef::Linear(m);
                }
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            self.pick_pin(v, pos);
        }
    }

    /// What a press at `pos` would grab on the selected gradient.
    fn gradient_hit(&self, v: &View, pos: Pos2) -> Option<Drag> {
        if let Some(i) = self.selected()
            && let MaskDef::Linear(m) = &self.adjustments[i].mask
        {
            let pin = v.to_screen([m.x, m.y]);
            let (n, d) = v.line_frame(m);
            if (pin + d * KNOB_DIST).distance(pos) <= PIN {
                return Some(Drag::GradRotate);
            }
            if pin.distance(pos) <= PIN {
                return Some(Drag::GradMove);
            }
            let half = v.len_px(m.feather) / 2.0;
            for side in [-1.0, 1.0] {
                if dist_to_line(pos, pin + n * (half * side), n) <= EDGE_GRAB {
                    return Some(Drag::GradFeather);
                }
            }
        }
        None
    }

    fn pin_at(&self, v: &View, pos: Pos2) -> Option<String> {
        self.adjustments.iter().find_map(|a| {
            let p = match &a.mask {
                MaskDef::Linear(m) => [m.x, m.y],
                MaskDef::Brush { strokes } => *strokes.first()?.points.first()?,
            };
            (v.to_screen(p).distance(pos) <= PIN).then(|| a.id.clone())
        })
    }

    fn pick_pin(&mut self, v: &View, pos: Pos2) {
        if let Some(id) = self.pin_at(v, pos) {
            if let Some(a) = self.adjustments.iter().find(|a| a.id == id) {
                self.kind = match a.mask {
                    MaskDef::Linear(_) => ToolId::Gradient,
                    MaskDef::Brush { .. } => ToolId::Brush,
                };
            }
            self.sel = Some(id);
        }
    }

    // --- Canvas: brush -------------------------------------------------------

    fn brush_canvas(&mut self, ui: &Ui, v: &View, resp: &Response) {
        let painter = ui.painter_at(v.img.expand(2.0));
        let selected = self.selected();
        for (i, a) in self.adjustments.iter().enumerate() {
            if let MaskDef::Brush { strokes } = &a.mask
                && let Some(p) = strokes.first().and_then(|s| s.points.first())
            {
                let pin = v.to_screen(*p);
                let col = if Some(i) == selected {
                    Color32::WHITE
                } else {
                    Color32::from_white_alpha(150)
                };
                painter.circle_filled(pin, PIN * 0.6, Color32::BLACK);
                painter.circle_stroke(pin, PIN * 0.6, line(2.0, col));
            }
        }
        let alt = ui.input(|i| i.modifiers.alt);
        let radius = v.len_px(self.brush.size) / 2.0;
        if let Some(pos) = resp.hover_pos().filter(|p| v.img.contains(*p)) {
            ui.ctx().set_cursor_icon(CursorIcon::None);
            let col = if alt {
                Color32::from_rgb(255, 120, 120)
            } else {
                Color32::WHITE
            };
            painter.circle_stroke(pos, radius, line(3.0, Color32::from_black_alpha(130)));
            painter.circle_stroke(pos, radius, line(1.2, col));
            painter.circle_stroke(
                pos,
                radius * (1.0 - self.brush.feather as f32),
                line(1.0, col.gamma_multiply(0.6)),
            );
        }

        let origin = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started()
            && let Some(pos) = origin
            && self.begin_stroke(v, pos, alt)
        {
            self.drag = Some(Drag::Paint { last: pos });
        }
        if resp.dragged()
            && let Some(Drag::Paint { last }) = self.drag
            && let Some(pos) = resp.interact_pointer_pos()
            && last.distance(pos) >= (radius / 4.0).max(1.0)
        {
            if let Some(p) = v.to_src(pos) {
                self.extend_stroke(p);
            }
            self.drag = Some(Drag::Paint { last: pos });
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
            && v.img.contains(pos)
        {
            if self.pin_at(v, pos).is_some() {
                self.pick_pin(v, pos);
            } else {
                self.begin_stroke(v, pos, alt);
            }
        }
    }

    /// Starts a stroke (a dab when it never moves) in the selected zone,
    /// creating one first when none is selected.
    fn begin_stroke(&mut self, v: &View, pos: Pos2, erase: bool) -> bool {
        let Some(p) = v.to_src(pos) else {
            return false;
        };
        let idx = match self
            .selected()
            .filter(|i| matches!(self.adjustments[*i].mask, MaskDef::Brush { .. }))
        {
            Some(i) => i,
            None => {
                if self.at_limit() {
                    return false;
                }
                let name = self.name_for("Zone", true);
                self.show_mask = true;
                self.add(
                    name,
                    MaskDef::Brush {
                        strokes: Vec::new(),
                    },
                )
            }
        };
        if let MaskDef::Brush { strokes } = &mut self.adjustments[idx].mask {
            strokes.push(Stroke {
                erase,
                points: vec![p],
                ..self.brush.clone()
            });
        }
        true
    }

    fn extend_stroke(&mut self, p: [f64; 2]) {
        if let Some(i) = self.selected()
            && let MaskDef::Brush { strokes } = &mut self.adjustments[i].mask
            && let Some(s) = strokes.last_mut()
        {
            s.points.push(p);
        }
    }

    // --- Options -------------------------------------------------------------

    fn red_eye_options(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let sel_mode = self
                .sel_spot
                .and_then(|i| self.spots.get(i))
                .map_or(self.eye_mode, |s| s.mode);
            for (mode, name) in [(EyeMode::Red, "Red Eye"), (EyeMode::Pet, "Pet Eye")] {
                if ui.selectable_label(sel_mode == mode, name).clicked() {
                    self.eye_mode = mode;
                    if let Some(s) = self.sel_spot.and_then(|i| self.spots.get_mut(i)) {
                        s.mode = mode;
                    }
                }
            }
        });
        let idx = self.sel_spot.filter(|i| *i < self.spots.len());
        let mut size = idx.map_or(self.eye_r, |i| self.spots[i].r) * 100.0;
        if spec_slider(ui, &SIZE, &mut size) {
            self.eye_r = size / 100.0;
            if let Some(i) = idx {
                self.spots[i].r = size / 100.0;
            }
        }
        let mut darken = idx.map_or(self.eye_darken, |i| self.spots[i].darken);
        if spec_slider(ui, &DARKEN, &mut darken) {
            self.eye_darken = darken;
            if let Some(i) = idx {
                self.spots[i].darken = darken;
            }
        }
        let pet = idx.map_or(self.eye_mode, |i| self.spots[i].mode) == EyeMode::Pet;
        if pet {
            let mut c = idx.map_or(self.eye_catchlight, |i| self.spots[i].catchlight);
            if ui.checkbox(&mut c, "Add Catchlight").changed() {
                self.eye_catchlight = c;
                if let Some(i) = idx {
                    self.spots[i].catchlight = c;
                }
            }
        }
        ui.small(if pet {
            "Click or drag to place a circle over the pupil; size it to fit."
        } else {
            "Click an eye to place a circle; drag its edge to resize."
        });
    }

    fn adjustment_options(&mut self, ui: &mut Ui) {
        let brush = self.kind == ToolId::Brush;
        if brush {
            let mut size = self.brush.size * 100.0;
            let mut feather = self.brush.feather * 100.0;
            let mut flow = self.brush.flow * 100.0;
            if spec_slider(ui, &BRUSH_SIZE, &mut size) {
                self.brush.size = size / 100.0;
            }
            if spec_slider(ui, &FEATHER, &mut feather) {
                self.brush.feather = feather / 100.0;
            }
            if spec_slider(ui, &FLOW, &mut flow) {
                self.brush.flow = flow / 100.0;
            }
            ui.small("Paint on the photo; hold Alt to erase. [ ] size, Shift+[ ] feather.");
            ui.separator();
        } else {
            ui.small("Drag on the photo to place a gradient; the drag length sets the feather.");
        }
        if let Some(i) = self.selected() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(self.adjustments[i].name.clone()).strong());
                if !brush && ui.button("Flip").on_hover_text("'").clicked() {
                    self.flip();
                }
            });
            for (k, spec) in TONE.iter().enumerate() {
                if let Some(a) = self.adjustments.get_mut(i) {
                    let mut v = tone_get(a, k);
                    if spec_slider(ui, spec, &mut v) {
                        tone_set(a, k, v);
                    }
                }
            }
            ui.checkbox(&mut self.show_mask, "Show mask (O)");
        }
        if !self.adjustments.is_empty() {
            ui.add_space(4.0);
            ui.label("Local adjustments");
            let mut pick = None;
            for a in &mut self.adjustments {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut a.enabled, "");
                    if ui
                        .selectable_label(Some(&a.id) == self.sel.as_ref(), &a.name)
                        .clicked()
                    {
                        pick = Some((a.id.clone(), matches!(a.mask, MaskDef::Brush { .. })));
                    }
                });
            }
            if let Some((id, is_brush)) = pick {
                self.sel = Some(id);
                self.kind = if is_brush {
                    ToolId::Brush
                } else {
                    ToolId::Gradient
                };
            }
        }
    }
}

impl CanvasTool for LocalTool {
    fn id(&self) -> ToolId {
        self.kind
    }

    fn mask_overlay(&self) -> Option<String> {
        if self.kind == ToolId::RedEye || !self.show_mask {
            return None;
        }
        self.sel.clone()
    }

    fn display_params(&self, doc: &EditParams) -> EditParams {
        let mut p = doc.clone();
        p.set::<RedEye>(RedEyeParams {
            spots: self.spots.clone(),
        });
        p.set_local_draft(&self.adjustments);
        p
    }

    fn canvas(
        &mut self,
        ui: &Ui,
        img: Rect,
        resp: &Response,
        geom: &Geometry,
        _params: &EditParams,
    ) -> Option<Outcome> {
        let v = View { img, geom };
        let before = (self.spots.clone(), self.adjustments.clone());
        match self.kind {
            ToolId::RedEye => self.red_eye_canvas(ui, &v, resp),
            ToolId::Gradient => self.gradient_canvas(ui, &v, resp),
            _ => self.brush_canvas(ui, &v, resp),
        }
        if before != (self.spots.clone(), self.adjustments.clone()) {
            ui.ctx().request_repaint();
        }
        None
    }

    fn options(
        &mut self,
        ui: &mut Ui,
        _params: &EditParams,
        _geom: &Geometry,
    ) -> (Option<Outcome>, ToolAction) {
        let mut action = ToolAction::None;
        ui.horizontal(|ui| {
            ui.heading(self.kind.label());
        });
        match self.kind {
            ToolId::RedEye => self.red_eye_options(ui),
            _ => self.adjustment_options(ui),
        }
        if let Some(n) = self.note {
            ui.colored_label(Color32::from_rgb(255, 190, 40), n);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Apply").on_hover_text("Enter").clicked() {
                action = ToolAction::Apply;
            }
            if ui.button("Cancel").on_hover_text("Esc").clicked() {
                action = ToolAction::Cancel;
            }
            let can_delete = match self.kind {
                ToolId::RedEye => self.sel_spot.is_some(),
                _ => self.selected().is_some(),
            };
            if ui
                .add_enabled(can_delete, egui::Button::new("Delete"))
                .clicked()
            {
                self.delete();
            }
        });
        (None, action)
    }

    fn on_key(
        &mut self,
        key: Key,
        shift: bool,
        _params: &EditParams,
        _geom: &Geometry,
    ) -> Option<Outcome> {
        match key {
            Key::O => self.show_mask = !self.show_mask,
            Key::Quote => self.flip(),
            Key::OpenBracket | Key::CloseBracket => {
                let up = key == Key::CloseBracket;
                if shift {
                    let f = self.brush.feather + if up { 0.1 } else { -0.1 };
                    self.brush.feather = f.clamp(0.0, 1.0);
                } else {
                    let s = self.brush.size * if up { 1.15 } else { 1.0 / 1.15 };
                    self.brush.size = s.clamp(0.002, 0.3);
                }
            }
            Key::Delete | Key::Backspace => self.delete(),
            _ => {}
        }
        None
    }

    fn finish(&mut self, apply: bool, doc: &EditParams) -> Option<Outcome> {
        if !apply {
            return None;
        }
        let mut p = doc.clone();
        p.set::<RedEye>(RedEyeParams {
            spots: self.spots.clone(),
        });
        p.set_local_adjustments(&self.adjustments);
        if p == *doc {
            return None;
        }
        let kept: Vec<LocalAdjustment> = p.local_adjustments();
        let label = if self.spots != self.orig_spots && kept == self.orig {
            "Red Eye"
        } else {
            self.label(&kept)
        };
        Outcome::new(label, p, true)
    }

    fn switch_to(&mut self, to: ToolId) -> bool {
        if to == ToolId::Crop {
            return false;
        }
        self.kind = to;
        self.drag = None;
        self.note = None;
        // Keep the selection only if it fits the new tool.
        if let Some(i) = self.selected() {
            let brush = matches!(self.adjustments[i].mask, MaskDef::Brush { .. });
            if brush != (to == ToolId::Brush) {
                self.sel = None;
            }
        }
        true
    }
}
