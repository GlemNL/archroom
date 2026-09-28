//! The Lightroom-style slider (plan §10.2): label on the left, value on the
//! right, drag to change, double-click the label to reset, click the value
//! to type a precise number, mouse wheel for a fine step and Shift+wheel
//! for a coarse one.

use egui::{Response, Ui, Widget};

#[derive(Debug)]
pub struct LrSlider<'a> {
    label: &'a str,
    value: &'a mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    fine_step: f32,
    coarse_step: f32,
    decimals: usize,
    suffix: &'a str,
    label_width: f32,
}

impl<'a> LrSlider<'a> {
    pub fn new(label: &'a str, value: &'a mut f32, range: std::ops::RangeInclusive<f32>) -> Self {
        let span = (range.end() - range.start()).abs();
        let fine_step = (span / 200.0).max(0.001);
        Self {
            label,
            value,
            range,
            default: 0.0,
            fine_step,
            coarse_step: fine_step * 10.0,
            decimals: 2,
            suffix: "",
            label_width: 90.0,
        }
    }

    pub fn default_value(mut self, default: f32) -> Self {
        self.default = default;
        self
    }

    pub fn step(mut self, fine: f32, coarse: f32) -> Self {
        self.fine_step = fine;
        self.coarse_step = coarse;
        self
    }

    pub fn decimals(mut self, decimals: usize) -> Self {
        self.decimals = decimals;
        self
    }

    pub fn suffix(mut self, suffix: &'a str) -> Self {
        self.suffix = suffix;
        self
    }

    pub fn label_width(mut self, width: f32) -> Self {
        self.label_width = width;
        self
    }
}

impl Widget for LrSlider<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let LrSlider {
            label,
            value,
            range,
            default,
            fine_step,
            coarse_step,
            decimals,
            suffix,
            label_width,
        } = self;

        ui.horizontal(|ui| {
            let label_response = ui.add_sized(
                [label_width, ui.spacing().interact_size.y],
                egui::Label::new(label).sense(egui::Sense::click()),
            );
            if label_response.double_clicked() {
                *value = default;
            }
            let label_response = label_response.on_hover_text("Double-click to reset");

            let drag_response = ui.add(
                egui::Slider::new(value, range.clone())
                    .show_value(false)
                    .trailing_fill(true),
            );

            if drag_response.hovered() {
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 {
                    let coarse = ui.input(|i| i.modifiers.shift);
                    let step = if coarse { coarse_step } else { fine_step };
                    *value = (*value + step * scroll.signum()).clamp(*range.start(), *range.end());
                }
            }

            let text = format!("{value:.decimals$}{suffix}");
            ui.add_sized(
                [56.0, ui.spacing().interact_size.y],
                egui::DragValue::new(value)
                    .range(range)
                    .custom_formatter(move |_, _| text.clone())
                    .speed(fine_step),
            );

            label_response | drag_response
        })
        .inner
    }
}
