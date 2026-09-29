//! Re-renders a photo's L1/L2 previews through the develop pipeline after
//! an edit, so the Library shows what Develop shows (plan §5.4, §8.4). A
//! background job: it decodes on its own, so it never competes with the
//! interactive session for the render thread.

use std::path::PathBuf;

use archroom_catalog::repo::PhotoFileInfo;
use archroom_core::ids::PhotoId;
use archroom_engine::gpu::GpuContext;
use archroom_engine::pipeline::{Pipeline, RenderRequest};
use archroom_engine::{EditParams, Orientation};
use archroom_io::DecodeOptions;
use archroom_jobs::{Job, JobContext, Priority};
use archroom_preview::{
    L1_BUDGET_PX, L2_BUDGET_PX, LEVEL_L1, LEVEL_L2, PreviewCache, encode_jpeg_from_rgba8,
};

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct RerenderPreviewsJob {
    pub catalog_path: PathBuf,
    pub gpu: GpuContext,
    pub photo: PhotoId,
    pub info: PhotoFileInfo,
    pub params: EditParams,
}

fn params_hash_i64(params: &EditParams) -> i64 {
    let h = params.hash();
    i64::from_be_bytes([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]])
}

/// Renders `params` at both preview budgets and stores them. An unedited
/// photo goes back to the plain preview instead.
pub fn rerender(
    previews: &PreviewCache,
    gpu: &GpuContext,
    photo: PhotoId,
    info: &PhotoFileInfo,
    params: &EditParams,
    cx: &JobContext,
) -> Result<()> {
    if params.is_identity() {
        for (level, budget) in [(LEVEL_L1, L1_BUDGET_PX), (LEVEL_L2, L2_BUDGET_PX)] {
            crate::preview::generate_and_store(previews, &info.path, photo, level, budget)?;
        }
        return Ok(());
    }
    let decoder = archroom_io::decoder_for(&info.path)
        .ok_or_else(|| Error::Other(format!("no decoder for {}", info.path.display())))?;
    let decoded = decoder.decode(&info.path, &DecodeOptions::default())?;
    let mut pipeline = Pipeline::new(gpu, &decoded).map_err(|e| Error::Other(e.to_string()))?;
    let hash = params_hash_i64(params);
    for (level, budget) in [(LEVEL_L2, L2_BUDGET_PX), (LEVEL_L1, L1_BUDGET_PX)] {
        if cx.is_cancelled() {
            return Ok(());
        }
        let mut req = RenderRequest::new(params.clone(), budget);
        req.orientation = Orientation::from_exif(info.exif_orientation);
        pipeline
            .render(&req)
            .map_err(|e| Error::Other(e.to_string()))?;
        let (w, h, rgba) = pipeline
            .read_output_rgba8()
            .map_err(|e| Error::Other(e.to_string()))?;
        let jpeg = encode_jpeg_from_rgba8(&rgba, w, h)?;
        previews.store(photo, level, hash, &jpeg, w, h)?;
    }
    Ok(())
}

impl Job for RerenderPreviewsJob {
    fn label(&self) -> String {
        "Updating previews".to_string()
    }

    fn priority(&self) -> Priority {
        Priority::Background
    }

    fn run(self: Box<Self>, cx: &JobContext) {
        let result = PreviewCache::open_for_catalog(&self.catalog_path)
            .map_err(Error::from)
            .and_then(|previews| {
                rerender(
                    &previews,
                    &self.gpu,
                    self.photo,
                    &self.info,
                    &self.params,
                    cx,
                )
            });
        if let Err(e) = result {
            tracing::warn!(photo = self.photo.get(), error = %e, "preview re-render failed");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn hash_differs_between_edits() {
        let a = EditParams::default();
        let mut b = EditParams::default();
        b.set::<archroom_engine::ops::Exposure>(archroom_engine::ops::ExposureParams { ev: 1.0 });
        assert_ne!(params_hash_i64(&a), params_hash_i64(&b));
    }
}
