//! The Library filter bar (plan §7.5) and the rule editor shared with the
//! smart-collection dialog. Both produce `criteria::Rule`s; every bit of
//! SQL lives in `viberoom_catalog::criteria`, not here.

use viberoom_services::criteria::{FlagValue, RelOp, Rule, SmartRules};

use crate::shortcuts::COLOR_LABELS;

/// The bar's editable state. `rules()` turns it into the criteria list the
/// Grid query ANDs together.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FilterState {
    pub text: String,
    /// 0 = no rating filter.
    pub min_rating: u8,
    pub flag: Option<FlagValue>,
    pub label: Option<String>,
    pub edited: Option<bool>,
    pub camera: String,
}

impl FilterState {
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    pub fn rules(&self) -> Vec<Rule> {
        let mut rules = Vec::new();
        if !self.text.trim().is_empty() {
            rules.push(Rule::Text {
                contains: self.text.trim().to_string(),
            });
        }
        if self.min_rating > 0 {
            rules.push(Rule::Rating {
                op: RelOp::AtLeast,
                stars: self.min_rating,
            });
        }
        if let Some(value) = self.flag {
            rules.push(Rule::Flag { value });
        }
        if let Some(name) = &self.label {
            rules.push(Rule::Label {
                label: Some(name.clone()),
            });
        }
        if let Some(edited) = self.edited {
            rules.push(Rule::Edited { edited });
        }
        if !self.camera.trim().is_empty() {
            rules.push(Rule::Camera {
                model: self.camera.trim().to_string(),
            });
        }
        rules
    }
}

/// Draws the bar; returns true when the filter changed this frame.
pub fn show(ui: &mut egui::Ui, filter: &mut FilterState, shown: usize) -> bool {
    let before = filter.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("🔍");
        ui.add(
            egui::TextEdit::singleline(&mut filter.text)
                .hint_text("Filename, title, caption, keyword")
                .desired_width(200.0),
        );
        ui.separator();

        ui.label("Rating ≥");
        for star in 1..=5u8 {
            let on = filter.min_rating >= star;
            if ui.selectable_label(on, "★").clicked() {
                filter.min_rating = if filter.min_rating == star { 0 } else { star };
            }
        }
        ui.separator();

        for (text, value) in [
            ("⚑ Picked", FlagValue::Picked),
            ("Rejected", FlagValue::Rejected),
        ] {
            if ui
                .selectable_label(filter.flag == Some(value), text)
                .clicked()
            {
                filter.flag = if filter.flag == Some(value) {
                    None
                } else {
                    Some(value)
                };
            }
        }
        ui.separator();

        egui::ComboBox::from_id_salt("filter_label")
            .selected_text(filter.label.clone().unwrap_or_else(|| "Label".to_string()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filter.label, None, "Any");
                for name in COLOR_LABELS {
                    ui.selectable_value(&mut filter.label, Some(name.to_string()), name);
                }
            });
        egui::ComboBox::from_id_salt("filter_edited")
            .selected_text(match filter.edited {
                None => "Edited: any",
                Some(true) => "Edited",
                Some(false) => "Unedited",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filter.edited, None, "Any");
                ui.selectable_value(&mut filter.edited, Some(true), "Edited");
                ui.selectable_value(&mut filter.edited, Some(false), "Unedited");
            });
        ui.add(
            egui::TextEdit::singleline(&mut filter.camera)
                .hint_text("Camera model")
                .desired_width(110.0),
        );
        ui.separator();

        if ui
            .add_enabled(filter.is_active(), egui::Button::new("Clear"))
            .clicked()
        {
            *filter = FilterState::default();
        }
        ui.weak(format!("{shown} shown"));
    });
    *filter != before
}

/// Editor for a smart collection's rules; returns true when anything changed.
pub fn rules_editor(ui: &mut egui::Ui, rules: &mut SmartRules) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Match");
        changed |= ui
            .selectable_value(&mut rules.match_all, true, "all")
            .changed();
        changed |= ui
            .selectable_value(&mut rules.match_all, false, "any")
            .changed();
        ui.label("of the following rules:");
    });

    let mut remove = None;
    for (i, rule) in rules.rules.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= rule_row(ui, i, rule);
            if ui.small_button("✖").on_hover_text("Remove rule").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        rules.rules.remove(i);
        changed = true;
    }

    ui.menu_button("+ Add rule", |ui| {
        for (name, rule) in default_rules() {
            if ui.button(name).clicked() {
                rules.rules.push(rule);
                changed = true;
                ui.close();
            }
        }
    });
    changed
}

fn default_rules() -> Vec<(&'static str, Rule)> {
    vec![
        (
            "Text",
            Rule::Text {
                contains: String::new(),
            },
        ),
        (
            "Rating",
            Rule::Rating {
                op: RelOp::AtLeast,
                stars: 3,
            },
        ),
        (
            "Flag",
            Rule::Flag {
                value: FlagValue::Picked,
            },
        ),
        (
            "Color label",
            Rule::Label {
                label: Some(COLOR_LABELS[0].to_string()),
            },
        ),
        ("Edited", Rule::Edited { edited: true }),
        ("Virtual copy", Rule::VirtualCopy { is_copy: true }),
        ("Capture year", Rule::CaptureYear { year: 2024 }),
        (
            "Camera",
            Rule::Camera {
                model: String::new(),
            },
        ),
        (
            "Lens",
            Rule::Lens {
                contains: String::new(),
            },
        ),
    ]
}

fn rule_row(ui: &mut egui::Ui, index: usize, rule: &mut Rule) -> bool {
    let before = rule.clone();
    match &mut *rule {
        Rule::Text { contains } => {
            ui.label("Text contains");
            ui.text_edit_singleline(contains);
        }
        Rule::Rating { op, stars } => {
            ui.label("Rating");
            egui::ComboBox::from_id_salt(("rule_op", index))
                .selected_text(match op {
                    RelOp::AtLeast => "≥",
                    RelOp::Exactly => "=",
                    RelOp::AtMost => "≤",
                })
                .width(40.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(op, RelOp::AtLeast, "≥");
                    ui.selectable_value(op, RelOp::Exactly, "=");
                    ui.selectable_value(op, RelOp::AtMost, "≤");
                });
            ui.add(egui::DragValue::new(stars).range(0..=5));
        }
        Rule::Flag { value } => {
            ui.label("Flag is");
            ui.selectable_value(value, FlagValue::Picked, "Picked");
            ui.selectable_value(value, FlagValue::Unflagged, "Unflagged");
            ui.selectable_value(value, FlagValue::Rejected, "Rejected");
        }
        Rule::Label { label } => {
            ui.label("Label is");
            egui::ComboBox::from_id_salt(("rule_label", index))
                .selected_text(label.clone().unwrap_or_else(|| "None".to_string()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(label, None, "None");
                    for name in COLOR_LABELS {
                        ui.selectable_value(label, Some(name.to_string()), name);
                    }
                });
        }
        Rule::Edited { edited } => {
            ui.label("Photo is");
            ui.selectable_value(edited, true, "edited");
            ui.selectable_value(edited, false, "unedited");
        }
        Rule::VirtualCopy { is_copy } => {
            ui.label("Photo is");
            ui.selectable_value(is_copy, true, "a virtual copy");
            ui.selectable_value(is_copy, false, "a master");
        }
        Rule::CaptureYear { year } => {
            ui.label("Captured in");
            ui.add(egui::DragValue::new(year).range(1900..=2100));
        }
        Rule::Camera { model } => {
            ui.label("Camera is");
            ui.text_edit_singleline(model);
        }
        Rule::Lens { contains } => {
            ui.label("Lens contains");
            ui.text_edit_singleline(contains);
        }
        // Month/day rules only come from the date column; show read-only.
        other => {
            ui.label(other.describe());
        }
    }
    *rule != before
}
