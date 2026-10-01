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
    formatter: Option<fn(f32) -> String>,
    parser: Option<fn(&str) -> Option<f32>>,
    accent: Option<egui::Color32>,
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
            formatter: None,
            parser: None,
            accent: None,
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

    /// Colours the filled part of the track (the colour bands of HSL).
    pub fn accent(mut self, color: egui::Color32) -> Self {
        self.accent = Some(color);
        self
    }

    /// Shows the value through `format` and reads typed text through
    /// `parse`, for sliders whose track isn't the displayed unit (Temp is
    /// linear in mireds but reads in kelvin).
    pub fn display(mut self, format: fn(f32) -> String, parse: fn(&str) -> Option<f32>) -> Self {
        self.formatter = Some(format);
        self.parser = Some(parse);
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
            formatter,
            parser,
            accent,
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

            // The track takes whatever the row has left after the value box,
            // so the slider follows the side panel's width.
            const VALUE_WIDTH: f32 = 56.0;
            let spacing = ui.spacing().item_spacing.x;
            ui.spacing_mut().slider_width =
                (ui.available_width() - VALUE_WIDTH - spacing).max(40.0);

            let drag_response = ui
                .scope(|ui| {
                    if let Some(c) = accent {
                        ui.visuals_mut().selection.bg_fill = c;
                    }
                    ui.add(
                        egui::Slider::new(value, range.clone())
                            .show_value(false)
                            .trailing_fill(true),
                    )
                })
                .inner;

            if drag_response.hovered() {
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 {
                    let coarse = ui.input(|i| i.modifiers.shift);
                    let step = if coarse { coarse_step } else { fine_step };
                    *value = (*value + step * scroll.signum()).clamp(*range.start(), *range.end());
                }
            }

            let text = match formatter {
                Some(f) => f(*value),
                None => format!("{value:.decimals$}{suffix}"),
            };
            let mut drag = egui::DragValue::new(value)
                .range(range)
                .custom_formatter(move |_, _| text.clone())
                .speed(fine_step);
            if let Some(parse) = parser {
                drag = drag.custom_parser(move |s| parse(s).map(f64::from));
            }
            ui.add_sized([VALUE_WIDTH, ui.spacing().interact_size.y], drag);

            label_response | drag_response
        })
        .inner
    }
}
