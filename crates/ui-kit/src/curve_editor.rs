//! A point-curve editor (plan §6.6/§8.2): a square graph where control
//! points are added by clicking, moved by dragging and removed by
//! double-clicking. Points are normalised to 0..1 with y pointing up. The
//! widget doesn't know how the curve interpolates; the caller passes the
//! sampled curve to draw, so `ui-kit` stays free of the engine.

use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

const HANDLE_RADIUS: f32 = 5.0;
const MIN_GAP: f64 = 0.01;

/// Result of one frame of interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveEdit {
    None,
    /// A point was added or is being dragged: the value changes live.
    Changed,
}

fn to_screen(rect: Rect, p: [f64; 2]) -> Pos2 {
    Pos2::new(
        rect.left() + p[0] as f32 * rect.width(),
        rect.bottom() - p[1] as f32 * rect.height(),
    )
}

fn from_screen(rect: Rect, pos: Pos2) -> [f64; 2] {
    [
        f64::from(((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0)),
        f64::from(((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0)),
    ]
}

/// Draws the editor at `size` points square. `samples` is the evaluated
/// curve, evenly spaced over x in 0..1 (empty = the diagonal).
pub fn curve_editor(
    ui: &mut Ui,
    id: egui::Id,
    points: &mut Vec<[f64; 2]>,
    samples: &[f32],
    color: Color32,
) -> (Response, CurveEdit) {
    let side = ui.available_width().min(260.0);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_gray(0x14));
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let stroke = Stroke::new(1.0_f32, Color32::from_gray(0x2a));
        painter.line_segment(
            [
                Pos2::new(rect.left() + t * rect.width(), rect.top()),
                Pos2::new(rect.left() + t * rect.width(), rect.bottom()),
            ],
            stroke,
        );
        painter.line_segment(
            [
                Pos2::new(rect.left(), rect.top() + t * rect.height()),
                Pos2::new(rect.right(), rect.top() + t * rect.height()),
            ],
            stroke,
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        Stroke::new(1.0_f32, Color32::from_gray(0x3c)),
    );
    if samples.len() > 1 {
        let n = samples.len() - 1;
        let line: Vec<Pos2> = samples
            .iter()
            .enumerate()
            .map(|(i, y)| to_screen(rect, [i as f64 / n as f64, f64::from(*y)]))
            .collect();
        painter.add(egui::Shape::line(line, Stroke::new(1.5_f32, color)));
    }
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0_f32, Color32::from_gray(0x50)),
        egui::StrokeKind::Inside,
    );

    let mut edit = CurveEdit::None;
    let drag_key = id.with("drag");
    let hit = |pts: &[[f64; 2]], pos: Pos2| {
        pts.iter()
            .position(|p| to_screen(rect, *p).distance(pos) <= HANDLE_RADIUS + 3.0)
    };
    let seed = |pts: &mut Vec<[f64; 2]>| {
        if pts.is_empty() {
            *pts = vec![[0.0, 0.0], [1.0, 1.0]];
        }
    };

    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        seed(points);
        let grabbed = hit(points, pos).or_else(|| {
            // Grabbing empty space inserts a point under the cursor.
            let p = from_screen(rect, pos);
            let idx = points.partition_point(|q| q[0] < p[0]);
            let clear = points.iter().all(|q| (q[0] - p[0]).abs() > MIN_GAP);
            clear.then(|| {
                points.insert(idx, p);
                edit = CurveEdit::Changed;
                idx
            })
        });
        ui.data_mut(|d| d.insert_temp(drag_key, grabbed));
    }
    if resp.dragged()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(Some(i)) = ui.data(|d| d.get_temp::<Option<usize>>(drag_key))
        && i < points.len()
    {
        let mut p = from_screen(rect, pos);
        let last = points.len() - 1;
        if i == 0 || i == last {
            p[0] = points[i][0]; // end points only move vertically
        } else {
            p[0] = p[0].clamp(points[i - 1][0] + MIN_GAP, points[i + 1][0] - MIN_GAP);
        }
        if points[i] != p {
            points[i] = p;
            edit = CurveEdit::Changed;
        }
    }
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(i) = hit(points, pos)
        && i != 0
        && i != points.len() - 1
    {
        points.remove(i);
        edit = CurveEdit::Changed;
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove_temp::<Option<usize>>(drag_key));
    }

    for p in points.iter() {
        let c = to_screen(rect, *p);
        painter.circle(
            c,
            HANDLE_RADIUS,
            Color32::from_gray(0x14),
            Stroke::new(1.5_f32, color),
        );
    }
    if edit == CurveEdit::Changed {
        resp.mark_changed();
    }
    (resp, edit)
}
