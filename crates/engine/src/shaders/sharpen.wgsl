// Unsharp mask on luminance, in the sRGB-encoded domain: the detail layer
// (image minus its Gaussian blur) is soft-limited by Detail (halo
// suppression), restricted to edges by Masking, scaled by Amount and put
// back as a luminance gain so hue is untouched.
struct P {
    v: vec4<f32>,     // amount (1.0 = 100%), detail 0..1, masking 0..1, _
    dims: vec4<f32>,  // w, h
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var blurred: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

fn oetf(x: f32) -> f32 {
    let v = max(x, 0.0);
    if (v <= 0.0031308) { return v * 12.92; }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}
fn eotf(x: f32) -> f32 {
    if (x <= 0.04045) { return x / 12.92; }
    return pow((x + 0.055) / 1.055, 2.4);
}
fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2627, 0.6780, 0.0593)); }

// Encoded luminance of the *blurred* image, so the edge mask is as wide as
// the sharpening radius and covers the overshoot pixels beside an edge.
fn le(q: vec2<i32>, dims: vec2<i32>) -> f32 {
    let qq = clamp(q, vec2<i32>(0), dims - vec2<i32>(1));
    return oetf(luma(textureLoad(blurred, qq, 0).rgb));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let dims = vec2<i32>(i32(p.dims.x), i32(p.dims.y));
    let xy = vec2<i32>(id.xy);
    let c = textureLoad(src, xy, 0).rgb;
    let y = luma(c);
    if (y <= 1e-5) {
        textureStore(dst, xy, vec4<f32>(c, 1.0));
        return;
    }
    let ye = oetf(y);
    var d = ye - oetf(luma(textureLoad(blurred, xy, 0).rgb));
    let lim = mix(0.03, 1.0, p.v.y);
    d = lim * tanh(d / lim);

    let gx = le(xy + vec2<i32>(2, 0), dims) - le(xy - vec2<i32>(2, 0), dims);
    let gy = le(xy + vec2<i32>(0, 2), dims) - le(xy - vec2<i32>(0, 2), dims);
    let edge = smoothstep(0.01, 0.08, length(vec2<f32>(gx, gy)));
    let m = mix(1.0, edge, p.v.z);

    let y2 = eotf(clamp(ye + p.v.x * d * m, 0.0, 1.5));
    let gain = clamp(y2 / y, 0.0, 4.0);
    textureStore(dst, xy, vec4<f32>(c * gain, 1.0));
}
