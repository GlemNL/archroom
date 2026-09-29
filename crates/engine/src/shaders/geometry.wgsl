// Stage 1 (plan §6.2): orientation + resample to the proxy. Each output
// pixel averages the source footprint it covers (area filter), or samples
// bilinearly when it covers less than a source pixel.
struct P {
    dims: vec4<f32>, // src_w, src_h, dst_w, dst_h
    m: vec4<f32>,    // oriented(uv) -> source(uv) linear part (centred): m00 m01 m10 m11
    crop: vec4<f32>, // crop x0, y0, width, height (normalised to the canvas)
    rot: vec4<f32>,  // cos(angle), sin(angle), canvas_w, canvas_h (pixels)
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

// Output uv -> canvas -> un-straightened (oriented) uv.
fn to_oriented(uv: vec2<f32>) -> vec2<f32> {
    let canvas = p.rot.zw;
    let d = (p.crop.xy + uv * p.crop.zw) * canvas - canvas * 0.5;
    // Rotate by -angle (clockwise angle in y-down coordinates).
    let q = vec2<f32>(d.x * p.rot.x + d.y * p.rot.y, -d.x * p.rot.y + d.y * p.rot.x);
    return (q + canvas * 0.5) / canvas;
}

fn to_src(uv: vec2<f32>) -> vec2<f32> {
    let c = to_oriented(uv) - vec2<f32>(0.5);
    return vec2<f32>(p.m.x * c.x + p.m.y * c.y, p.m.z * c.x + p.m.w * c.y) + vec2<f32>(0.5);
}

fn load(x: i32, y: i32) -> vec4<f32> {
    let sw = i32(p.dims.x);
    let sh = i32(p.dims.y);
    return textureLoad(src, vec2<i32>(clamp(x, 0, sw - 1), clamp(y, 0, sh - 1)), 0);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dw = p.dims.z;
    let dh = p.dims.w;
    if (f32(id.x) >= dw || f32(id.y) >= dh) { return; }
    let ssize = vec2<f32>(p.dims.x, p.dims.y);
    let dsize = vec2<f32>(dw, dh);
    let o = vec2<f32>(f32(id.x), f32(id.y));
    // Bounding box of the pixel's footprint in the source (exact when the
    // straighten angle is 0; a slight over-blur when rotated).
    let a = to_src(o / dsize) * ssize;
    let b = to_src((o + vec2<f32>(1.0, 0.0)) / dsize) * ssize;
    let c2 = to_src((o + vec2<f32>(0.0, 1.0)) / dsize) * ssize;
    let d2 = to_src((o + vec2<f32>(1.0, 1.0)) / dsize) * ssize;
    let lo = min(min(a, b), min(c2, d2));
    let hi = max(max(a, b), max(c2, d2));

    // Outside the rotated image (only when the crop reaches past it).
    let centre = to_oriented((o + vec2<f32>(0.5)) / dsize);
    if (centre.x < 0.0 || centre.x > 1.0 || centre.y < 0.0 || centre.y > 1.0) {
        textureStore(dst, vec2<i32>(id.xy), vec4<f32>(0.0, 0.0, 0.0, 1.0));
        return;
    }

    var acc = vec3<f32>(0.0);
    if (hi.x - lo.x < 1.0 && hi.y - lo.y < 1.0) {
        let c = (lo + hi) * 0.5 - vec2<f32>(0.5);
        let f = fract(c);
        let i = vec2<i32>(floor(c));
        let top = mix(load(i.x, i.y).rgb, load(i.x + 1, i.y).rgb, f.x);
        let bot = mix(load(i.x, i.y + 1).rgb, load(i.x + 1, i.y + 1).rgb, f.x);
        acc = mix(top, bot, f.y);
    } else {
        var wsum = 0.0;
        let x0 = i32(floor(lo.x));
        let x1 = min(i32(ceil(hi.x)), x0 + 64);
        let y0 = i32(floor(lo.y));
        let y1 = min(i32(ceil(hi.y)), y0 + 64);
        for (var y = y0; y < y1; y++) {
            let wy = min(f32(y + 1), hi.y) - max(f32(y), lo.y);
            for (var x = x0; x < x1; x++) {
                let wx = min(f32(x + 1), hi.x) - max(f32(x), lo.x);
                let w = max(wx, 0.0) * max(wy, 0.0);
                acc += load(x, y).rgb * w;
                wsum += w;
            }
        }
        acc = acc / max(wsum, 1e-6);
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(acc, 1.0));
}
