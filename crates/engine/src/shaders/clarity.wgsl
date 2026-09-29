// Clarity: boosts the medium-scale detail layer (log-luminance minus its
// edge-aware base), weighted toward the midtones. RGB is scaled by the
// resulting gain so hue and saturation are untouched.
struct P { v: vec4<f32>, }; // amount -1..1, w, h, _
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var base: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

const GRAY: f32 = -2.4739312; // log2(0.18)

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.v.y || f32(id.y) >= p.v.z) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let y = dot(c, vec3<f32>(0.2627, 0.6780, 0.0593));
    if (y <= 1e-6) {
        textureStore(dst, vec2<i32>(id.xy), vec4<f32>(c, 1.0));
        return;
    }
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(p.v.y, p.v.z);
    let b = textureSampleLevel(base, smp, uv, 0.0).r;
    let detail = log2(y) - b;
    let w = smoothstep(GRAY - 4.0, GRAY - 1.0, b) * (1.0 - smoothstep(GRAY + 1.5, GRAY + 3.5, b));
    let gain = clamp(exp2(0.6 * p.v.x * detail * w), 0.25, 4.0);
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(c * gain, 1.0));
}
