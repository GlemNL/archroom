// M0 spike shader (plan roadmap): exposure + sRGB output on a decoded raw.
// Validates that the egui/wgpu stack (D1) can show a GPU-rendered image
// with a live slider. The real tone/color pipeline (stages 2-5, plan §6.2)
// replaces this in M3.

struct Params {
    exposure_ev: f32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Full-screen triangle: no vertex buffer needed.
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );
    var out: VertexOutput;
    out.clip_position = vec4<f32>(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

fn srgb_oetf(c: f32) -> f32 {
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let linear = textureSample(src_tex, src_sampler, in.uv).rgb;
    let exposed = clamp(linear * exp2(params.exposure_ev), vec3<f32>(0.0), vec3<f32>(1.0));
    let encoded = vec3<f32>(
        srgb_oetf(exposed.r),
        srgb_oetf(exposed.g),
        srgb_oetf(exposed.b),
    );
    return vec4<f32>(encoded, 1.0);
}
