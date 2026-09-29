// Highlights/Shadows step 1: whole-frame log-luminance at ~1024 px.
// r = mean log2(Y), g = mean log2(Y)^2 (for the guided filter's variance).
struct P { dims: vec4<f32>, }; // src_w, src_h, dst_w, dst_h
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

fn logy(x: i32, y: i32) -> f32 {
    let sw = i32(p.dims.x);
    let sh = i32(p.dims.y);
    let c = textureLoad(src, vec2<i32>(clamp(x, 0, sw - 1), clamp(y, 0, sh - 1)), 0).rgb;
    let yv = dot(c, vec3<f32>(0.2627, 0.6780, 0.0593));
    return log2(max(yv, 1e-5));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.z || f32(id.y) >= p.dims.w) { return; }
    let scale = vec2<f32>(p.dims.x / p.dims.z, p.dims.y / p.dims.w);
    let lo = vec2<f32>(f32(id.x), f32(id.y)) * scale;
    let hi = lo + scale;
    let x0 = i32(floor(lo.x));
    let x1 = min(i32(ceil(hi.x)), x0 + 32);
    let y0 = i32(floor(lo.y));
    let y1 = min(i32(ceil(hi.y)), y0 + 32);
    var s = 0.0;
    var s2 = 0.0;
    var n = 0.0;
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            let l = logy(x, y);
            s += l;
            s2 += l * l;
            n += 1.0;
        }
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(s / n, s2 / n, 0.0, 1.0));
}
