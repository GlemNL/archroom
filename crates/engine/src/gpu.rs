//! GPU plumbing shared by the pipeline: the device context and a small
//! compute-kernel helper (one WGSL file, an explicit bind-group layout).

use std::sync::Arc;

/// The device the engine runs on: either its own headless one (CLI, tests)
/// or the app's (the same wgpu device egui draws with — plan D1).
#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: Arc<str>,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuContext")
            .field("adapter", &self.adapter_name)
            .finish()
    }
}

impl GpuContext {
    /// Wraps an existing device (the app hands over egui's).
    pub fn from_parts(device: wgpu::Device, queue: wgpu::Queue, adapter_name: &str) -> Self {
        Self {
            device,
            queue,
            adapter_name: adapter_name.into(),
        }
    }

    /// Names of the Vulkan adapters (for the Preferences GPU picker).
    pub fn adapter_names() -> Vec<String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        instance
            .enumerate_adapters(wgpu::Backends::VULKAN)
            .into_iter()
            .map(|a| a.get_info().name)
            .collect()
    }

    /// A headless Vulkan device. `VIBEROOM_ADAPTER=<substring>` picks an
    /// adapter by name (e.g. `llvmpipe` for the software fallback used by
    /// CI); otherwise the high-performance one. `None` when there is none.
    pub fn headless() -> Option<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let wanted = std::env::var("VIBEROOM_ADAPTER").ok();
        let adapter = match &wanted {
            Some(name) => instance
                .enumerate_adapters(wgpu::Backends::VULKAN)
                .into_iter()
                .find(|a| {
                    a.get_info()
                        .name
                        .to_lowercase()
                        .contains(&name.to_lowercase())
                }),
            None => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }))
            .ok(),
        }?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("viberoom-engine"),
            // A 100 MP raw is wider than the 8192 default.
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .ok()?;
        Some(Self {
            device,
            queue,
            adapter_name: info.name.into(),
        })
    }
}

/// What a kernel binds at one slot.
#[derive(Debug, Clone, Copy)]
pub enum Slot {
    Uniform,
    /// A sampled 2D float texture (`textureLoad` or, if `filterable`, sampled).
    Tex2d {
        filterable: bool,
    },
    /// A sampled 2D float texture array (one layer per brush zone).
    Tex2dArray {
        filterable: bool,
    },
    Tex3d,
    Sampler,
    /// Write-only storage texture of the given format.
    StorageOut(wgpu::TextureFormat),
    /// Read-write storage buffer.
    Storage,
}

#[derive(Debug)]
pub struct Kernel {
    pub pipeline: wgpu::ComputePipeline,
    pub layout: wgpu::BindGroupLayout,
}

impl Kernel {
    pub fn new(device: &wgpu::Device, label: &str, wgsl: &str, slots: &[Slot]) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let entries: Vec<wgpu::BindGroupLayoutEntry> = slots
            .iter()
            .enumerate()
            .map(|(i, slot)| wgpu::BindGroupLayoutEntry {
                binding: i as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: match *slot {
                    Slot::Uniform => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    Slot::Tex2d { filterable } => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    Slot::Tex2dArray { filterable } => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    Slot::Tex3d => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    Slot::Sampler => {
                        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                    }
                    Slot::StorageOut(format) => wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    Slot::Storage => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &entries,
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&pl),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    /// Records one dispatch covering `w`×`h` invocations (8×8 groups).
    pub fn dispatch(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        resources: &[wgpu::BindingResource<'_>],
        w: u32,
        h: u32,
    ) {
        let entries: Vec<wgpu::BindGroupEntry> = resources
            .iter()
            .enumerate()
            .map(|(i, r)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: r.clone(),
            })
            .collect();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &entries,
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
    }
}

/// A 2D texture the pipeline reads with `textureLoad` and writes as storage.
pub fn work_texture(
    device: &wgpu::Device,
    label: &str,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    extra: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | extra,
        view_formats: &[],
    })
}
