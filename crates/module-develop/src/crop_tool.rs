//! The crop & straighten tool (plan §8.3): the first canvas tool. While it
//! is open the pipeline renders the whole straightened canvas
//! (`RenderRequest::ignore_crop`) and this draws the crop rectangle, its
//! handles and a composition overlay on top. All rectangles are normalised
//! to the canvas, like the `crop` op's.

use egui::{Color32, CursorIcon, Pos2, Rect, Stroke};
use viberoom_services::engine::EditParams;
use viberoom_services::engine::Op;
use viberoom_services::engine::geometry::{
    MIN_CROP, constrain_between, fit_rect, level_angle, mirror_rect, rotate_rect_cw, sanitize,
};
use viberoom_services::engine::ops::{Crop, Straighten, StraightenParams};

use viberoom_services::engine::geometry::Geometry;

use crate::basic::spec_slider;
use crate::tool::{CanvasTool, Outcome, ToolAction, ToolId};

const HANDLE: f32 = 9.0;
const OVERLAYS: [&str; 4] = ["Rule of Thirds", "Grid", "Golden Ratio", "None"];

/// Aspect presets as width:height (`[0, 0]` = free).
const PRESETS: [(&str, [f64; 2]); 5] = [
    ("1:1", [1.0, 1.0]),
    ("4:5", [4.0, 5.0]),
    ("3:2", [3.0, 2.0]),
    ("16:9", [16.0, 9.0]),
    ("5:4", [5.0, 4.0]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
    Tl,
    T,
    Tr,
    R,
    Br,
    B,
    Bl,
    L,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Move {
        start: [f64; 4],
        grab: [f64; 2],
    },
    Resize {
        handle: Handle,
        start: [f64; 4],
    },
    Level {
        from: Pos2,
    },
    /// The image turns under the upright box as the pointer sweeps around
    /// the box's centre (plan v0.2.0 §4.5).
    Rotate {
        start_rect: [f64; 4],
        start_angle: f64,
        start_pointer: f32,
    },
}

/// How far out from a corner (screen px) the rotate zone reaches.
const ROTATE_REACH: f32 = 40.0;
/// Below this angle (degrees) a rotation snaps to level.
const LEVEL_SNAP: f64 = 0.3;

/// The angle of `pos` around `center`, in degrees (y down, clockwise).
fn pointer_angle(center: Pos2, pos: Pos2) -> f32 {
    (pos.y - center.y).atan2(pos.x - center.x).to_degrees()
}

/// The straighten angle and crop box for a rotate drag: the image turns by
/// the angle the pointer swept (clockwise = clockwise), Shift snaps to whole
/// degrees, and the box shrinks about its centre to stay inside the image,
/// always measured from the box as the drag began so it grows back when the
/// angle returns toward 0.
pub fn rotate_drag(
    start_angle: f64,
    start_rect: [f64; 4],
    canvas: (u32, u32),
    start_pointer: f32,
    pointer: f32,
    snap_whole: bool,
) -> (f64, [f64; 4]) {
    let mut sweep = f64::from(pointer - start_pointer);
    if sweep > 180.0 {
        sweep -= 360.0;
    } else if sweep < -180.0 {
        sweep += 360.0;
    }
    let mut angle = (start_angle + sweep).clamp(-45.0, 45.0);
    if snap_whole {
        angle = angle.round();
    }
    if angle.abs() < LEVEL_SNAP {
        angle = 0.0;
    }
    (angle, fit_rect(angle, canvas, start_rect))
}

#[derive(Debug)]
pub struct CropTool {
    overlay: usize,
    drag: Option<Drag>,
    level: bool,
    level_line: Option<(Pos2, Pos2)>,
    /// A double click inside the box: apply the crop and close the tool.
    close_requested: bool,
    /// Aspect lock as width:height; `[0, 0]` is free.
    aspect: [f64; 2],
    custom: [f64; 2],
}

fn lerp_pos(rect: Rect, p: [f64; 2]) -> Pos2 {
    Pos2::new(
        rect.left() + p[0] as f32 * rect.width(),
        rect.top() + p[1] as f32 * rect.height(),
    )
}

fn to_norm(rect: Rect, pos: Pos2) -> [f64; 2] {
    [
        f64::from((pos.x - rect.left()) / rect.width()),
        f64::from((pos.y - rect.top()) / rect.height()),
    ]
}

/// Normalised width/height a rectangle needs for the pixel aspect `a` on a
/// `canvas`-sized frame.
fn ratio_norm(a: [f64; 2], canvas: (u32, u32)) -> Option<f64> {
    (a[0] > 0.0 && a[1] > 0.0).then(|| a[0] / a[1] * f64::from(canvas.1) / f64::from(canvas.0))
}

/// The biggest rectangle of normalised ratio `r` inside `rect`, centred on it.
fn fit_ratio(rect: [f64; 4], r: f64) -> [f64; 4] {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let (nw, nh) = if w / h > r { (h * r, h) } else { (w, w / r) };
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    [cx - nw / 2.0, cy - nh / 2.0, cx + nw / 2.0, cy + nh / 2.0]
}

fn resize(handle: Handle, start: [f64; 4], p: [f64; 2], ratio: Option<f64>) -> [f64; 4] {
    let p = [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)];
    let [x0, y0, x1, y1] = start;
    let mut r = start;
    match handle {
        Handle::Tl => (r[0], r[1]) = (p[0], p[1]),
        Handle::T => r[1] = p[1],
        Handle::Tr => (r[2], r[1]) = (p[0], p[1]),
        Handle::R => r[2] = p[0],
        Handle::Br => (r[2], r[3]) = (p[0], p[1]),
        Handle::B => r[3] = p[1],
        Handle::Bl => (r[0], r[3]) = (p[0], p[1]),
        Handle::L => r[0] = p[0],
    }
    if let Some(ratio) = ratio {
        let corner = |anchor: [f64; 2]| {
            let (mut w, mut h) = ((p[0] - anchor[0]).abs(), (p[1] - anchor[1]).abs());
            if w / h.max(1e-9) > ratio {
                w = h * ratio;
            } else {
                h = w / ratio;
            }
            let sx = if p[0] >= anchor[0] { 1.0 } else { -1.0 };
            let sy = if p[1] >= anchor[1] { 1.0 } else { -1.0 };
            let (ex, ey) = (anchor[0] + sx * w, anchor[1] + sy * h);
            [
                anchor[0].min(ex),
                anchor[1].min(ey),
                anchor[0].max(ex),
                anchor[1].max(ey),
            ]
        };
        r = match handle {
            Handle::Tl => corner([x1, y1]),
            Handle::Tr => corner([x0, y1]),
            Handle::Br => corner([x0, y0]),
            Handle::Bl => corner([x1, y0]),
            Handle::T | Handle::B => {
                let h = r[3] - r[1];
                let cx = (x0 + x1) / 2.0;
                [cx - h * ratio / 2.0, r[1], cx + h * ratio / 2.0, r[3]]
            }
            Handle::L | Handle::R => {
                let w = r[2] - r[0];
                let cy = (y0 + y1) / 2.0;
                [r[0], cy - w / ratio / 2.0, r[2], cy + w / ratio / 2.0]
            }
        };
    }
    sanitize(r)
}

/// Just outside a corner of the crop box (the rotate zone).
fn in_rotate_zone(crop_rect: Rect, pos: Pos2) -> bool {
    !crop_rect.contains(pos)
        && [
            crop_rect.left_top(),
            crop_rect.right_top(),
            crop_rect.left_bottom(),
            crop_rect.right_bottom(),
        ]
        .iter()
        .any(|c| c.distance(pos) <= ROTATE_REACH)
}

fn handle_cursor(h: Handle) -> CursorIcon {
    match h {
        Handle::Tl | Handle::Br => CursorIcon::ResizeNwSe,
        Handle::Tr | Handle::Bl => CursorIcon::ResizeNeSw,
        Handle::T | Handle::B => CursorIcon::ResizeVertical,
        Handle::L | Handle::R => CursorIcon::ResizeHorizontal,
    }
}

/// `params` with the crop rectangle, aspect and (optionally) angle replaced.
fn with_geometry(
    params: &EditParams,
    rect: Option<[f64; 4]>,
    angle: Option<f64>,
    aspect: [f64; 2],
) -> EditParams {
    let mut p = params.clone();
    let mut crop = p.get::<Crop>();
    if let Some(r) = rect {
        crop.set_rect(r);
    }
    crop.aspect = aspect;
    p.set::<Crop>(crop);
    if let Some(a) = angle {
        p.set::<Straighten>(StraightenParams { angle: a });
    }
    p
}

impl CropTool {
    pub fn new(params: &EditParams) -> Self {
        Self {
            overlay: 0,
            drag: None,
            level: false,
            level_line: None,
            close_requested: false,
            aspect: params.get::<Crop>().aspect,
            custom: [4.0, 3.0],
        }
    }

    pub fn cycle_overlay(&mut self) {
        self.overlay = (self.overlay + 1) % OVERLAYS.len();
    }

    fn outcome(label: &str, params: EditParams, immediate: bool) -> Option<Outcome> {
        Outcome::new(label, params, immediate)
    }

    /// Locks (or frees) the aspect and fits the current rectangle to it.
    fn set_aspect(
        &mut self,
        params: &EditParams,
        canvas: (u32, u32),
        aspect: [f64; 2],
    ) -> Option<Outcome> {
        self.aspect = aspect;
        let crop = params.get::<Crop>();
        let mut rect = crop.rect();
        let angle = params.get::<Straighten>().angle;
        if let Some(r) = ratio_norm(aspect, canvas) {
            rect = fit_rect(angle, canvas, fit_ratio(rect, r));
        }
        Self::outcome(
            "Crop Aspect",
            with_geometry(params, Some(rect), None, aspect),
            true,
        )
    }

    /// `X`: swaps a portrait crop for a landscape one (and back).
    fn swap_orientation(&mut self, params: &EditParams, canvas: (u32, u32)) -> Option<Outcome> {
        let crop = params.get::<Crop>();
        let [x0, y0, x1, y1] = crop.rect();
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (w_px, h_px) = (
            (x1 - x0) * f64::from(canvas.0),
            (y1 - y0) * f64::from(canvas.1),
        );
        // Swap the pixel dimensions about the centre.
        let (nw, nh) = (h_px / f64::from(canvas.0), w_px / f64::from(canvas.1));
        let rect = [cx - nw / 2.0, cy - nh / 2.0, cx + nw / 2.0, cy + nh / 2.0];
        let angle = params.get::<Straighten>().angle;
        let rect = fit_rect(angle, canvas, sanitize(rect));
        self.aspect = [self.aspect[1], self.aspect[0]];
        Self::outcome(
            "Crop Orientation",
            with_geometry(params, Some(rect), None, self.aspect),
            true,
        )
    }

    fn rotate(&mut self, params: &EditParams, cw: bool) -> Option<Outcome> {
        let mut p = params.clone();
        let mut crop = p.get::<Crop>();
        let step = if crop.flip_h { -1 } else { 1 };
        let (turns, dq) = if cw { (1, step) } else { (3, -step) };
        let mut rect = crop.rect();
        for _ in 0..turns {
            rect = rotate_rect_cw(rect);
        }
        crop.quarters = (crop.quarters + dq).rem_euclid(4);
        crop.set_rect(rect);
        self.aspect = [self.aspect[1], self.aspect[0]];
        crop.aspect = self.aspect;
        p.set::<Crop>(crop);
        Self::outcome("Rotate", p, true)
    }

    fn flip(&mut self, params: &EditParams, vertical: bool) -> Option<Outcome> {
        let mut p = params.clone();
        let mut crop = p.get::<Crop>();
        let mut rect = crop.rect();
        if vertical {
            // Flip V = flip H after a half turn.
            rect = [rect[0], 1.0 - rect[3], rect[2], 1.0 - rect[1]];
            crop.quarters = (crop.quarters + 2).rem_euclid(4);
        } else {
            rect = mirror_rect(rect);
        }
        crop.flip_h = !crop.flip_h;
        crop.set_rect(rect);
        p.set::<Crop>(crop);
        let angle = p.get::<Straighten>().angle;
        p.set::<Straighten>(StraightenParams { angle: -angle });
        Self::outcome("Flip", p, true)
    }

    fn reset(&mut self, params: &EditParams) -> Option<Outcome> {
        self.aspect = [0.0, 0.0];
        let mut p = params.clone();
        // Keep the flip/rotate state: Reset restores the crop and angle.
        let mut crop = p.get::<Crop>();
        crop.set_rect([0.0, 0.0, 1.0, 1.0]);
        crop.aspect = [0.0, 0.0];
        p.set::<Crop>(crop);
        p.set::<Straighten>(StraightenParams::default());
        Self::outcome("Reset Crop", p, true)
    }

    /// The options block (right panel). Returns a change and whether the
    /// user pressed Done.
    fn crop_options(
        &mut self,
        ui: &mut egui::Ui,
        params: &EditParams,
        canvas: (u32, u32),
    ) -> (Option<Outcome>, bool) {
        let mut out: Option<Outcome> = None;
        let mut done = false;
        ui.horizontal(|ui| {
            ui.heading("Crop & Straighten");
            if ui.button("Done").on_hover_text("Enter / R").clicked() {
                done = true;
            }
        });

        let current = if self.aspect[0] <= 0.0 || self.aspect[1] <= 0.0 {
            "Free".to_string()
        } else if ratio_norm(self.aspect, canvas).is_some_and(|r| (r - 1.0).abs() < 1e-6) {
            "Original".to_string()
        } else {
            PRESETS.iter().find(|(_, a)| *a == self.aspect).map_or(
                format!("{}:{}", self.aspect[0], self.aspect[1]),
                |(n, _)| (*n).to_string(),
            )
        };
        ui.horizontal(|ui| {
            ui.label("Aspect");
            egui::ComboBox::from_id_salt("crop_aspect")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(false, "Free").clicked() {
                        out = self.set_aspect(params, canvas, [0.0, 0.0]);
                    }
                    if ui.selectable_label(false, "Original").clicked() {
                        out = self.set_aspect(
                            params,
                            canvas,
                            [f64::from(canvas.0), f64::from(canvas.1)],
                        );
                    }
                    for (name, a) in PRESETS {
                        if ui.selectable_label(false, name).clicked() {
                            out = self.set_aspect(params, canvas, a);
                        }
                    }
                });
            if ui
                .button("⇄")
                .on_hover_text("Swap portrait/landscape (X)")
                .clicked()
            {
                out = self.swap_orientation(params, canvas);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Custom");
            ui.add(
                egui::DragValue::new(&mut self.custom[0])
                    .range(1.0..=100.0)
                    .speed(0.1),
            );
            ui.label(":");
            ui.add(
                egui::DragValue::new(&mut self.custom[1])
                    .range(1.0..=100.0)
                    .speed(0.1),
            );
            if ui.button("Apply").clicked() {
                out = self.set_aspect(params, canvas, self.custom);
            }
        });

        let mut angle = params.get::<Straighten>().angle;
        if spec_slider(ui, &Straighten::specs()[0], &mut angle) {
            let crop = params.get::<Crop>();
            let rect = fit_rect(angle, canvas, crop.rect());
            out = Self::outcome(
                "Straighten",
                with_geometry(params, Some(rect), Some(angle), self.aspect),
                false,
            );
        }

        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(self.level, "Level")
                .on_hover_text("Drag a line along the horizon or a vertical edge")
                .clicked()
            {
                self.level = !self.level;
                self.level_line = None;
            }
            if ui.button("⟲").on_hover_text("Rotate left").clicked() {
                out = self.rotate(params, false);
            }
            if ui.button("⟳").on_hover_text("Rotate right").clicked() {
                out = self.rotate(params, true);
            }
            if ui.button("↔").on_hover_text("Flip horizontally").clicked() {
                out = self.flip(params, false);
            }
            if ui.button("↕").on_hover_text("Flip vertically").clicked() {
                out = self.flip(params, true);
            }
            if ui.button("Reset").clicked() {
                out = self.reset(params);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Overlay (O)");
            egui::ComboBox::from_id_salt("crop_overlay")
                .selected_text(OVERLAYS[self.overlay])
                .show_ui(ui, |ui| {
                    for (i, name) in OVERLAYS.iter().enumerate() {
                        ui.selectable_value(&mut self.overlay, i, *name);
                    }
                });
        });
        (out, done)
    }

    fn hit(&self, img: Rect, rect: [f64; 4], pos: Pos2) -> Option<Handle> {
        let tl = lerp_pos(img, [rect[0], rect[1]]);
        let br = lerp_pos(img, [rect[2], rect[3]]);
        let r = Rect::from_min_max(tl, br);
        let near = |a: Pos2| a.distance(pos) <= HANDLE + 4.0;
        let corners = [
            (Handle::Tl, r.left_top()),
            (Handle::Tr, r.right_top()),
            (Handle::Br, r.right_bottom()),
            (Handle::Bl, r.left_bottom()),
        ];
        if let Some((h, _)) = corners.iter().find(|(_, c)| near(*c)) {
            return Some(*h);
        }
        let band = 7.0;
        let inside_x = pos.x > r.left() + HANDLE && pos.x < r.right() - HANDLE;
        let inside_y = pos.y > r.top() + HANDLE && pos.y < r.bottom() - HANDLE;
        if inside_x && (pos.y - r.top()).abs() <= band {
            return Some(Handle::T);
        }
        if inside_x && (pos.y - r.bottom()).abs() <= band {
            return Some(Handle::B);
        }
        if inside_y && (pos.x - r.left()).abs() <= band {
            return Some(Handle::L);
        }
        if inside_y && (pos.x - r.right()).abs() <= band {
            return Some(Handle::R);
        }
        None
    }

    /// Draws the overlay over the canvas image at `img` and handles the
    /// pointer. `canvas` is the canvas size in pixels.
    fn crop_canvas(
        &mut self,
        ui: &egui::Ui,
        img: Rect,
        resp: &egui::Response,
        params: &EditParams,
        canvas: (u32, u32),
    ) -> Option<Outcome> {
        let crop = params.get::<Crop>();
        let rect = sanitize(crop.rect());
        let angle = params.get::<Straighten>().angle;
        let painter = ui.painter_at(img.expand(20.0));
        let crop_rect = Rect::from_min_max(
            lerp_pos(img, [rect[0], rect[1]]),
            lerp_pos(img, [rect[2], rect[3]]),
        );

        // Dim everything outside the crop.
        let dim = Color32::from_black_alpha(150);
        painter.rect_filled(
            Rect::from_min_max(img.left_top(), Pos2::new(img.right(), crop_rect.top())),
            0.0,
            dim,
        );
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(img.left(), crop_rect.bottom()),
                img.right_bottom(),
            ),
            0.0,
            dim,
        );
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(img.left(), crop_rect.top()),
                Pos2::new(crop_rect.left(), crop_rect.bottom()),
            ),
            0.0,
            dim,
        );
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(crop_rect.right(), crop_rect.top()),
                Pos2::new(img.right(), crop_rect.bottom()),
            ),
            0.0,
            dim,
        );

        // Composition overlay.
        let line = Stroke::new(1.0_f32, Color32::from_white_alpha(110));
        let rotating = matches!(self.drag, Some(Drag::Rotate { .. }));
        let fractions: &[f32] = match if rotating { 1 } else { self.overlay } {
            0 => &[1.0 / 3.0, 2.0 / 3.0],
            1 => &[1.0 / 6.0, 2.0 / 6.0, 3.0 / 6.0, 4.0 / 6.0, 5.0 / 6.0],
            2 => &[0.381_966, 0.618_034],
            _ => &[],
        };
        for f in fractions {
            let x = crop_rect.left() + f * crop_rect.width();
            let y = crop_rect.top() + f * crop_rect.height();
            painter.line_segment(
                [
                    Pos2::new(x, crop_rect.top()),
                    Pos2::new(x, crop_rect.bottom()),
                ],
                line,
            );
            painter.line_segment(
                [
                    Pos2::new(crop_rect.left(), y),
                    Pos2::new(crop_rect.right(), y),
                ],
                line,
            );
        }
        painter.rect_stroke(
            crop_rect,
            0.0,
            Stroke::new(1.5_f32, Color32::WHITE),
            egui::StrokeKind::Outside,
        );
        for c in [
            crop_rect.left_top(),
            crop_rect.right_top(),
            crop_rect.left_bottom(),
            crop_rect.right_bottom(),
            crop_rect.center_top(),
            crop_rect.center_bottom(),
            crop_rect.left_center(),
            crop_rect.right_center(),
        ] {
            painter.rect_filled(
                Rect::from_center_size(c, egui::vec2(8.0, 8.0)),
                1.0,
                Color32::WHITE,
            );
        }
        if let Some((a, b)) = self.level_line {
            painter.line_segment(
                [a, b],
                Stroke::new(2.0_f32, Color32::from_rgb(255, 210, 60)),
            );
            painter.circle_filled(a, 4.0, Color32::from_rgb(255, 210, 60));
            painter.circle_filled(b, 4.0, Color32::from_rgb(255, 210, 60));
        }

        // Pointer.
        let ratio = ratio_norm(self.aspect, canvas);
        let hover = resp.hover_pos();
        if self.level {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        } else if let Some(pos) = hover {
            if let Some(h) = self.hit(img, rect, pos) {
                ui.ctx().set_cursor_icon(handle_cursor(h));
            } else if crop_rect.contains(pos) {
                ui.ctx().set_cursor_icon(CursorIcon::Move);
            } else if in_rotate_zone(crop_rect, pos) {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
        }
        if matches!(self.drag, Some(Drag::Rotate { .. })) {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }

        let mut out = None;
        // Hit-test where the button went down: by the time egui reports the
        // drag the pointer has already travelled past its drag threshold.
        if resp.drag_started()
            && let Some(pos) = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
        {
            self.drag = Some(if self.level {
                Drag::Level { from: pos }
            } else if let Some(handle) = self.hit(img, rect, pos) {
                Drag::Resize {
                    handle,
                    start: rect,
                }
            } else if crop_rect.contains(pos) {
                Drag::Move {
                    start: rect,
                    grab: to_norm(img, pos),
                }
            } else if in_rotate_zone(crop_rect, pos) {
                Drag::Rotate {
                    start_rect: rect,
                    start_angle: angle,
                    start_pointer: pointer_angle(crop_rect.center(), pos),
                }
            } else {
                // Dragging outside starts a new crop from that point.
                let p = to_norm(img, pos);
                Drag::Resize {
                    handle: Handle::Br,
                    start: [p[0], p[1], p[0] + MIN_CROP, p[1] + MIN_CROP],
                }
            });
        }
        if resp.dragged()
            && let (Some(drag), Some(pos)) = (self.drag, resp.interact_pointer_pos())
        {
            match drag {
                Drag::Level { from } => self.level_line = Some((from, pos)),
                Drag::Rotate {
                    start_rect,
                    start_angle,
                    start_pointer,
                } => {
                    let shift = ui.input(|i| i.modifiers.shift);
                    let (new_angle, new_rect) = rotate_drag(
                        start_angle,
                        start_rect,
                        canvas,
                        start_pointer,
                        pointer_angle(crop_rect.center(), pos),
                        shift,
                    );
                    painter.text(
                        pos + egui::vec2(16.0, -16.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{new_angle:+.1}°"),
                        egui::FontId::proportional(14.0),
                        Color32::WHITE,
                    );
                    if new_angle != angle || new_rect != rect {
                        out = Self::outcome(
                            "Rotate Crop",
                            with_geometry(params, Some(new_rect), Some(new_angle), self.aspect),
                            false,
                        );
                    }
                }
                Drag::Move { start, grab } => {
                    let now = to_norm(img, pos);
                    let (dx, dy) = (now[0] - grab[0], now[1] - grab[1]);
                    let (w, h) = (start[2] - start[0], start[3] - start[1]);
                    let x0 = (start[0] + dx).clamp(0.0, 1.0 - w);
                    let y0 = (start[1] + dy).clamp(0.0, 1.0 - h);
                    let cand = [x0, y0, x0 + w, y0 + h];
                    let next = constrain_between(angle, canvas, rect, cand);
                    if next != rect {
                        out = Self::outcome(
                            "Crop",
                            with_geometry(params, Some(next), None, self.aspect),
                            false,
                        );
                    }
                }
                Drag::Resize { handle, start } => {
                    let cand = resize(handle, start, to_norm(img, pos), ratio);
                    let next = constrain_between(angle, canvas, rect, cand);
                    if next != rect {
                        out = Self::outcome(
                            "Crop",
                            with_geometry(params, Some(next), None, self.aspect),
                            false,
                        );
                    }
                }
            }
        }
        if resp.double_clicked()
            && !self.level
            && let Some(pos) = resp.interact_pointer_pos()
            && crop_rect.contains(pos)
            && self.hit(img, rect, pos).is_none()
        {
            self.close_requested = true;
        }
        if resp.double_clicked()
            && let Some(pos) = resp.interact_pointer_pos()
            && in_rotate_zone(crop_rect, pos)
            && angle != 0.0
        {
            let rect = fit_rect(0.0, canvas, crop.rect());
            out = Self::outcome(
                "Rotate Crop",
                with_geometry(params, Some(rect), Some(0.0), self.aspect),
                true,
            );
        }
        if resp.drag_stopped() {
            if let Some(Drag::Level { from }) = self.drag
                && let Some(to) = resp.interact_pointer_pos()
                && from.distance(to) > 12.0
            {
                let px = |p: Pos2| {
                    let n = to_norm(img, p);
                    (n[0] * f64::from(canvas.0), n[1] * f64::from(canvas.1))
                };
                let new_angle = level_angle(angle, px(from), px(to));
                let rect = fit_rect(new_angle, canvas, crop.rect());
                out = Self::outcome(
                    "Straighten",
                    with_geometry(params, Some(rect), Some(new_angle), self.aspect),
                    true,
                );
                self.level = false;
            }
            self.level_line = None;
            self.drag = None;
        }
        out
    }
}

impl CanvasTool for CropTool {
    fn id(&self) -> ToolId {
        ToolId::Crop
    }

    fn take_close_request(&mut self) -> bool {
        std::mem::take(&mut self.close_requested)
    }

    fn ignore_crop(&self) -> bool {
        true
    }

    fn canvas(
        &mut self,
        ui: &egui::Ui,
        img: Rect,
        resp: &egui::Response,
        geom: &Geometry,
        params: &EditParams,
    ) -> Option<Outcome> {
        self.crop_canvas(ui, img, resp, params, geom.canvas)
    }

    fn options(
        &mut self,
        ui: &mut egui::Ui,
        params: &EditParams,
        geom: &Geometry,
    ) -> (Option<Outcome>, ToolAction) {
        let (out, done) = self.crop_options(ui, params, geom.canvas);
        (
            out,
            if done {
                ToolAction::Apply
            } else {
                ToolAction::None
            },
        )
    }

    fn on_key(
        &mut self,
        key: egui::Key,
        _shift: bool,
        params: &EditParams,
        geom: &Geometry,
    ) -> Option<Outcome> {
        match key {
            egui::Key::O => {
                self.cycle_overlay();
                None
            }
            egui::Key::X => self.swap_orientation(params, geom.canvas),
            _ => None,
        }
    }

    fn finish(&mut self, _apply: bool, _doc: &EditParams) -> Option<Outcome> {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use viberoom_services::engine::ops::CropParams;

    #[test]
    fn a_locked_corner_drag_keeps_the_ratio_and_anchors_the_opposite_corner() {
        let r = resize(Handle::Br, [0.1, 0.1, 0.5, 0.5], [0.9, 0.6], Some(2.0));
        assert!(
            (r[0] - 0.1).abs() < 1e-9 && (r[1] - 0.1).abs() < 1e-9,
            "{r:?}"
        );
        assert!(((r[2] - r[0]) / (r[3] - r[1]) - 2.0).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn a_free_edge_drag_only_moves_that_edge() {
        let r = resize(Handle::R, [0.1, 0.2, 0.5, 0.6], [0.8, 0.9], None);
        assert_eq!(r, [0.1, 0.2, 0.8, 0.6]);
    }

    #[test]
    fn dragging_a_corner_past_the_opposite_one_flips_the_rectangle() {
        let r = resize(Handle::Br, [0.5, 0.5, 0.6, 0.6], [0.2, 0.3], Some(1.0));
        assert!(r[0] < r[2] && r[1] < r[3]);
        assert!((r[2] - 0.5).abs() < 1e-9 || (r[0] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn fit_ratio_centres_the_largest_rectangle_of_that_ratio() {
        let r = fit_ratio([0.0, 0.0, 1.0, 1.0], 2.0);
        assert!(((r[2] - r[0]) - 1.0).abs() < 1e-9 && ((r[3] - r[1]) - 0.5).abs() < 1e-9);
        assert!(((r[1] + r[3]) / 2.0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rotating_swaps_the_lock_and_tracks_the_quarter_state() {
        let mut tool = CropTool::new(&EditParams::default());
        tool.aspect = [3.0, 2.0];
        let out = tool.rotate(&EditParams::default(), true).unwrap();
        let c = out.params.get::<Crop>();
        assert_eq!(c.quarters, 1);
        assert_eq!(tool.aspect, [2.0, 3.0]);
        // A flipped image turns the other way in the canonical form.
        let mut flipped = EditParams::default();
        flipped.set::<Crop>(CropParams {
            flip_h: true,
            ..Default::default()
        });
        let out = tool.rotate(&flipped, true).unwrap();
        assert_eq!(out.params.get::<Crop>().quarters, 3);
    }

    #[test]
    fn a_rotate_drag_turns_the_image_with_the_pointer_and_keeps_the_box_inside() {
        let canvas = (300, 200);
        let full = [0.0, 0.0, 1.0, 1.0];
        // Pointer sweeps 10° clockwise: the angle follows, the box shrinks.
        let (a, r) = rotate_drag(0.0, full, canvas, 30.0, 40.0, false);
        assert!((a - 10.0).abs() < 1e-6);
        assert!(viberoom_services::engine::geometry::rect_inside(
            a, canvas, r
        ));
        assert!(r[2] - r[0] < 0.95);
        // Coming back to 0 gives the full box back (measured from the start).
        let (a, r) = rotate_drag(0.0, full, canvas, 30.0, 30.1, false);
        assert_eq!((a, r), (0.0, full), "light snap at level");
        // Shift snaps to whole degrees; the angle is clamped to ±45°.
        let (a, _) = rotate_drag(0.0, full, canvas, 0.0, 7.4, true);
        assert_eq!(a, 7.0);
        let (a, _) = rotate_drag(40.0, full, canvas, 0.0, 30.0, false);
        assert_eq!(a, 45.0);
        // Crossing ±180° on the pointer circle doesn't jump.
        let (a, _) = rotate_drag(0.0, full, canvas, 175.0, -175.0, false);
        assert!((a - 10.0).abs() < 1e-6);
    }
}
