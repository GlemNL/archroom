// Stage 5, export variant: same gamut handling and display LUT as
// `output.wgsl`, written as 16-bit unsigned integers with no dither (the
// quantisation step is 1/65535) and no overlay.
struct P { v: vec4<f32>, }; // w, h, unused, lut_n
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var lut: texture_3d<f32>;
@group(0) @binding(3) var smp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16uint, write>;

fn oetf(x: f32) -> f32 {
    let v = clamp(x, 0.0, 1.0);
    if (v <= 0.0031308) { return v * 12.92; }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.v.x || f32(id.y) >= p.v.y) { return; }
    var c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let y = dot(c, vec3<f32>(0.2627, 0.6780, 0.0593));
    let mn = min(c.r, min(c.g, c.b));
    if (mn < 0.0 && y > 0.0) {
        c = vec3<f32>(y) + (c - vec3<f32>(y)) * (y / (y - mn));
    }
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let n = p.v.w;
    let uvw = vec3<f32>(oetf(c.r), oetf(c.g), oetf(c.b)) * ((n - 1.0) / n) + vec3<f32>(0.5 / n);
    let o = clamp(textureSampleLevel(lut, smp, uvw, 0.0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let q = vec3<u32>(round(o * 65535.0));
    textureStore(dst, vec2<i32>(id.xy), vec4<u32>(q, 65535u));
}
