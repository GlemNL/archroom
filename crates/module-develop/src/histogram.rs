//! The histogram widget (plan §8.2): RGB overlay from the GPU's 256-bin
//! counts, with clipping triangles that toggle the `J` overlay.

use archroom_services::engine::pipeline::Histogram;

const HEIGHT: f32 = 80.0;

/// Draws `hist`; returns true when a clipping triangle was clicked.
pub fn show(ui: &mut egui::Ui, hist: Option<&Histogram>, clip_on: bool) -> bool {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(0x14));

    let mut shadows_clipped = false;
    let mut highlights_clipped = false;
    if let Some(h) = hist {
        // Scale by a high percentile of the interior bins so one spike
        // (pure black/white) doesn't flatten everything else.
        let mut interior: Vec<u32> = (0..3)
            .flat_map(|c| h.bins[c][1..255].iter().copied())
            .collect();
        interior.sort_unstable();
        let peak = interior
            .get(interior.len() * 995 / 1000)
            .copied()
            .unwrap_or(1)
            .max(1) as f32;
        let colors = [
            egui::Color32::from_rgba_unmultiplied(230, 60, 60, 110),
            egui::Color32::from_rgba_unmultiplied(60, 200, 80, 110),
            egui::Color32::from_rgba_unmultiplied(70, 120, 240, 110),
        ];
        for (c, color) in colors.iter().enumerate() {
            let mut pts = Vec::with_capacity(258);
            pts.push(egui::pos2(rect.left(), rect.bottom()));
            for (i, count) in h.bins[c].iter().enumerate() {
                let x = rect.left() + rect.width() * (i as f32 + 0.5) / 256.0;
                let y = rect.bottom() - (*count as f32 / peak).sqrt().min(1.0) * (HEIGHT - 2.0);
                pts.push(egui::pos2(x, y));
            }
            pts.push(egui::pos2(rect.right(), rect.bottom()));
            painter.add(egui::Shape::convex_polygon(pts, *color, egui::Stroke::NONE));
        }
        let total: u32 = h.bins[3].iter().sum::<u32>().max(1);
        let frac = |bin: usize| (0..3).any(|c| h.bins[c][bin] as f32 / total as f32 > 0.001);
        shadows_clipped = frac(0);
        highlights_clipped = frac(255);
    }

    let tri = |x: f32, dir: f32, lit: bool, color: egui::Color32| {
        let a = egui::pos2(x, rect.top() + 3.0);
        let pts = vec![
            a,
            egui::pos2(x + 10.0 * dir, a.y),
            egui::pos2(x, a.y + 10.0),
        ];
        let fill = if lit {
            color
        } else {
            egui::Color32::from_gray(0x50)
        };
        painter.add(egui::Shape::convex_polygon(pts, fill, egui::Stroke::NONE));
    };
    tri(
        rect.left() + 4.0,
        1.0,
        shadows_clipped,
        egui::Color32::from_rgb(70, 120, 240),
    );
    tri(
        rect.right() - 4.0,
        -1.0,
        highlights_clipped,
        egui::Color32::from_rgb(230, 60, 60),
    );
    if clip_on {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0_f32, archroom_ui::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    resp.on_hover_text("Click to toggle the clipping overlay (J)")
        .clicked()
}
