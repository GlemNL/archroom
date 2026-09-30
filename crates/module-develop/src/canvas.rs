//! The Develop canvas (plan §8.3): the engine's output texture with zoom,
//! pan, before/after, clipping overlay and the white-balance eyedropper,
//! plus the toolbar and Develop's keyboard shortcuts.

use archroom_core::settings::ClickZoom;
use archroom_services::LEVEL_L2;
use archroom_services::engine::ops::{WbMode, WhiteBalance, WhiteBalanceParams};
use archroom_shell::AppCx;
use egui::Modifiers;

use crate::basic::Change;
use crate::copy_dialog::{CopyDialog, Mode};
use crate::{DevelopModule, Zoom};

/// How long the Fit/1:1/2:1 transition takes, in seconds.
const ZOOM_ANIM_SECS: f64 = 0.15;

/// A running zoom transition, from the image size and pan that were drawn
/// when the zoom changed.
#[derive(Debug, Clone, Copy)]
pub struct ZoomAnim {
    start: f64,
    from_size: egui::Vec2,
    from_pan: egui::Vec2,
}

fn zoom_from_click(z: ClickZoom) -> Option<Zoom> {
    match z {
        ClickZoom::Off => None,
        ClickZoom::OneToOne => Some(Zoom::OneToOne),
        ClickZoom::TwoToOne => Some(Zoom::TwoToOne),
    }
}

pub fn toolbar(ui: &mut egui::Ui, m: &mut DevelopModule, cx: &mut AppCx) {
    for (z, name) in [
        (Zoom::Fit, "Fit"),
        (Zoom::OneToOne, "1:1"),
        (Zoom::TwoToOne, "2:1"),
    ] {
        if ui.selectable_label(m.zoom == z, name).clicked() {
            m.zoom = z;
            m.pan = egui::Vec2::ZERO;
        }
    }
    archroom_shell::click_zoom_picker(ui, cx);
    ui.separator();
    let mut tool_on = m.crop_tool.is_some();
    if ui
        .add_enabled(m.doc.is_some(), egui::Button::selectable(tool_on, "Crop"))
        .on_hover_text("Crop & straighten (R)")
        .clicked()
    {
        tool_on = !tool_on;
    }
    if tool_on != m.crop_tool.is_some() {
        m.toggle_crop_tool();
    }
    ui.separator();
    ui.toggle_value(&mut m.clip, "Clipping")
        .on_hover_text("Show clipped highlights and shadows (J)");
    ui.toggle_value(&mut m.before, "Before")
        .on_hover_text("Show the original (\\)");
    ui.separator();
    let (can_undo, can_redo) = m.doc.as_ref().map_or((false, false), |d| {
        (
            archroom_services::develop::undo_target(&d.history, d.cursor).is_some(),
            archroom_services::develop::redo_target(&d.history, d.cursor).is_some(),
        )
    });
    if ui
        .add_enabled(can_undo, egui::Button::new("↶"))
        .on_hover_text("Undo (Ctrl+Z)")
        .clicked()
    {
        m.walk(cx, false);
    }
    if ui
        .add_enabled(can_redo, egui::Button::new("↷"))
        .on_hover_text("Redo (Ctrl+Y)")
        .clicked()
    {
        m.walk(cx, true);
    }
}

/// True only while a `TextEdit` has focus. `wants_keyboard_input` is true
/// for any focused widget, and buttons take focus when clicked, which would
/// silence every shortcut after a single click (same fix as Library's).
pub fn typing_in_text_field(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

/// Develop's single-key and Ctrl shortcuts (plan §10.3), ignored while a
/// text field has focus.
fn handle_keys(ctx: &egui::Context, m: &mut DevelopModule, cx: &mut AppCx) {
    if typing_in_text_field(ctx) {
        return;
    }
    use egui::Key;
    let cmd = Modifiers::COMMAND;
    let keys = ctx.input_mut(|i| {
        [
            i.consume_key(Modifiers::NONE, Key::J),
            i.consume_key(Modifiers::NONE, Key::W),
            i.consume_key(Modifiers::NONE, Key::V),
            i.consume_key(Modifiers::NONE, Key::Z),
            i.consume_key(Modifiers::NONE, Key::Backslash),
            i.consume_key(cmd, Key::U),
            i.consume_key(cmd | Modifiers::SHIFT, Key::U),
            i.consume_key(cmd | Modifiers::SHIFT, Key::C),
            i.consume_key(cmd | Modifiers::SHIFT, Key::V),
            i.consume_key(cmd | Modifiers::ALT, Key::V),
            i.consume_key(Modifiers::NONE, Key::ArrowLeft),
            i.consume_key(Modifiers::NONE, Key::ArrowRight),
            i.consume_key(Modifiers::NONE, Key::R),
            i.consume_key(Modifiers::NONE, Key::O),
            i.consume_key(Modifiers::NONE, Key::X),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
            i.consume_key(cmd, Key::N),
        ]
    });
    let [
        j,
        w,
        v,
        z,
        bs,
        auto_tone,
        auto_wb,
        copy,
        paste,
        prev,
        left,
        right,
        r,
        o,
        x,
        enter,
        esc,
        snapshot,
    ] = keys;
    if m.doc.is_none() && !(left || right) {
        return;
    }
    if snapshot {
        m.add_snapshot(cx, "");
    }
    if r || ((enter || esc) && m.crop_tool.is_some()) {
        m.toggle_crop_tool();
    }
    if o && let Some(tool) = &mut m.crop_tool {
        tool.cycle_overlay();
    }
    if x && let (Some(tool), Some(doc)) = (&mut m.crop_tool, &m.doc)
        && let Some(session) = &doc.session
    {
        let canvas = archroom_services::engine::geometry::resolve(
            session.source_size,
            session.orientation,
            &doc.params,
            true,
        )
        .canvas;
        if let Some(out) = tool.swap_orientation(&doc.params, canvas) {
            m.apply_outcome(cx, out);
        }
    }
    if j {
        m.clip = !m.clip;
    }
    if w {
        m.eyedropper = !m.eyedropper;
    }
    if v {
        m.toggle_treatment(cx);
    }
    if z {
        m.zoom = if m.zoom == Zoom::Fit {
            Zoom::OneToOne
        } else {
            Zoom::Fit
        };
        m.pan = egui::Vec2::ZERO;
    }
    if bs {
        m.before = !m.before;
    }
    if auto_tone {
        m.auto_tone(cx);
    }
    if auto_wb {
        m.auto_wb(cx);
    }
    if copy && let Some(doc) = &m.doc {
        m.copy_dialog = Some(CopyDialog::new(&m.registry, &doc.params, Mode::Copy));
    }
    if paste && let Some(c) = m.clipboard.take() {
        m.paste(cx, &c.source, &c.groups, "Paste Settings", true);
        m.clipboard = Some(c);
    }
    if prev {
        m.paste_previous(cx);
    }
    if left || right {
        let ordered = cx.selection.visible.clone();
        cx.selection.advance(&ordered, if right { 1 } else { -1 });
    }
}

pub fn show(ui: &mut egui::Ui, m: &mut DevelopModule, cx: &mut AppCx) {
    handle_keys(ui.ctx(), m, cx);

    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_gray(0x10));

    let Some(doc) = &m.doc else {
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.centered_and_justified(|ui| ui.label("Select a photo to develop."));
        });
        return;
    };
    let ppp = ui.ctx().pixels_per_point();

    let (Some(session), Some(tex)) = (&doc.session, doc.texture) else {
        // Still decoding: show the cached preview so the canvas isn't empty.
        let preview = cx
            .previews
            .as_ref()
            .and_then(|p| p.lookup(doc.photo, LEVEL_L2).ok().flatten());
        if let Some(path) = preview {
            let quarters = doc.info.as_ref().map_or(0, |i| i.user_orientation);
            let odd = quarters % 2 != 0;
            let max = if odd {
                egui::vec2(rect.height(), rect.width())
            } else {
                rect.size()
            };
            let mut image = egui::Image::from_uri(format!("file://{}", path.display()))
                .max_size(max)
                .maintain_aspect_ratio(true);
            if quarters != 0 {
                image = image.rotate(
                    quarters as f32 * std::f32::consts::FRAC_PI_2,
                    egui::Vec2::splat(0.5),
                );
            }
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::centered_and_justified(
                        egui::Direction::TopDown,
                    )),
                |ui| ui.add(image),
            );
        }
        if let Some(e) = &doc.error {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                e,
                egui::FontId::proportional(14.0),
                egui::Color32::LIGHT_RED,
            );
        } else {
            badge(ui, rect, "Loading…", true);
        }
        return;
    };

    // Layout: the image rect within the canvas.
    // The layout follows what is rendered: the crop, or the whole canvas
    // while the crop tool is open.
    let shown = if m.before {
        archroom_services::engine::EditParams::default()
    } else {
        doc.params.clone()
    };
    let geom = archroom_services::engine::geometry::resolve(
        session.source_size,
        session.orientation,
        &shown,
        m.crop_tool.is_some(),
    );
    let (ow, oh) = geom.crop_px();
    let (ow, oh) = (ow as f32, oh as f32);
    let mut size = match m.zoom.factor() {
        None => {
            let s = (rect.width() / ow).min(rect.height() / oh);
            egui::vec2(ow * s, oh * s)
        }
        Some(z) => egui::vec2(ow * z / ppp, oh * z / ppp),
    };
    m.want_edge = ((size.max_elem() * ppp).ceil() as u32).clamp(256, 4096);

    let tool_open = m.crop_tool.is_some();
    if resp.dragged() && m.zoom != Zoom::Fit && !m.eyedropper && !tool_open {
        m.pan += resp.drag_delta();
    }
    let max_pan = ((size - rect.size()) / 2.0).max(egui::Vec2::ZERO);
    m.pan = m.pan.clamp(-max_pan, max_pan);
    // A left-click toggles between Fit and the configured zoom.
    if resp.clicked_by(egui::PointerButton::Primary)
        && !m.eyedropper
        && !tool_open
        && let Some(target) = zoom_from_click(cx.settings.click_zoom)
    {
        // Zooming in keeps the image point under the pointer in place.
        let anchor = resp.interact_pointer_pos().filter(|_| m.zoom == Zoom::Fit);
        let old_center = rect.center() + m.pan;
        m.zoom = if m.zoom == Zoom::Fit {
            target
        } else {
            Zoom::Fit
        };
        m.pan = egui::Vec2::ZERO;
        if let (Some(pos), Some(z)) = (anchor, m.zoom.factor()) {
            let frac = (pos - old_center) / size;
            size = egui::vec2(ow * z / ppp, oh * z / ppp);
            let max_pan = ((size - rect.size()) / 2.0).max(egui::Vec2::ZERO);
            m.pan = (pos - frac * size - rect.center()).clamp(-max_pan, max_pan);
        } else {
            size = match m.zoom.factor() {
                None => {
                    let s = (rect.width() / ow).min(rect.height() / oh);
                    egui::vec2(ow * s, oh * s)
                }
                Some(z) => egui::vec2(ow * z / ppp, oh * z / ppp),
            };
        }
        m.want_edge = ((size.max_elem() * ppp).ceil() as u32).clamp(256, 4096);
    }
    // Ease from the previously drawn view to the new one whenever the zoom
    // changes. Center and size both move linearly, so the point under the
    // pointer at a click stays under it throughout.
    let now = ui.input(|i| i.time);
    if m.zoom != m.last_zoom {
        m.last_zoom = m.zoom;
        m.zoom_anim = (m.drawn.0.x > 0.0).then_some(ZoomAnim {
            start: now,
            from_size: m.drawn.0,
            from_pan: m.drawn.1,
        });
    }
    if resp.dragged() {
        m.zoom_anim = None;
    }
    let (draw_size, draw_pan) = match m.zoom_anim {
        Some(a) => {
            let t = ((now - a.start) / ZOOM_ANIM_SECS) as f32;
            if t >= 1.0 {
                m.zoom_anim = None;
                (size, m.pan)
            } else {
                let e = 1.0 - (1.0 - t).powi(3);
                ui.ctx().request_repaint();
                (
                    a.from_size + (size - a.from_size) * e,
                    a.from_pan + (m.pan - a.from_pan) * e,
                )
            }
        }
        None => (size, m.pan),
    };
    m.drawn = (draw_size, draw_pan);
    let img_rect = egui::Rect::from_center_size(rect.center() + draw_pan, draw_size);
    // The left panel (Navigator) draws before this runs, so it sees last
    // frame's view; ask for one more frame when the view moved.
    let view = crate::ViewInfo {
        canvas: rect,
        image: img_rect,
    };
    if m.view
        .is_none_or(|old| old.canvas != view.canvas || old.image != view.image)
    {
        ui.ctx().request_repaint();
    }
    m.view = Some(view);
    painter.image(
        tex,
        img_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    // Crop & straighten tool overlay.
    let mut crop_out = None;
    if let Some(tool) = &mut m.crop_tool {
        crop_out = tool.canvas(ui, img_rect, &resp, &doc.params, geom.canvas);
    }

    // White-balance eyedropper.
    let mut picked: Option<(f64, f64)> = None;
    if m.eyedropper {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if let Some(pos) = resp.hover_pos()
            && img_rect.contains(pos)
        {
            let u = (pos.x - img_rect.left()) / img_rect.width();
            let v = (pos.y - img_rect.top()) / img_rect.height();
            if let Some((t, n)) = session.analysis.pick_white(&geom, u, v) {
                painter.text(
                    pos + egui::vec2(14.0, 14.0),
                    egui::Align2::LEFT_TOP,
                    format!("{t:.0} K  {n:+.0}"),
                    egui::FontId::proportional(13.0),
                    egui::Color32::WHITE,
                );
                if resp.clicked() {
                    picked = Some((t, n));
                }
            }
        }
    }

    if doc.session.as_ref().is_some_and(|_| doc.error.is_some()) {
        badge(ui, rect, "Render error", false);
    } else if m.before {
        badge(ui, rect, "Before", false);
    }
    if let Some(o) = crop_out {
        m.apply_outcome(cx, o);
        return;
    }
    if let Some((temp, tint)) = picked {
        let mut params = doc.params.clone();
        params.set::<WhiteBalance>(WhiteBalanceParams {
            mode: WbMode::Custom,
            temp,
            tint,
        });
        m.eyedropper = false;
        m.apply(
            cx,
            params,
            Change {
                label: "White Balance".into(),
                immediate: true,
            },
        );
    }
}

fn badge(ui: &mut egui::Ui, rect: egui::Rect, text: &str, spinner: bool) {
    let area =
        egui::Rect::from_min_size(rect.left_top() + egui::vec2(10.0, 10.0), egui::Vec2::ZERO);
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(area.expand2(egui::vec2(160.0, 16.0))),
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if spinner {
                        ui.spinner();
                    }
                    ui.label(text);
                });
            });
        },
    );
}
