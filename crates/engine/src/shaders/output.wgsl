// Stage 5: gamut mapping, the display 3D LUT (built by lcms2) and
// dithering to 8 bits, plus the clipping overlay (`J`).
struct P { v: vec4<f32>, }; // w, h, clip_overlay, lut_n
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var lut: texture_3d<f32>;
@group(0) @binding(3) var smp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba8unorm, write>;

fn oetf(x: f32) -> f32 {
    let v = clamp(x, 0.0, 1.0);
    if (v <= 0.0031308) { return v * 12.92; }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn hash(x: u32, y: u32) -> f32 {
    var h = x * 374761393u + y * 668265263u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    h = h ^ (h >> 16u);
    return f32(h & 0xffffu) / 65535.0;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.v.x || f32(id.y) >= p.v.y) { return; }
    var c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    // Gamut compression (MVP): pull negatives to zero along the luma axis.
    let y = dot(c, vec3<f32>(0.2627, 0.6780, 0.0593));
    let mn = min(c.r, min(c.g, c.b));
    if (mn < 0.0 && y > 0.0) {
        c = vec3<f32>(y) + (c - vec3<f32>(y)) * (y / (y - mn));
    }
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let n = p.v.w;
    let uvw = vec3<f32>(oetf(c.r), oetf(c.g), oetf(c.b)) * ((n - 1.0) / n) + vec3<f32>(0.5 / n);
    var o = textureSampleLevel(lut, smp, uvw, 0.0).rgb;
    // The overlay judges the undithered value.
    let q = round(clamp(o, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0);
    o = clamp(o + vec3<f32>((hash(id.x, id.y) - 0.5) / 255.0), vec3<f32>(0.0), vec3<f32>(1.0));
    if (p.v.z > 0.5) {
        if (max(q.r, max(q.g, q.b)) >= 255.0) { o = vec3<f32>(1.0, 0.0, 0.0); }
        else if (min(q.r, min(q.g, q.b)) <= 0.0) { o = vec3<f32>(0.0, 0.0, 1.0); }
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(o, 1.0));
}
