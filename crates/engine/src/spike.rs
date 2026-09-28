//! M0 spike (plan roadmap): "decode a raw with LibRaw → linear f32 → wgpu
//! upload → WGSL exposure and sRGB output → shown in an egui paint callback
//! with a live slider. Measure latency." This validates D1 (egui/eframe on
//! wgpu, sharing one GPU device with the engine) before the real `Op`
//! registry and stage pipeline (M3) are designed. It is throwaway: M3's
//! pipeline replaces it rather than growing out of it.

use std::time::{Duration, Instant};

use archroom_io::ImageF32;
use wgpu::util::DeviceExt;

const SHADER_SRC: &str = include_str!("spike.wgsl");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    exposure_ev: f32,
    _pad: [f32; 3],
}

/// Owns the GPU resources for the spike: the uploaded source image, the
/// render pipeline, and an `Rgba8Unorm` output texture registered with
/// egui so it can be drawn with `ui.image(..)`.
pub struct SpikePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl std::fmt::Debug for SpikePipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpikePipeline")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl SpikePipeline {
    /// Uploads `image` and builds the pipeline. Call once per opened photo.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, image: &ImageF32) -> Self {
        let size = wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        };

        // Rgba32Float isn't filterable on most hardware without the
        // FLOAT32_FILTERABLE feature; Rgba16Float is filterable by default
        // and matches the plan's own precision strategy (§6.10).
        let data_f16: Vec<half::f16> = image.data.iter().map(|&v| half::f16::from_f32(v)).collect();
        let src_texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("archroom-spike-src"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&data_f16),
        );
        let src_view = src_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("archroom-spike-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let params = Params {
            exposure_ev: 0.0,
            _pad: [0.0; 3],
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("archroom-spike-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("archroom-spike-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("archroom-spike-bind-group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("archroom-spike-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("archroom-spike-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let output_format = wgpu::TextureFormat::Rgba8Unorm; // required by egui_wgpu::register_native_texture
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("archroom-spike-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("archroom-spike-output"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: output_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            pipeline,
            bind_group,
            uniform_buffer,
            output_texture,
            output_view,
            width: image.width,
            height: image.height,
        }
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output_view
    }

    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output_texture
    }

    /// Re-runs the shader with a new exposure value and blocks until the
    /// GPU finishes, returning wall-clock latency (submit + execute; not a
    /// GPU timestamp-query measurement — see plan §13 for that, later).
    pub fn render(&self, device: &wgpu::Device, queue: &wgpu::Queue, exposure_ev: f32) -> Duration {
        let start = Instant::now();

        let params = Params {
            exposure_ev,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&params));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("archroom-spike-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("archroom-spike-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
        let _ = device.poll(wgpu::PollType::wait_indefinitely());

        start.elapsed()
    }
}
