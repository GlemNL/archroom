//! The History list (plan §8.1): click a step to go back to it.

use archroom_services::engine::EditParams;
use archroom_shell::AppCx;

use crate::DevelopModule;

pub fn show(ui: &mut egui::Ui, m: &mut DevelopModule, cx: &mut AppCx) {
    let Some(doc) = &m.doc else {
        return;
    };
    let newest = doc.history.last().map_or(0, |r| r.seq);
    let current = doc.cursor.unwrap_or(newest);
    let rows: Vec<(i64, String, String)> = doc
        .history
        .iter()
        .rev()
        .map(|r| (r.seq, r.label.clone(), r.params.clone()))
        .collect();
    let mut target: Option<(i64, EditParams)> = None;
    egui::ScrollArea::vertical()
        .max_height(240.0)
        .id_salt("history_list")
        .show(ui, |ui| {
            for (seq, label, params) in &rows {
                if ui.selectable_label(*seq == current, label).clicked() {
                    target = Some((*seq, EditParams::from_json(params).unwrap_or_default()));
                }
            }
            if ui.selectable_label(current == 0, "Initial state").clicked() {
                target = Some((0, EditParams::default()));
            }
        });
    if let Some((seq, params)) = target {
        m.jump(cx, seq, params);
    }
}
