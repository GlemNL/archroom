//! The develop pipeline (plan §6.1/§6.2): five GPU stages, each caching its
//! output texture under a key that chains its own params onto its upstream
//! key, so a slider change re-runs only its stage and the ones after it.
//!
//! ```text
//! source(camera RGB f16, full res)
//!   1 geometry   orientation + resample to the proxy
//!   2 scene      camera→Rec.2020, white balance (CAT), exposure; noise
//!                reduction; local highlights/shadows and clarity via
//!                whole-frame guided-filter base layers
//!   3 tone map   profile base curve, contrast, whites, blacks
//!   4 display    tone curve (LUT), HSL / B&W mixer, vibrance, saturation,
//!                B&W (OkLab), sharpening, post-crop vignette
//!   5 output     gamut map, display 3D LUT, dither, clipping overlay → rgba8
//! ```
//!
//! The base layer is computed from the proxy, which today *is* the whole
//! frame; when ROI/tiled rendering arrives (M4/M5) it must come from a
//! whole-frame downscale shared by all tiles (plan §6.10).

use std::time::{Duration, Instant};

use archroom_color::Mat3d;
use archroom_color::cat::Cat;
use archroom_color::cie::{REC2020, SRGB};
use archroom_color::icc::{OutputSpace, build_display_lut};
use archroom_color::oklab::rgb_to_lms;
use archroom_color::temp::temp_tint_to_xy;
use archroom_io::DecodedImage;
use wgpu::util::DeviceExt;
use xxhash_rust::xxh3::xxh3_64;

use crate::error::{Error, Result};
use crate::gpu::{GpuContext, Kernel, Slot, work_texture};
use crate::op::Op;
use crate::ops::{
    BwMix, Clarity, Exposure, Hsl, Noise, Presence, Profile, ProfileName, Sharpen, Tone, ToneCurve,
    Treatment, Vignette, WbMode, WhiteBalance, build_luts,
};
use rayon::prelude::*;

use crate::orientation::Orientation;
use crate::params::EditParams;
use crate::rawprep::SceneColor;
use crate::tone::tone_uniform;

const F16: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const OUT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const OUT16: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Uint;
const LUT_N: u32 = 33;
const BASE_MAX_EDGE: f32 = 1024.0;
/// Guided-filter smoothness in EV² (larger = smoother base layer).
const GUIDED_EPS: f32 = 0.1;

/// One frame to render.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    pub params: EditParams,
    /// Longest edge of the proxy in pixels (never upscales past the source).
    pub max_edge: u32,
    pub orientation: Orientation,
    /// The `J` overlay: red = clipped, blue = crushed.
    pub clip_overlay: bool,
    pub want_histogram: bool,
    /// Render the whole (straightened) canvas instead of the crop, for the
    /// crop tool (plan §6.8).
    pub ignore_crop: bool,
    /// Delivery color space of the output (sRGB for the interactive views).
    pub space: OutputSpace,
    /// Export path: write 16-bit output (read with `read_output_rgba16`)
    /// instead of the dithered 8-bit texture; no overlay or histogram.
    pub depth16: bool,
}

impl RenderRequest {
    pub fn new(params: EditParams, max_edge: u32) -> Self {
        Self {
            params,
            max_edge,
            orientation: Orientation::IDENTITY,
            clip_overlay: false,
            want_histogram: false,
            ignore_crop: false,
            space: OutputSpace::Srgb,
            depth16: false,
        }
    }
}

/// Which stages actually executed for a render (the rest were cache hits).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RanStages {
    pub geometry: bool,
    pub scene: bool,
    pub highlights_shadows: bool,
    pub tone: bool,
    pub display: bool,
    pub output: bool,
    pub noise: bool,
    pub clarity: bool,
    pub curve: bool,
    pub hsl: bool,
    pub sharpen: bool,
    pub vignette: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RenderStats {
    pub ran: RanStages,
    pub width: u32,
    pub height: u32,
    /// Encode + submit + GPU wait, wall clock.
    pub elapsed: Duration,
}

/// 256 bins each of R, G, B and luma of the 8-bit output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    pub bins: [[u32; 256]; 4],
}

#[derive(Debug)]
struct Kernels {
    geometry: Kernel,
    scene: Kernel,
    luma_down: Kernel,
    blur: Kernel,
    guided_ab: Kernel,
    guided_base: Kernel,
    highlights_shadows: Kernel,
    tone: Kernel,
    display: Kernel,
    output: Kernel,
    output16: Kernel,
    histogram: Kernel,
    noise: Kernel,
    clarity: Kernel,
    curve: Kernel,
    hsl: Kernel,
    blur_rgb: Kernel,
    sharpen: Kernel,
    vignette: Kernel,
}

impl Kernels {
    fn new(device: &wgpu::Device) -> Self {
        let io = [
            Slot::Uniform,
            Slot::Tex2d { filterable: false },
            Slot::StorageOut(F16),
        ];
        Self {
            geometry: Kernel::new(
                device,
                "geometry",
                include_str!("shaders/geometry.wgsl"),
                &io,
            ),
            scene: Kernel::new(device, "scene", include_str!("shaders/scene.wgsl"), &io),
            luma_down: Kernel::new(
                device,
                "luma_down",
                include_str!("shaders/luma_down.wgsl"),
                &io,
            ),
            blur: Kernel::new(device, "blur", include_str!("shaders/blur.wgsl"), &io),
            guided_ab: Kernel::new(
                device,
                "guided_ab",
                include_str!("shaders/guided_ab.wgsl"),
                &io,
            ),
            guided_base: Kernel::new(
                device,
                "guided_base",
                include_str!("shaders/guided_base.wgsl"),
                &[
                    Slot::Tex2d { filterable: false },
                    Slot::Tex2d { filterable: false },
                    Slot::StorageOut(F16),
                ],
            ),
            highlights_shadows: Kernel::new(
                device,
                "highlights_shadows",
                include_str!("shaders/highlights_shadows.wgsl"),
                &[
                    Slot::Uniform,
                    Slot::Tex2d { filterable: false },
                    Slot::Tex2d { filterable: true },
                    Slot::Sampler,
                    Slot::StorageOut(F16),
                ],
            ),
            tone: Kernel::new(device, "tone", include_str!("shaders/tone.wgsl"), &io),
            display: Kernel::new(device, "display", include_str!("shaders/display.wgsl"), &io),
            output: Kernel::new(
                device,
                "output",
                include_str!("shaders/output.wgsl"),
                &[
                    Slot::Uniform,
                    Slot::Tex2d { filterable: false },
                    Slot::Tex3d,
                    Slot::Sampler,
                    Slot::StorageOut(OUT),
                ],
            ),
            output16: Kernel::new(
                device,
                "output16",
                include_str!("shaders/output16.wgsl"),
                &[
                    Slot::Uniform,
                    Slot::Tex2d { filterable: false },
                    Slot::Tex3d,
                    Slot::Sampler,
                    Slot::StorageOut(OUT16),
                ],
            ),
            histogram: Kernel::new(
                device,
                "histogram",
                include_str!("shaders/histogram.wgsl"),
                &[
                    Slot::Uniform,
                    Slot::Tex2d { filterable: false },
                    Slot::Storage,
                ],
            ),
            noise: Kernel::new(device, "noise", include_str!("shaders/noise.wgsl"), &io),
            clarity: Kernel::new(
                device,
                "clarity",
                include_str!("shaders/clarity.wgsl"),
                &[
                    Slot::Uniform,
                    Slot::Tex2d { filterable: false },
                    Slot::Tex2d { filterable: true },
                    Slot::Sampler,
                    Slot::StorageOut(F16),
                ],
            ),
            curve: Kernel::new(
                device,
                "curve",
                include_str!("shaders/curve.wgsl"),
                &[
                    Slot::Uniform,
                    Slot::Tex2d { filterable: false },
                    Slot::Tex2d { filterable: false },
                    Slot::StorageOut(F16),
                ],
            ),
            hsl: Kernel::new(device, "hsl", include_str!("shaders/hsl.wgsl"), &io),
            blur_rgb: Kernel::new(
                device,
                "blur_rgb",
                include_str!("shaders/blur_rgb.wgsl"),
                &io,
            ),
            sharpen: Kernel::new(
                device,
                "sharpen",
                include_str!("shaders/sharpen.wgsl"),
                &[
                    Slot::Uniform,
                    Slot::Tex2d { filterable: false },
                    Slot::Tex2d { filterable: false },
                    Slot::StorageOut(F16),
                ],
            ),
            vignette: Kernel::new(
                device,
                "vignette",
                include_str!("shaders/vignette.wgsl"),
                &io,
            ),
        }
    }
}

#[derive(Debug)]
enum SourceKind {
    Raw(SceneColor),
    Rendered,
}

#[derive(Debug)]
struct Source {
    tex: wgpu::Texture,
    w: u32,
    h: u32,
    kind: SourceKind,
}

#[derive(Debug)]
struct Cached {
    key: u64,
    tex: wgpu::Texture,
    w: u32,
    h: u32,
}

/// Makes `slot` hold a `w`×`h` texture tagged `key`; true when the stage
/// must (re)run.
fn ensure(
    device: &wgpu::Device,
    slot: &mut Option<Cached>,
    key: u64,
    (w, h): (u32, u32),
    format: wgpu::TextureFormat,
    extra: wgpu::TextureUsages,
) -> bool {
    if let Some(c) = slot.as_mut()
        && c.w == w
        && c.h == h
    {
        if c.key == key {
            return false;
        }
        c.key = key;
        return true;
    }
    *slot = Some(Cached {
        key,
        tex: work_texture(device, "stage", w, h, format, extra),
        w,
        h,
    });
    true
}

#[derive(Default)]
struct Key(Vec<u8>);

impl Key {
    fn chain(prev: u64) -> Self {
        Key(prev.to_le_bytes().to_vec())
    }
    fn u(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f(&mut self, v: f64) -> &mut Self {
        self.u(v.to_bits())
    }
    fn finish(&self) -> u64 {
        xxh3_64(&self.0)
    }
}

fn uniform(device: &wgpu::Device, data: &[[f32; 4]]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniform"),
        contents: bytemuck::cast_slice(data),
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

fn row3(m: &Mat3d, r: usize, scale: f64) -> [f32; 4] {
    [
        (m.0[r][0] * scale) as f32,
        (m.0[r][1] * scale) as f32,
        (m.0[r][2] * scale) as f32,
        0.0,
    ]
}

/// Runs a one-input, one-output pass into `slot` when `key` changed.
/// Binding order: uniform, input, `extra`..., output. True when it ran.
#[allow(clippy::too_many_arguments)]
fn run_simple(
    device: &wgpu::Device,
    enc: &mut wgpu::CommandEncoder,
    kernel: &Kernel,
    slot: &mut Option<Cached>,
    key: u64,
    dims: (u32, u32),
    uni: &[[f32; 4]],
    input: &wgpu::Texture,
    extra: &[wgpu::BindingResource<'_>],
) -> bool {
    if !ensure(device, slot, key, dims, F16, wgpu::TextureUsages::empty()) {
        return false;
    }
    let Some(o) = slot.as_ref() else {
        return false;
    };
    let u = uniform(device, uni);
    let (iv, ov) = (view(input), view(&o.tex));
    let mut res = vec![
        u.as_entire_binding(),
        wgpu::BindingResource::TextureView(&iv),
    ];
    res.extend(extra.iter().cloned());
    res.push(wgpu::BindingResource::TextureView(&ov));
    kernel.dispatch(device, enc, &res, dims.0, dims.1);
    true
}

/// Stable cache key of any serializable params.
fn json_key<T: serde::Serialize>(v: &T) -> u64 {
    xxh3_64(serde_json::to_string(v).unwrap_or_default().as_bytes())
}

/// The whole-frame guided-filter base layer (edge-aware smooth
/// log-luminance) of `src` at ~1024 px, cached in `slot` under `key`
/// (plan §6.5). `radius_frac` is the filter radius as a fraction of the
/// diagonal, so the look is the same at every zoom. Returns the base
/// texture's dimensions.
#[allow(clippy::too_many_arguments)]
fn guided_base(
    device: &wgpu::Device,
    k: &Kernels,
    enc: &mut wgpu::CommandEncoder,
    src: &wgpu::Texture,
    slot: &mut Option<Cached>,
    key: u64,
    (w, h): (u32, u32),
    radius_frac: f32,
) {
    let bs = (BASE_MAX_EDGE / w.max(h) as f32).min(1.0);
    let bdims = (
        ((w as f32 * bs).round() as u32).max(1),
        ((h as f32 * bs).round() as u32).max(1),
    );
    if !ensure(device, slot, key, bdims, F16, wgpu::TextureUsages::empty()) {
        return;
    }
    let mk = |label: &str| {
        work_texture(
            device,
            label,
            bdims.0,
            bdims.1,
            F16,
            wgpu::TextureUsages::empty(),
        )
    };
    let (la, t1, t2, t3) = (mk("luma"), mk("t1"), mk("t2"), mk("t3"));
    let (bw, bh) = (bdims.0 as f32, bdims.1 as f32);
    let radius = (radius_frac * (bw * bw + bh * bh).sqrt()).round().max(2.0);
    let u = uniform(device, &[[w as f32, h as f32, bw, bh]]);
    k.luma_down.dispatch(
        device,
        enc,
        &[
            u.as_entire_binding(),
            wgpu::BindingResource::TextureView(&view(src)),
            wgpu::BindingResource::TextureView(&view(&la)),
        ],
        bdims.0,
        bdims.1,
    );
    let blur = |enc: &mut wgpu::CommandEncoder,
                src: &wgpu::Texture,
                dst: &wgpu::Texture,
                dir: [f32; 2]| {
        let u = uniform(device, &[[dir[0], dir[1], radius, 0.0]]);
        k.blur.dispatch(
            device,
            enc,
            &[
                u.as_entire_binding(),
                wgpu::BindingResource::TextureView(&view(src)),
                wgpu::BindingResource::TextureView(&view(dst)),
            ],
            bdims.0,
            bdims.1,
        );
    };
    blur(enc, &la, &t1, [1.0, 0.0]);
    blur(enc, &t1, &t2, [0.0, 1.0]);
    let u = uniform(device, &[[GUIDED_EPS, 0.0, 0.0, 0.0]]);
    k.guided_ab.dispatch(
        device,
        enc,
        &[
            u.as_entire_binding(),
            wgpu::BindingResource::TextureView(&view(&t2)),
            wgpu::BindingResource::TextureView(&view(&t1)),
        ],
        bdims.0,
        bdims.1,
    );
    blur(enc, &t1, &t2, [1.0, 0.0]);
    blur(enc, &t2, &t3, [0.0, 1.0]);
    let Some(b) = slot.as_ref() else {
        return;
    };
    k.guided_base.dispatch(
        device,
        enc,
        &[
            wgpu::BindingResource::TextureView(&view(&t3)),
            wgpu::BindingResource::TextureView(&view(&la)),
            wgpu::BindingResource::TextureView(&view(&b.tex)),
        ],
        bdims.0,
        bdims.1,
    );
}

/// The 3D LUT taking display-referred Rec.2020 to `space`'s encoded values.
fn display_lut(ctx: &GpuContext, space: OutputSpace) -> Result<wgpu::Texture> {
    let lut_rgb = build_display_lut(&REC2020, &space.profile()?, LUT_N as usize)?;
    let lut_rgba: Vec<half::f16> = lut_rgb
        .chunks_exact(3)
        .flat_map(|c| {
            [
                half::f16::from_f32(c[0]),
                half::f16::from_f32(c[1]),
                half::f16::from_f32(c[2]),
                half::f16::ONE,
            ]
        })
        .collect();
    Ok(ctx.device.create_texture_with_data(
        &ctx.queue,
        &wgpu::TextureDescriptor {
            label: Some("display-lut"),
            size: wgpu::Extent3d {
                width: LUT_N,
                height: LUT_N,
                depth_or_array_layers: LUT_N,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: F16,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&lut_rgba),
    ))
}

/// The whole develop pipeline for one opened photo.
pub struct Pipeline {
    ctx: GpuContext,
    k: Kernels,
    source: Source,
    lut: wgpu::Texture,
    lut_space: OutputSpace,
    sampler: wgpu::Sampler,
    geo: Option<Cached>,
    scene: Option<Cached>,
    hs: Option<Cached>,
    tone: Option<Cached>,
    display: Option<Cached>,
    out: Option<Cached>,
    out16: Option<Cached>,
    /// Guided-filter base layer of `scene`, keyed by the scene key.
    base: Option<Cached>,
    nr: Option<Cached>,
    base_clarity: Option<Cached>,
    clarity: Option<Cached>,
    curve: Option<Cached>,
    curve_lut: Option<(u64, wgpu::Texture)>,
    hsl: Option<Cached>,
    sharp_blur: Option<Cached>,
    sharp: Option<Cached>,
    vignette: Option<Cached>,
    hist_buf: wgpu::Buffer,
    hist_read: wgpu::Buffer,
    hist_key: Option<u64>,
    histogram: Option<Histogram>,
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("source", &(self.source.w, self.source.h))
            .finish_non_exhaustive()
    }
}

impl Pipeline {
    /// Uploads `image` (once per opened photo) and builds the kernels.
    pub fn new(ctx: &GpuContext, image: &DecodedImage) -> Result<Self> {
        let (rgb, kind) = match image {
            DecodedImage::SceneLinear { rgb, camera } => (
                rgb,
                SourceKind::Raw(SceneColor::new(camera).ok_or(Error::BadCameraMatrix)?),
            ),
            DecodedImage::Rendered { rgb, .. } => (rgb, SourceKind::Rendered),
        };
        let device = &ctx.device;
        let max = device.limits().max_texture_dimension_2d;
        if rgb.width > max || rgb.height > max {
            return Err(Error::TooLarge {
                w: rgb.width,
                h: rgb.height,
                max,
            });
        }

        let linearize = matches!(kind, SourceKind::Rendered);
        let table: Vec<f32> = (0..=4096)
            .map(|i| archroom_color::srgb_eotf(i as f32 / 4096.0))
            .collect();
        let ch = rgb.channels.max(3) as usize;
        let px = (rgb.width as usize) * (rgb.height as usize);
        let mut data = vec![half::f16::ZERO; px * 4];
        // Converting 24 MP one sample at a time takes a couple of hundred
        // ms; spread it over the cores.
        data.par_chunks_mut(4 * 4096)
            .zip(rgb.data.par_chunks(ch * 4096))
            .for_each(|(out, src)| {
                for (o, p) in out.chunks_exact_mut(4).zip(src.chunks_exact(ch)) {
                    for c in 0..3 {
                        let v = p[c];
                        let v = if linearize {
                            let x = v.clamp(0.0, 1.0) * 4096.0;
                            let i = (x as usize).min(4095);
                            let f = x - i as f32;
                            table[i] * (1.0 - f) + table[i + 1] * f
                        } else {
                            v
                        };
                        o[c] = half::f16::from_f32(v);
                    }
                    o[3] = half::f16::ONE;
                }
            });
        let tex = device.create_texture_with_data(
            &ctx.queue,
            &wgpu::TextureDescriptor {
                label: Some("source"),
                size: wgpu::Extent3d {
                    width: rgb.width,
                    height: rgb.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: F16,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&data),
        );

        let lut = display_lut(ctx, OutputSpace::Srgb)?;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let hist_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("histogram"),
            size: 4096,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let hist_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("histogram-read"),
            size: 4096,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            k: Kernels::new(device),
            ctx: ctx.clone(),
            source: Source {
                tex,
                w: rgb.width,
                h: rgb.height,
                kind,
            },
            lut,
            lut_space: OutputSpace::Srgb,
            sampler,
            geo: None,
            out16: None,
            scene: None,
            hs: None,
            tone: None,
            display: None,
            out: None,
            base: None,
            nr: None,
            base_clarity: None,
            clarity: None,
            curve: None,
            curve_lut: None,
            hsl: None,
            sharp_blur: None,
            sharp: None,
            vignette: None,
            hist_buf,
            hist_read,
            hist_key: None,
            histogram: None,
        })
    }

    /// The as-shot (Temp K, Tint) of a raw; `None` for rendered files.
    pub fn as_shot_temp_tint(&self) -> Option<(f64, f64)> {
        match &self.source.kind {
            SourceKind::Raw(sc) => Some(sc.as_shot_temp_tint()),
            SourceKind::Rendered => None,
        }
    }

    pub fn source_size(&self) -> (u32, u32) {
        (self.source.w, self.source.h)
    }

    pub fn is_raw(&self) -> bool {
        matches!(self.source.kind, SourceKind::Raw(_))
    }

    /// The last rendered output (`Rgba8Unorm`); the app draws this texture.
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.out.as_ref().map(|c| &c.tex)
    }

    pub fn histogram(&self) -> Option<&Histogram> {
        self.histogram.as_ref()
    }

    fn scene_matrix(&self, params: &EditParams) -> Mat3d {
        let gain = 2f64.powf(params.get::<Exposure>().ev.clamp(-5.0, 5.0));
        let m = match &self.source.kind {
            SourceKind::Raw(sc) => {
                let wb = params.get::<WhiteBalance>();
                let illuminant = match wb.mode {
                    WbMode::AsShot => sc.as_shot_white(),
                    WbMode::Custom => temp_tint_to_xy(wb.temp, wb.tint),
                };
                sc.decoded_to_working(illuminant, Cat::Cat16)
            }
            SourceKind::Rendered => REC2020.xyz_to_rgb().mul(&SRGB.rgb_to_xyz()),
        };
        Mat3d::diag([gain; 3]).mul(&m)
    }

    /// Renders `req`, running only the stages whose inputs changed.
    pub fn render(&mut self, req: &RenderRequest) -> Result<RenderStats> {
        let started = Instant::now();
        if self.lut_space != req.space {
            self.lut = display_lut(&self.ctx, req.space)?;
            self.lut_space = req.space;
            self.out = None;
            self.out16 = None;
        }
        let scene_m = self.scene_matrix(&req.params);
        let rendered = matches!(self.source.kind, SourceKind::Rendered);
        let Self {
            ctx,
            k,
            source,
            lut,
            sampler,
            geo,
            scene,
            hs,
            tone,
            display,
            out,
            out16,
            base,
            nr,
            base_clarity,
            clarity,
            curve,
            curve_lut,
            hsl,
            sharp_blur,
            sharp,
            vignette,
            ..
        } = self;
        let device = &ctx.device;
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render"),
        });
        let mut ran = RanStages::default();

        // Stage 1: geometry (orientation, straighten, crop, resample).
        let geom = crate::geometry::resolve(
            (source.w, source.h),
            req.orientation,
            &req.params,
            req.ignore_crop,
        );
        let (cw, ch) = geom.crop_px();
        let scale = (req.max_edge.max(1) as f64 / cw.max(ch)).min(1.0);
        let dims = (
            ((cw * scale).round() as u32).max(1),
            ((ch * scale).round() as u32).max(1),
        );
        let (w, h) = dims;
        let (ow, oh) = geom.canvas;
        let orient = geom.orientation;
        let mut kg = Key::default();
        kg.u(w as u64).u(h as u64);
        for row in orient.0 {
            for v in row {
                kg.u(v as u64);
            }
        }
        kg.f(geom.angle_deg);
        for v in geom.crop {
            kg.f(v);
        }
        let k_geo = kg.finish();
        if ensure(device, geo, k_geo, dims, F16, wgpu::TextureUsages::empty()) {
            let o = orient.as_f32();
            let (sin, cos) = geom.angle_deg.to_radians().sin_cos();
            let c = geom.crop;
            let u = uniform(
                device,
                &[
                    [source.w as f32, source.h as f32, w as f32, h as f32],
                    o,
                    [
                        c[0] as f32,
                        c[1] as f32,
                        (c[2] - c[0]) as f32,
                        (c[3] - c[1]) as f32,
                    ],
                    [cos as f32, sin as f32, ow as f32, oh as f32],
                ],
            );
            let (Some(g), sv) = (geo.as_ref(), view(&source.tex)) else {
                unreachable!()
            };
            k.geometry.dispatch(
                device,
                &mut enc,
                &[
                    u.as_entire_binding(),
                    wgpu::BindingResource::TextureView(&sv),
                    wgpu::BindingResource::TextureView(&view(&g.tex)),
                ],
                w,
                h,
            );
            ran.geometry = true;
        }
        let Some(geo_tex) = geo.as_ref() else {
            return Err(Error::Readback("geometry missing".into()));
        };

        // Stage 2a: scene matrix (white balance + exposure).
        let mut ks = Key::chain(k_geo);
        for r in 0..3 {
            for c in 0..3 {
                ks.f(scene_m.0[r][c]);
            }
        }
        let k_scene = ks.finish();
        if ensure(
            device,
            scene,
            k_scene,
            dims,
            F16,
            wgpu::TextureUsages::empty(),
        ) {
            let u = uniform(
                device,
                &[
                    row3(&scene_m, 0, 1.0),
                    row3(&scene_m, 1, 1.0),
                    row3(&scene_m, 2, 1.0),
                    [w as f32, h as f32, 0.0, 0.0],
                ],
            );
            let Some(s) = scene.as_ref() else {
                unreachable!()
            };
            k.scene.dispatch(
                device,
                &mut enc,
                &[
                    u.as_entire_binding(),
                    wgpu::BindingResource::TextureView(&view(&geo_tex.tex)),
                    wgpu::BindingResource::TextureView(&view(&s.tex)),
                ],
                w,
                h,
            );
            ran.scene = true;
        }
        let Some(scene_tex) = scene.as_ref() else {
            return Err(Error::Readback("scene missing".into()));
        };

        // Stage 2a': noise reduction (scene-linear, before local tone).
        let noise_p = req.params.get::<Noise>();
        let mut k_scene_in = k_scene;
        let scene_in: &Cached = if Noise::is_identity(&noise_p) {
            scene_tex
        } else {
            let mut kn = Key::chain(k_scene);
            kn.f(noise_p.luma)
                .f(noise_p.luma_detail)
                .f(noise_p.luma_contrast)
                .f(noise_p.color)
                .f(noise_p.color_detail)
                .f(noise_p.color_smoothness);
            k_scene_in = kn.finish();
            // Pixel-scale op: radii follow the proxy's scale, so it is only
            // exact at 1:1 (plan §6.7).
            let zoom = (w as f32 / cw as f32).clamp(0.35, 1.0);
            let pct = |v: f64| (v / 100.0) as f32;
            ran.noise = run_simple(
                device,
                &mut enc,
                &k.noise,
                nr,
                k_scene_in,
                dims,
                &[
                    [
                        pct(noise_p.luma),
                        pct(noise_p.luma_detail),
                        pct(noise_p.luma_contrast),
                        zoom,
                    ],
                    [
                        pct(noise_p.color),
                        pct(noise_p.color_detail),
                        pct(noise_p.color_smoothness),
                        0.0,
                    ],
                    [w as f32, h as f32, 0.0, 0.0],
                ],
                &scene_tex.tex,
                &[],
            );
            let Some(n) = nr.as_ref() else {
                return Err(Error::Readback("noise reduction missing".into()));
            };
            n
        };

        // Stage 2b: local highlights/shadows.
        let tone_p = req.params.get::<Tone>();
        let (shadows, highlights) = (
            tone_p.shadows.clamp(-100.0, 100.0) / 100.0,
            tone_p.highlights.clamp(-100.0, 100.0) / 100.0,
        );
        let use_hs = shadows != 0.0 || highlights != 0.0;
        let mut k_after_scene = k_scene_in;
        if use_hs {
            let mut kh = Key::chain(k_scene_in);
            kh.f(shadows).f(highlights);
            k_after_scene = kh.finish();
            guided_base(
                device,
                k,
                &mut enc,
                &scene_in.tex,
                base,
                k_scene_in,
                dims,
                0.02,
            );
            let Some(b) = base.as_ref() else {
                return Err(Error::Readback("base layer missing".into()));
            };
            ran.highlights_shadows = run_simple(
                device,
                &mut enc,
                &k.highlights_shadows,
                hs,
                k_after_scene,
                dims,
                &[[shadows as f32, highlights as f32, w as f32, h as f32]],
                &scene_in.tex,
                &[
                    wgpu::BindingResource::TextureView(&view(&b.tex)),
                    wgpu::BindingResource::Sampler(sampler),
                ],
            );
        }
        let hs_out: &Cached = if use_hs {
            let Some(hh) = hs.as_ref() else {
                return Err(Error::Readback("highlights/shadows missing".into()));
            };
            hh
        } else {
            scene_in
        };

        // Stage 2c: clarity (medium-scale detail boost).
        let clarity_p = req.params.get::<Clarity>();
        let mut k_tone_in = k_after_scene;
        let tone_in: &Cached = if Clarity::is_identity(&clarity_p) {
            hs_out
        } else {
            let amount = (clarity_p.clarity / 100.0).clamp(-1.0, 1.0);
            let mut kc = Key::chain(k_after_scene);
            kc.f(amount);
            k_tone_in = kc.finish();
            guided_base(
                device,
                k,
                &mut enc,
                &hs_out.tex,
                base_clarity,
                k_after_scene,
                dims,
                0.03,
            );
            let Some(b) = base_clarity.as_ref() else {
                return Err(Error::Readback("clarity base missing".into()));
            };
            ran.clarity = run_simple(
                device,
                &mut enc,
                &k.clarity,
                clarity,
                k_tone_in,
                dims,
                &[[amount as f32, w as f32, h as f32, 0.0]],
                &hs_out.tex,
                &[
                    wgpu::BindingResource::TextureView(&view(&b.tex)),
                    wgpu::BindingResource::Sampler(sampler),
                ],
            );
            let Some(c) = clarity.as_ref() else {
                return Err(Error::Readback("clarity missing".into()));
            };
            c
        };

        // Stage 3: tone map.
        let profile = req.params.get::<Profile>();
        let tu = tone_uniform(&profile, &tone_p, rendered);
        let mut kt = Key::chain(k_tone_in);
        for v in tu.abcd.iter().chain(&tu.efwb).chain(&tu.cbm) {
            kt.f(*v as f64);
        }
        let k_tone = kt.finish();
        if ensure(
            device,
            tone,
            k_tone,
            dims,
            F16,
            wgpu::TextureUsages::empty(),
        ) {
            let u = uniform(
                device,
                &[tu.abcd, tu.efwb, tu.cbm, [w as f32, h as f32, 0.0, 0.0]],
            );
            let Some(t) = tone.as_ref() else {
                unreachable!()
            };
            k.tone.dispatch(
                device,
                &mut enc,
                &[
                    u.as_entire_binding(),
                    wgpu::BindingResource::TextureView(&view(&tone_in.tex)),
                    wgpu::BindingResource::TextureView(&view(&t.tex)),
                ],
                w,
                h,
            );
            ran.tone = true;
        }
        let Some(tone_tex) = tone.as_ref() else {
            return Err(Error::Readback("tone map missing".into()));
        };

        // Stage 4a: tone curve (parametric + point curves as LUTs).
        let curve_p = req.params.get::<ToneCurve>();
        let mut k_curve = k_tone;
        let curved: &Cached = if ToneCurve::is_identity(&curve_p) {
            tone_tex
        } else {
            let curve_key = json_key(&curve_p);
            k_curve = Key::chain(k_tone).u(curve_key).finish();
            if curve_lut.as_ref().is_none_or(|(k0, _)| *k0 != curve_key) {
                let luts = build_luts(&curve_p);
                let mut texels: Vec<f32> = Vec::with_capacity(luts[0].len() * 4);
                for ((r, g), b) in luts[0].iter().zip(&luts[1]).zip(&luts[2]) {
                    texels.extend([*r, *g, *b, 1.0]);
                }
                let tex = device.create_texture_with_data(
                    &ctx.queue,
                    &wgpu::TextureDescriptor {
                        label: Some("curve-lut"),
                        size: wgpu::Extent3d {
                            width: luts[0].len() as u32,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba32Float,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    bytemuck::cast_slice(&texels),
                );
                *curve_lut = Some((curve_key, tex));
            }
            let Some((_, lut_tex)) = curve_lut.as_ref() else {
                return Err(Error::Readback("curve LUT missing".into()));
            };
            ran.curve = run_simple(
                device,
                &mut enc,
                &k.curve,
                curve,
                k_curve,
                dims,
                &[[w as f32, h as f32, 0.0, 0.0]],
                &tone_tex.tex,
                &[wgpu::BindingResource::TextureView(&view(lut_tex))],
            );
            let Some(c) = curve.as_ref() else {
                return Err(Error::Readback("curve missing".into()));
            };
            c
        };

        // Stage 4b: HSL / B&W mixer.
        let bw =
            profile.treatment == Treatment::BlackWhite || profile.name == ProfileName::Monochrome;
        let hsl_p = req.params.get::<Hsl>();
        let bw_p = req.params.get::<BwMix>();
        let hsl_active = if bw {
            !BwMix::is_identity(&bw_p)
        } else {
            !Hsl::is_identity(&hsl_p)
        };
        let mut k_hsl = k_curve;
        let mixed: &Cached = if !hsl_active {
            curved
        } else {
            let (hue, sat, lum) = if bw {
                ([0.0; 8], [0.0; 8], bw_p.mix)
            } else {
                (hsl_p.hue, hsl_p.sat, hsl_p.lum)
            };
            let pack = |v: [f64; 8]| -> [[f32; 4]; 2] {
                let f = |i: usize| (v[i] / 100.0).clamp(-1.0, 1.0) as f32;
                [[f(0), f(1), f(2), f(3)], [f(4), f(5), f(6), f(7)]]
            };
            let (hp, sp, lp) = (pack(hue), pack(sat), pack(lum));
            let mut kh = Key::chain(k_curve);
            kh.u(bw as u64);
            for v in hue.iter().chain(&sat).chain(&lum) {
                kh.f(*v);
            }
            k_hsl = kh.finish();
            let to_lms = rgb_to_lms(&REC2020);
            let from_lms = to_lms.inverse().unwrap_or(Mat3d::IDENTITY);
            ran.hsl = run_simple(
                device,
                &mut enc,
                &k.hsl,
                hsl,
                k_hsl,
                dims,
                &[
                    row3(&to_lms, 0, 1.0),
                    row3(&to_lms, 1, 1.0),
                    row3(&to_lms, 2, 1.0),
                    row3(&from_lms, 0, 1.0),
                    row3(&from_lms, 1, 1.0),
                    row3(&from_lms, 2, 1.0),
                    hp[0],
                    hp[1],
                    sp[0],
                    sp[1],
                    lp[0],
                    lp[1],
                    [w as f32, h as f32, 0.0, 0.0],
                ],
                &curved.tex,
                &[],
            );
            let Some(m) = hsl.as_ref() else {
                return Err(Error::Readback("hsl missing".into()));
            };
            m
        };

        // Stage 4c: vibrance, saturation, B&W (OkLab).
        let pres = req.params.get::<Presence>();
        let mut kd = Key::chain(k_hsl);
        kd.f(pres.vibrance).f(pres.saturation).u(bw as u64);
        let k_display = kd.finish();
        if ensure(
            device,
            display,
            k_display,
            dims,
            F16,
            wgpu::TextureUsages::empty(),
        ) {
            let to_lms = rgb_to_lms(&REC2020);
            let from_lms = to_lms.inverse().unwrap_or(Mat3d::IDENTITY);
            let u = uniform(
                device,
                &[
                    row3(&to_lms, 0, 1.0),
                    row3(&to_lms, 1, 1.0),
                    row3(&to_lms, 2, 1.0),
                    row3(&from_lms, 0, 1.0),
                    row3(&from_lms, 1, 1.0),
                    row3(&from_lms, 2, 1.0),
                    [
                        (pres.vibrance / 100.0) as f32,
                        (pres.saturation / 100.0) as f32,
                        bw as u32 as f32,
                        0.0,
                    ],
                    [w as f32, h as f32, 0.0, 0.0],
                ],
            );
            let Some(d) = display.as_ref() else {
                unreachable!()
            };
            k.display.dispatch(
                device,
                &mut enc,
                &[
                    u.as_entire_binding(),
                    wgpu::BindingResource::TextureView(&view(&mixed.tex)),
                    wgpu::BindingResource::TextureView(&view(&d.tex)),
                ],
                w,
                h,
            );
            ran.display = true;
        }
        let Some(display_tex) = display.as_ref() else {
            return Err(Error::Readback("display missing".into()));
        };

        // Stage 4d: sharpening (unsharp mask on luminance).
        let sharp_p = req.params.get::<Sharpen>();
        let mut k_fx = k_display;
        let sharpened: &Cached = if Sharpen::is_identity(&sharp_p) {
            display_tex
        } else {
            // The radius is in source pixels; scale it to the proxy.
            let sigma = (sharp_p.radius as f32 * (w as f32 / cw as f32)).clamp(0.35, 24.0);
            let k_blur = Key::chain(k_display).f(sigma as f64).finish();
            if ensure(
                device,
                sharp_blur,
                k_blur,
                dims,
                F16,
                wgpu::TextureUsages::empty(),
            ) {
                let tmp =
                    work_texture(device, "sharp-tmp", w, h, F16, wgpu::TextureUsages::empty());
                let Some(sb) = sharp_blur.as_ref() else {
                    unreachable!()
                };
                for (src_tex, dst_tex, dir) in [
                    (&display_tex.tex, &tmp, [1.0, 0.0]),
                    (&tmp, &sb.tex, [0.0, 1.0]),
                ] {
                    let u = uniform(device, &[[dir[0], dir[1], sigma, 0.0]]);
                    k.blur_rgb.dispatch(
                        device,
                        &mut enc,
                        &[
                            u.as_entire_binding(),
                            wgpu::BindingResource::TextureView(&view(src_tex)),
                            wgpu::BindingResource::TextureView(&view(dst_tex)),
                        ],
                        w,
                        h,
                    );
                }
            }
            let Some(sb) = sharp_blur.as_ref() else {
                return Err(Error::Readback("sharpen blur missing".into()));
            };
            let mut ksh = Key::chain(k_blur);
            ksh.f(sharp_p.amount).f(sharp_p.detail).f(sharp_p.masking);
            k_fx = ksh.finish();
            ran.sharpen = run_simple(
                device,
                &mut enc,
                &k.sharpen,
                sharp,
                k_fx,
                dims,
                &[
                    [
                        (sharp_p.amount / 100.0) as f32,
                        (sharp_p.detail / 100.0) as f32,
                        (sharp_p.masking / 100.0) as f32,
                        0.0,
                    ],
                    [w as f32, h as f32, 0.0, 0.0],
                ],
                &display_tex.tex,
                &[wgpu::BindingResource::TextureView(&view(&sb.tex))],
            );
            let Some(sh) = sharp.as_ref() else {
                return Err(Error::Readback("sharpen missing".into()));
            };
            sh
        };

        // Stage 4e: post-crop vignette.
        let vig_p = req.params.get::<Vignette>();
        let final_tex: &Cached = if Vignette::is_identity(&vig_p) {
            sharpened
        } else {
            let mut kv = Key::chain(k_fx);
            kv.f(vig_p.amount)
                .f(vig_p.midpoint)
                .f(vig_p.roundness)
                .f(vig_p.feather)
                .f(vig_p.highlights);
            k_fx = kv.finish();
            let pct = |v: f64| (v / 100.0) as f32;
            ran.vignette = run_simple(
                device,
                &mut enc,
                &k.vignette,
                vignette,
                k_fx,
                dims,
                &[
                    [
                        pct(vig_p.amount),
                        pct(vig_p.midpoint),
                        pct(vig_p.roundness),
                        pct(vig_p.feather),
                    ],
                    [pct(vig_p.highlights), 0.0, 0.0, 0.0],
                    [w as f32, h as f32, 0.0, 0.0],
                ],
                &sharpened.tex,
                &[],
            );
            let Some(v) = vignette.as_ref() else {
                return Err(Error::Readback("vignette missing".into()));
            };
            v
        };

        // Stage 5: output transform.
        let mut ko = Key::chain(k_fx);
        ko.u(req.clip_overlay as u64)
            .u(LUT_N as u64)
            .u(req.space as u64)
            .u(req.depth16 as u64);
        let k_out = ko.finish();
        let lut_view = lut.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });
        if req.depth16 {
            if ensure(
                device,
                out16,
                k_out,
                dims,
                OUT16,
                wgpu::TextureUsages::COPY_SRC,
            ) {
                let u = uniform(device, &[[w as f32, h as f32, 0.0, LUT_N as f32]]);
                let Some(o) = out16.as_ref() else {
                    unreachable!()
                };
                k.output16.dispatch(
                    device,
                    &mut enc,
                    &[
                        u.as_entire_binding(),
                        wgpu::BindingResource::TextureView(&view(&final_tex.tex)),
                        wgpu::BindingResource::TextureView(&lut_view),
                        wgpu::BindingResource::Sampler(sampler),
                        wgpu::BindingResource::TextureView(&view(&o.tex)),
                    ],
                    w,
                    h,
                );
                ran.output = true;
            }
        } else if ensure(device, out, k_out, dims, OUT, wgpu::TextureUsages::COPY_SRC) {
            let u = uniform(
                device,
                &[[
                    w as f32,
                    h as f32,
                    req.clip_overlay as u32 as f32,
                    LUT_N as f32,
                ]],
            );
            let Some(o) = out.as_ref() else {
                unreachable!()
            };
            k.output.dispatch(
                device,
                &mut enc,
                &[
                    u.as_entire_binding(),
                    wgpu::BindingResource::TextureView(&view(&final_tex.tex)),
                    wgpu::BindingResource::TextureView(&lut_view),
                    wgpu::BindingResource::Sampler(sampler),
                    wgpu::BindingResource::TextureView(&view(&o.tex)),
                ],
                w,
                h,
            );
            ran.output = true;
        }

        // Histogram of the 8-bit output.
        let want_hist = req.want_histogram && !req.depth16 && self.hist_key != Some(k_out);
        if want_hist && let Some(o) = out.as_ref() {
            enc.clear_buffer(&self.hist_buf, 0, None);
            let u = uniform(device, &[[w as f32, h as f32, 0.0, 0.0]]);
            k.histogram.dispatch(
                device,
                &mut enc,
                &[
                    u.as_entire_binding(),
                    wgpu::BindingResource::TextureView(&view(&o.tex)),
                    self.hist_buf.as_entire_binding(),
                ],
                w,
                h,
            );
            enc.copy_buffer_to_buffer(&self.hist_buf, 0, &self.hist_read, 0, 4096);
        }

        ctx.queue.submit(std::iter::once(enc.finish()));
        if want_hist {
            let bytes = read_buffer(device, &self.hist_read, 4096)?;
            let mut hist = Histogram {
                bins: [[0; 256]; 4],
            };
            for (i, v) in bytes.chunks_exact(4).enumerate() {
                hist.bins[i / 256][i % 256] = u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
            }
            self.histogram = Some(hist);
            self.hist_key = Some(k_out);
        } else {
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        Ok(RenderStats {
            ran,
            width: w,
            height: h,
            elapsed: started.elapsed(),
        })
    }

    /// Reads the output back as tightly packed RGBA8 (for the CLI and tests).
    pub fn read_output_rgba8(&self) -> Result<(u32, u32, Vec<u8>)> {
        self.read_texture(self.out.as_ref(), 4)
    }

    /// Reads a `depth16` render back as tightly packed RGBA, 16 bits per
    /// channel (the alpha channel is always full).
    pub fn read_output_rgba16(&self) -> Result<(u32, u32, Vec<u16>)> {
        let (w, h, bytes) = self.read_texture(self.out16.as_ref(), 8)?;
        let px = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        Ok((w, h, px))
    }

    fn read_texture(&self, o: Option<&Cached>, bpp: u32) -> Result<(u32, u32, Vec<u8>)> {
        let Some(o) = o else {
            return Err(Error::Readback("nothing rendered yet".into()));
        };
        let device = &self.ctx.device;
        let row = (o.w * bpp).next_multiple_of(256);
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * o.h) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("readback"),
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &o.tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(o.h),
                },
            },
            wgpu::Extent3d {
                width: o.w,
                height: o.h,
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue.submit(std::iter::once(enc.finish()));
        let padded = read_buffer(device, &buf, (row * o.h) as u64)?;
        let line = (o.w * bpp) as usize;
        let mut out = Vec::with_capacity(line * o.h as usize);
        for y in 0..o.h as usize {
            let s = y * row as usize;
            out.extend_from_slice(&padded[s..s + line]);
        }
        Ok((o.w, o.h, out))
    }
}

fn read_buffer(device: &wgpu::Device, buf: &wgpu::Buffer, size: u64) -> Result<Vec<u8>> {
    let slice = buf.slice(..size);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv()
        .map_err(|e| Error::Readback(e.to_string()))?
        .map_err(|e| Error::Readback(e.to_string()))?;
    let data = slice.get_mapped_range().to_vec();
    buf.unmap();
    Ok(data)
}
