// Tone curve (display stage): each channel goes through the sRGB
// transfer, then its 4096-entry LUT (parametric + point curves baked on
// the CPU), then back to linear.
struct P { dims: vec4<f32>, };
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var lut: texture_2d<f32>;   // 4096 x 1, rgb = R,G,B tables
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

fn oetf(x: f32) -> f32 {
    if (x <= 0.0031308) { return x * 12.92; }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}
fn eotf(x: f32) -> f32 {
    if (x <= 0.04045) { return x / 12.92; }
    return pow((x + 0.055) / 1.055, 2.4);
}

fn look(x: f32, ch: u32) -> f32 {
    let t = clamp(x, 0.0, 1.0) * 4095.0;
    let i = i32(floor(t));
    let f = t - f32(i);
    let a = textureLoad(lut, vec2<i32>(i, 0), 0);
    let b = textureLoad(lut, vec2<i32>(min(i + 1, 4095), 0), 0);
    return mix(a[ch], b[ch], f);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    var o = c;
    for (var ch = 0u; ch < 3u; ch++) {
        if (c[ch] > 0.0 && c[ch] <= 1.0) {
            o[ch] = eotf(look(oetf(c[ch]), ch));
        }
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(o, 1.0));
}
