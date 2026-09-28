//! M0 spike UI (plan roadmap): a debug window that decodes a raw file and
//! shows it through the real wgpu pipeline with a live Exposure slider,
//! proving D1 (egui/eframe sharing one wgpu device with the engine) works
//! end to end. Temporary — the real Develop canvas (M3) replaces this.
//!
//! Decoding runs on the UI thread on button click, which briefly violates
//! plan principle 4 ("the UI thread never waits"); that's a deliberate
//! spike-only shortcut, not the production import/decode path (M1/M3 run
//! decoding as a background `Job`).

use std::time::Duration;

use archroom_engine::spike::SpikePipeline;
use eframe::egui_wgpu;
use tracing::{error, info};

pub struct SpikeView {
    raw_path: String,
    status: String,
    pipeline: Option<SpikePipeline>,
    texture_id: Option<egui::TextureId>,
    exposure_ev: f32,
    last_latency: Option<Duration>,
}

impl Default for SpikeView {
    fn default() -> Self {
        let raw_path = std::env::var("ARCHROOM_SPIKE_RAW").unwrap_or_default();
        Self {
            raw_path,
            status: String::new(),
            pipeline: None,
            texture_id: None,
            exposure_ev: 0.0,
            last_latency: None,
        }
    }
}

impl SpikeView {
    pub fn ui(&mut self, ui: &mut egui::Ui, render_state: &egui_wgpu::RenderState) {
        ui.horizontal(|ui| {
            ui.label("Raw file:");
            ui.text_edit_singleline(&mut self.raw_path);
            if ui.button("Decode + upload").clicked() {
                self.decode_and_upload(render_state);
            }
        });

        if !self.status.is_empty() {
            ui.label(&self.status);
        }

        let Some(pipeline) = &self.pipeline else {
            ui.label("No image loaded yet.");
            return;
        };

        let mut changed = false;
        ui.add(
            archroom_ui::LrSlider::new("Exposure", &mut self.exposure_ev, -5.0..=5.0)
                .default_value(0.0)
                .step(0.05, 0.5)
                .decimals(2)
                .suffix(" EV"),
        )
        .changed()
        .then(|| changed = true);

        if changed || self.texture_id.is_none() {
            let latency =
                pipeline.render(&render_state.device, &render_state.queue, self.exposure_ev);
            self.last_latency = Some(latency);

            let mut renderer = render_state.renderer.write();
            let id = match self.texture_id {
                Some(id) => {
                    renderer.update_egui_texture_from_wgpu_texture(
                        &render_state.device,
                        pipeline.output_view(),
                        wgpu::FilterMode::Linear,
                        id,
                    );
                    id
                }
                None => renderer.register_native_texture(
                    &render_state.device,
                    pipeline.output_view(),
                    wgpu::FilterMode::Linear,
                ),
            };
            drop(renderer);
            self.texture_id = Some(id);
        }

        if let Some(latency) = self.last_latency {
            ui.label(format!(
                "{}x{} · render (submit+GPU wait): {:.2} ms",
                pipeline.width,
                pipeline.height,
                latency.as_secs_f64() * 1000.0
            ));
        }

        if let Some(id) = self.texture_id {
            let available = ui.available_size();
            let aspect = pipeline.width as f32 / pipeline.height as f32;
            let (w, h) = if available.x / aspect <= available.y {
                (available.x, available.x / aspect)
            } else {
                (available.y * aspect, available.y)
            };
            ui.add(egui::Image::new((id, egui::vec2(w.max(1.0), h.max(1.0)))));
        }
    }

    fn decode_and_upload(&mut self, render_state: &egui_wgpu::RenderState) {
        self.texture_id = None;
        self.pipeline = None;
        let path = std::path::Path::new(&self.raw_path);

        info!(path = %self.raw_path, "spike: decoding raw");
        let start = std::time::Instant::now();
        match archroom_io::decode_to_linear_rgb_f32(path) {
            Ok(image) => {
                let decode_ms = start.elapsed().as_secs_f64() * 1000.0;
                self.status = format!(
                    "decoded {}x{} in {decode_ms:.0} ms",
                    image.width, image.height
                );
                info!(
                    width = image.width,
                    height = image.height,
                    decode_ms,
                    "spike: decoded"
                );
                let pipeline =
                    SpikePipeline::new(&render_state.device, &render_state.queue, &image);
                self.pipeline = Some(pipeline);
            }
            Err(e) => {
                self.status = format!("decode failed: {e}");
                error!(error = %e, "spike: decode failed");
            }
        }
    }
}
