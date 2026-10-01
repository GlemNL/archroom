// Shared by the local-adjust pass and the mask overlay: the uniform layout
// and the mask evaluation. Linear masks are analytic in output pixels;
// brush masks sample the source-aligned raster through the geometry
// transform (the same `to_src` as geometry.wgsl).
struct Item {
    t0: vec4<f32>, // exposure (EV), contrast, highlights, shadows (-1..1)
    t1: vec4<f32>, // whites, blacks, kind (0 linear, 1 brush), brush layer
    m0: vec4<f32>, // linear: line point x, y, normal x, y (output px)
    m1: vec4<f32>, // linear: feather (px)
};
struct P {
    dims: vec4<f32>,  // w, h, count, overlay item (-1 = none)
    m: vec4<f32>,     // oriented -> source linear part
    crop: vec4<f32>,  // crop x0, y0, width, height
    rot: vec4<f32>,   // cos, sin, canvas w, canvas h
    tune0: vec4<f32>, // contrast slope, whites EV, blacks EV, _
    tune1: vec4<f32>, // whites range lo, hi; blacks range lo, hi
    items: array<Item, 16>,
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(2) var brush: texture_2d_array<f32>;
@group(0) @binding(3) var smp: sampler;

fn to_oriented(uv: vec2<f32>) -> vec2<f32> {
    let canvas = p.rot.zw;
    let d = (p.crop.xy + uv * p.crop.zw) * canvas - canvas * 0.5;
    let q = vec2<f32>(d.x * p.rot.x + d.y * p.rot.y, -d.x * p.rot.y + d.y * p.rot.x);
    return (q + canvas * 0.5) / canvas;
}

fn to_src(uv: vec2<f32>) -> vec2<f32> {
    let c = to_oriented(uv) - vec2<f32>(0.5);
    return vec2<f32>(p.m.x * c.x + p.m.y * c.y, p.m.z * c.x + p.m.w * c.y) + vec2<f32>(0.5);
}

fn mask_weight(i: u32, pix: vec2<f32>) -> f32 {
    let it = p.items[i];
    if (it.t1.z < 0.5) {
        let d = dot(pix - it.m0.xy, it.m0.zw);
        let t = clamp(d / max(it.m1.x, 1e-3) + 0.5, 0.0, 1.0);
        return t * t * (3.0 - 2.0 * t);
    }
    let uv = pix / p.dims.xy;
    let o = to_oriented(uv);
    if (o.x < 0.0 || o.x > 1.0 || o.y < 0.0 || o.y > 1.0) { return 0.0; }
    return textureSampleLevel(brush, smp, to_src(uv), i32(it.t1.w), 0.0).r;
}
