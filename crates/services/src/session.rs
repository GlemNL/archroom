//! An opened Develop photo (plan §6.1/§6.10): the decoded image uploaded to
//! the GPU behind a latest-wins [`RenderLoop`], plus the CPU-side analysis
//! that backs Auto Tone, Auto WB and the WB eyedropper. Opening is a
//! background job (decode is ~1–2 s); the UI only ever polls a channel.

use std::path::Path;

use viberoom_core::ids::PhotoId;
use viberoom_engine::analysis::{AutoTone, Proxy, auto_tone, auto_wb, downscale};
use viberoom_engine::geometry::Geometry;
use viberoom_engine::gpu::GpuContext;
use viberoom_engine::ops::{Profile, WbMode, WhiteBalance, WhiteBalanceParams};
use viberoom_engine::pipeline::Pipeline;
use viberoom_engine::rawprep::SceneColor;
use viberoom_engine::render_loop::RenderLoop;
use viberoom_engine::{EditParams, Orientation};
use viberoom_io::{DecodeOptions, DecodedImage};
use viberoom_jobs::{Job, JobContext, Priority, Scheduler};
use crossbeam_channel::Receiver;

use viberoom_catalog::repo::PhotoFileInfo;

const PROXY_EDGE: u32 = 512;

/// CPU analysis of a photo on a small linear proxy.
#[derive(Debug)]
pub struct Analysis {
    proxy: Proxy,
    scene: Option<SceneColor>,
    rendered: bool,
}

impl Analysis {
    fn new(decoded: &DecodedImage) -> Self {
        match decoded {
            DecodedImage::SceneLinear { rgb, camera } => Self {
                proxy: downscale(rgb, PROXY_EDGE, false),
                scene: SceneColor::new(camera),
                rendered: false,
            },
            DecodedImage::Rendered { rgb, .. } => Self {
                proxy: downscale(rgb, PROXY_EDGE, true),
                scene: None,
                rendered: true,
            },
        }
    }

    /// The decoded→working matrix under `edit`'s white balance.
    fn to_working(&self, edit: &EditParams) -> viberoom_color::Mat3d {
        match &self.scene {
            Some(sc) => {
                let wb = edit.get::<WhiteBalance>();
                let illuminant = match wb.mode {
                    WbMode::AsShot => sc.as_shot_white(),
                    WbMode::Custom => viberoom_color::temp::temp_tint_to_xy(wb.temp, wb.tint),
                };
                sc.decoded_to_working(illuminant, viberoom_color::cat::Cat::Cat16)
            }
            None => viberoom_color::cie::REC2020
                .xyz_to_rgb()
                .mul(&viberoom_color::cie::SRGB.rgb_to_xyz()),
        }
    }

    /// Gray-world Auto WB (raw files only).
    pub fn auto_wb(&self) -> Option<WhiteBalanceParams> {
        let (temp, tint) = auto_wb(&self.proxy, self.scene.as_ref()?)?;
        Some(WhiteBalanceParams {
            mode: WbMode::Custom,
            temp,
            tint,
        })
    }

    /// Auto Tone for the current white balance and profile.
    pub fn auto_tone(&self, edit: &EditParams) -> Option<AutoTone> {
        auto_tone(
            &self.proxy,
            &self.to_working(edit),
            &edit.get::<Profile>(),
            self.rendered,
        )
    }

    /// The Temp/Tint that makes the 5×5 median at output position
    /// (`u`, `v` ∈ 0..1 of what is rendered, i.e. through `geometry`'s crop,
    /// straighten and orientation) neutral (plan §6.4).
    pub fn pick_white(&self, geometry: &Geometry, u: f32, v: f32) -> Option<(f64, f64)> {
        let sc = self.scene.as_ref()?;
        let [su, sv] = geometry.output_to_source(f64::from(u), f64::from(v))?;
        let (su, sv) = (su as f32, sv as f32);
        let (w, h) = (self.proxy.width as i64, self.proxy.height as i64);
        let cx = ((su * w as f32) as i64).clamp(0, w - 1);
        let cy = ((sv * h as f32) as i64).clamp(0, h - 1);
        let mut chan: [Vec<f32>; 3] = Default::default();
        for y in (cy - 2).max(0)..=(cy + 2).min(h - 1) {
            for x in (cx - 2).max(0)..=(cx + 2).min(w - 1) {
                let p = self.proxy.rgb[(y * w + x) as usize];
                for c in 0..3 {
                    chan[c].push(p[c]);
                }
            }
        }
        let med = chan.map(|mut v| {
            v.sort_by(|a, b| a.total_cmp(b));
            f64::from(v[v.len() / 2])
        });
        if med.iter().any(|c| *c <= 1e-4) {
            return None;
        }
        let (temp, tint) = viberoom_color::temp::xy_to_temp_tint(sc.white_from_decoded(med));
        Some((temp.round(), tint.round()))
    }
}

/// A photo open in Develop.
#[derive(Debug)]
pub struct Session {
    pub photo: PhotoId,
    pub render: RenderLoop,
    pub analysis: Analysis,
    /// The raw's recorded (Temp, Tint); `None` for rendered files.
    pub as_shot: Option<(f64, f64)>,
    pub orientation: Orientation,
    pub source_size: (u32, u32),
    pub is_raw: bool,
}

fn open(
    gpu: &GpuContext,
    photo: PhotoId,
    info: &PhotoFileInfo,
) -> std::result::Result<Session, String> {
    let path: &Path = &info.path;
    let decoder = viberoom_io::decoder_for(path)
        .ok_or_else(|| format!("unsupported file: {}", path.display()))?;
    let decoded = decoder
        .decode(path, &DecodeOptions::default())
        .map_err(|e| e.to_string())?;
    let analysis = Analysis::new(&decoded);
    let pipeline = Pipeline::new(gpu, &decoded).map_err(|e| e.to_string())?;
    let as_shot = pipeline.as_shot_temp_tint();
    let source_size = pipeline.source_size();
    let is_raw = pipeline.is_raw();
    Ok(Session {
        photo,
        render: RenderLoop::spawn(pipeline),
        analysis,
        as_shot,
        orientation: Orientation::from_exif(info.exif_orientation).rotated(info.user_orientation),
        source_size,
        is_raw,
    })
}

struct OpenJob {
    gpu: GpuContext,
    photo: PhotoId,
    info: PhotoFileInfo,
    tx: crossbeam_channel::Sender<std::result::Result<Session, String>>,
}

impl Job for OpenJob {
    fn label(&self) -> String {
        "Opening photo".to_string()
    }

    fn priority(&self) -> Priority {
        Priority::Interactive
    }

    fn run(self: Box<Self>, cx: &JobContext) {
        if cx.is_cancelled() {
            return;
        }
        let _ = self.tx.send(open(&self.gpu, self.photo, &self.info));
    }
}

/// Decodes and uploads `photo` in the background. The receiver yields
/// exactly one message (or none if the job is cancelled).
pub fn open_in_background(
    jobs: &Scheduler,
    gpu: GpuContext,
    photo: PhotoId,
    info: PhotoFileInfo,
) -> Receiver<std::result::Result<Session, String>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    jobs.submit(OpenJob {
        gpu,
        photo,
        info,
        tx,
    });
    rx
}
