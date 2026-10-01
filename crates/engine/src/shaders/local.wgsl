// Stage 2d: local adjustments. One pass applies every mask's six tone
// sliders in the scene-linear domain, between clarity and the tone map.
// Gains add up in EV and scale RGB (hue-preserving). Highlights/Shadows
// use the same gain curve as highlights_shadows.wgsl on a guided base
// layer; the constants for the rest live in `engine::local::tuning`.
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(4) var base: texture_2d<f32>;
@group(0) @binding(5) var dst: texture_storage_2d<rgba16float, write>;

const GRAY: f32 = -2.4739312; // log2(0.18)
const LUMA: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let pix = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let l = clamp(log2(max(dot(c, LUMA), 1e-6) / 0.18), -8.0, 8.0);
    let b = textureSampleLevel(base, smp, pix / p.dims.xy, 0.0).r;
    let ws = 1.0 - smoothstep(GRAY - 3.0, GRAY + 1.0, b);
    let wh = smoothstep(GRAY - 1.0, GRAY + 3.0, b);
    let w_up = smoothstep(p.tune1.x, p.tune1.y, l);
    let w_dn = 1.0 - smoothstep(p.tune1.z, p.tune1.w, l);
    var dl = 0.0;
    let n = u32(p.dims.z);
    for (var i = 0u; i < n; i++) {
        let m = mask_weight(i, pix);
        if (m <= 0.0) { continue; }
        let it = p.items[i];
        let g = it.t0.x
            + it.t0.y * p.tune0.x * l
            + it.t0.w * 2.0 * ws + it.t0.z * 2.0 * wh
            + it.t1.x * p.tune0.y * w_up
            + it.t1.y * p.tune0.z * w_dn;
        dl += m * g;
    }
    dl = clamp(dl, -8.0, 8.0);
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(c * exp2(dl), 1.0));
}
