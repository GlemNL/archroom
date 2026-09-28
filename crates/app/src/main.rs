mod app;

use app::ArchroomApp;

fn main() -> anyhow::Result<()> {
    archroom_core::tracing_setup::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1440.0, 900.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Archroom",
        options,
        Box::new(|cc| Ok(Box::new(ArchroomApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}
