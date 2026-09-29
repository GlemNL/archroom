// Stage 2b: local highlights/shadows. The base layer (edge-aware smooth
// log-luminance) picks a per-pixel gain in stops; scaling RGB by 2^gain
// leaves the detail layer and the chromaticity untouched.
struct P { v: vec4<f32>, }; // shadows, highlights (-1..1), w, h
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var base: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

const GRAY: f32 = -2.4739312; // log2(0.18)

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.v.z || f32(id.y) >= p.v.w) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(p.v.z, p.v.w);
    let b = textureSampleLevel(base, smp, uv, 0.0).r;
    let ws = 1.0 - smoothstep(GRAY - 3.0, GRAY + 1.0, b);
    let wh = smoothstep(GRAY - 1.0, GRAY + 3.0, b);
    let gain = p.v.x * 2.0 * ws + p.v.y * 2.0 * wh;
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(c * exp2(gain), 1.0));
}
