//! CPU side of the red-eye and local-adjustment passes (plan v0.2.0 §3):
//! packs the placed circles and masks into kernel uniforms in *output*
//! pixels (everything is stored in source coordinates and mapped through
//! the geometry), and keeps the brush masks rasterized and uploaded.

use std::collections::HashMap;

use xxhash_rust::xxh3::xxh3_64;

use crate::geometry::Geometry;
use crate::gpu::GpuContext;
use crate::local::{BrushRaster, LocalAdjustment, MAX_LOCAL, MaskDef, Stroke, tuning};
use crate::ops::{EyeMode, EyeSpot, MAX_SPOTS};

/// A kernel uniform: rows of four floats.
pub type Uniform = Vec<[f32; 4]>;

/// Stable key of a uniform's contents.
pub fn uniform_key(u: &[[f32; 4]]) -> u64 {
    xxh3_64(bytemuck::cast_slice(u))
}

/// The red-eye kernel's uniform, `None` when there is nothing to do.
/// `dims` is the proxy size, `src` the decoded source size.
pub fn redeye_uniform(
    spots: &[EyeSpot],
    geom: &Geometry,
    dims: (u32, u32),
    src: (u32, u32),
) -> Option<Uniform> {
    let spots = &spots[..spots.len().min(MAX_SPOTS)];
    if spots.is_empty() {
        return None;
    }
    let (w, h) = (f64::from(dims.0), f64::from(dims.1));
    let scale = geom.output_scale(dims.0);
    let diag = f64::from(src.0).hypot(f64::from(src.1));
    let mut u: Uniform = vec![[0.0; 4]; 1 + MAX_SPOTS * 2];
    u[0] = [dims.0 as f32, dims.1 as f32, spots.len() as f32, 0.0];
    for (i, s) in spots.iter().enumerate() {
        let [ou, ov] = geom.source_to_output(s.x, s.y);
        u[1 + i * 2] = [
            (ou * w) as f32,
            (ov * h) as f32,
            (s.r.max(0.0) * diag * scale) as f32,
            (s.darken.clamp(0.0, 100.0) / 100.0) as f32,
        ];
        u[2 + i * 2] = [
            f32::from(s.mode == EyeMode::Pet),
            f32::from(s.catchlight),
            0.0,
            0.0,
        ];
    }
    Some(u)
}

/// What the local-adjust pass and the mask overlay run with.
pub struct LocalPlan {
    pub uniform: Uniform,
    /// Brush zones in layer order: (adjustment id, its strokes).
    pub zones: Vec<(String, Vec<Stroke>)>,
    /// True when any item uses Highlights or Shadows (needs a base layer).
    pub needs_base: bool,
    /// Index of the overlay's item in the uniform, if requested and found.
    pub overlay: Option<u32>,
}

/// Builds the plan for `adjustments`; identity ones are skipped unless
/// `overlay_id` names them (a freshly placed mask still shows its overlay).
/// `None` when nothing runs.
pub fn local_plan(
    adjustments: &[LocalAdjustment],
    overlay_id: Option<&str>,
    geom: &Geometry,
    dims: (u32, u32),
    src: (u32, u32),
) -> Option<LocalPlan> {
    let (w, h) = (f64::from(dims.0), f64::from(dims.1));
    let scale = geom.output_scale(dims.0);
    let diag = f64::from(src.0).hypot(f64::from(src.1));
    let active: Vec<&LocalAdjustment> = adjustments
        .iter()
        .filter(|a| a.enabled && (!a.is_identity() || Some(a.id.as_str()) == overlay_id))
        .take(MAX_LOCAL)
        .collect();
    if active.is_empty() {
        return None;
    }
    let o = geom.orientation.as_f32();
    let c = geom.crop;
    let (sin, cos) = geom.angle_deg.to_radians().sin_cos();
    let mut u: Uniform = vec![
        [dims.0 as f32, dims.1 as f32, active.len() as f32, -1.0],
        o,
        [
            c[0] as f32,
            c[1] as f32,
            (c[2] - c[0]) as f32,
            (c[3] - c[1]) as f32,
        ],
        [
            cos as f32,
            sin as f32,
            geom.canvas.0 as f32,
            geom.canvas.1 as f32,
        ],
        [
            tuning::CONTRAST_SLOPE,
            tuning::WHITES_EV,
            tuning::BLACKS_EV,
            0.0,
        ],
        [
            tuning::WHITES_RANGE[0],
            tuning::WHITES_RANGE[1],
            tuning::BLACKS_RANGE[0],
            tuning::BLACKS_RANGE[1],
        ],
    ];
    let mut zones = Vec::new();
    let mut needs_base = false;
    let mut overlay = None;
    for (i, a) in active.iter().enumerate() {
        let t = a.adjust.unit();
        needs_base |= t[2] != 0.0 || t[3] != 0.0;
        if Some(a.id.as_str()) == overlay_id {
            overlay = Some(i as u32);
        }
        let (kind, layer, m0, feather) = match &a.mask {
            MaskDef::Linear(l) => {
                let [ou, ov] = geom.source_to_output(l.x, l.y);
                // The side the mask affects is the source normal's; carry
                // it over as a vector (a mirror flips a rebuilt normal).
                let (s, c) = l.angle.to_radians().sin_cos();
                let [nx, ny] = geom.source_vector_to_output([-s, c]);
                (
                    0.0,
                    0.0,
                    [(ou * w) as f32, (ov * h) as f32, nx as f32, ny as f32],
                    (l.feather.max(0.0) * diag * scale) as f32,
                )
            }
            MaskDef::Brush { strokes } => {
                zones.push((a.id.clone(), strokes.clone()));
                (1.0, (zones.len() - 1) as f32, [0.0; 4], 0.0)
            }
        };
        u.push([t[0], t[1], t[2], t[3]]);
        u.push([t[4], t[5], kind, layer]);
        u.push(m0);
        u.push([feather, 0.0, 0.0, 0.0]);
    }
    u.resize(6 + MAX_LOCAL * 4, [0.0; 4]);
    Some(LocalPlan {
        uniform: u,
        zones,
        needs_base,
        overlay,
    })
}

#[derive(Debug)]
struct Entry {
    strokes: Vec<Stroke>,
    raster: BrushRaster,
    /// Bumped whenever the raster changes; part of the pass cache key.
    version: u64,
    dirty: bool,
}

/// Brush masks of the open photo: rasterized on the CPU (incrementally
/// while painting), uploaded as layers of one `R8` texture array.
#[derive(Debug, Default)]
pub struct BrushStore {
    entries: HashMap<String, Entry>,
    tex: Option<(u32, wgpu::Texture)>,
    counter: u64,
}

impl BrushStore {
    /// Brings the rasters and the texture up to date with `zones` (layer =
    /// position). Returns a key over the zones' contents.
    pub fn sync(
        &mut self,
        ctx: &GpuContext,
        src: (u32, u32),
        zones: &[(String, Vec<Stroke>)],
    ) -> u64 {
        self.entries
            .retain(|id, _| zones.iter().any(|(z, _)| z == id));
        let mut key = Vec::new();
        for (layer, (id, strokes)) in zones.iter().enumerate() {
            let counter = &mut self.counter;
            let e = self.entries.entry(id.clone()).or_insert_with(|| {
                *counter += 1;
                Entry {
                    strokes: Vec::new(),
                    raster: BrushRaster::new(src),
                    version: *counter,
                    dirty: true,
                }
            });
            if e.strokes != *strokes {
                e.raster.sync(&e.strokes, strokes);
                e.strokes.clone_from(strokes);
                self.counter += 1;
                e.version = self.counter;
                e.dirty = true;
            }
            key.extend(e.version.to_le_bytes());
            key.extend((layer as u64).to_le_bytes());
        }

        let layers = (zones.len() as u32).max(1);
        let (mask_w, mask_h) = BrushRaster::dims(src);
        if self.tex.as_ref().is_none_or(|(n, _)| *n != layers) {
            self.tex = Some((
                layers,
                ctx.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("brush-masks"),
                    size: wgpu::Extent3d {
                        width: mask_w,
                        height: mask_h,
                        depth_or_array_layers: layers,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                }),
            ));
            for e in self.entries.values_mut() {
                e.dirty = true;
            }
        }
        if let Some((_, tex)) = &self.tex {
            for (layer, (id, _)) in zones.iter().enumerate() {
                let Some(e) = self.entries.get_mut(id) else {
                    continue;
                };
                if !e.dirty {
                    continue;
                }
                e.dirty = false;
                ctx.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &e.raster.data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(e.raster.w),
                        rows_per_image: Some(e.raster.h),
                    },
                    wgpu::Extent3d {
                        width: e.raster.w,
                        height: e.raster.h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        xxh3_64(&key)
    }

    /// The array view the kernels bind; a 1×1 placeholder until `sync`
    /// has run.
    pub fn view(&self, device: &wgpu::Device) -> wgpu::TextureView {
        let desc = wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        };
        match &self.tex {
            Some((_, t)) => t.create_view(&desc),
            None => device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("brush-placeholder"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&desc),
        }
    }
}
