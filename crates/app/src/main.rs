mod app;

use app::ArchroomApp;

/// The GPU setup: the adapter named in Preferences when it exists (else the
/// default one), and a device that keeps the adapter's texture and buffer
/// limits so a 100 MP export does not trip egui's conservative defaults.
fn wgpu_setup(preferred: Option<String>) -> eframe::egui_wgpu::WgpuSetup {
    use std::sync::Arc;

    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::default();
    if let Some(name) = preferred {
        let wanted = name.to_lowercase();
        setup.native_adapter_selector = Some(Arc::new(move |adapters, _surface| {
            adapters
                .iter()
                .find(|a| a.get_info().name.to_lowercase().contains(&wanted))
                .or_else(|| adapters.first())
                .cloned()
                .ok_or_else(|| "no GPU adapter".to_string())
        }));
    }
    let base = setup.device_descriptor.clone();
    setup.device_descriptor = Arc::new(move |adapter| {
        let mut desc = base(adapter);
        let have = adapter.limits();
        desc.required_limits.max_texture_dimension_2d = have.max_texture_dimension_2d;
        desc.required_limits.max_buffer_size = have.max_buffer_size;
        desc.required_limits.max_storage_buffer_binding_size = have.max_storage_buffer_binding_size;
        desc
    });
    eframe::egui_wgpu::WgpuSetup::CreateNew(setup)
}

fn main() -> anyhow::Result<()> {
    archroom_core::tracing_setup::init();

    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1440.0, 900.0]),
        ..Default::default()
    };
    options.wgpu_options.wgpu_setup = wgpu_setup(
        archroom_core::settings::Settings::load()
            .ok()
            .and_then(|s| s.gpu_adapter),
    );

    eframe::run_native(
        "Archroom",
        options,
        Box::new(|cc| Ok(Box::new(ArchroomApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}
