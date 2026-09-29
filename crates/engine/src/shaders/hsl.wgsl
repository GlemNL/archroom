// HSL / Color Mixer and the B&W mixer in OkLCh. Eight hue bands with
// raised-cosine overlap (weights sum to 1), faded out at low chroma. In
// colour mode `hue`, `sat` and `lum` apply; in B&W mode only `lum` is used
// (it carries the B&W mix) and the display pass then drops the chroma.
struct P {
    r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>,   // Rec.2020 -> LMS
    i0: vec4<f32>, i1: vec4<f32>, i2: vec4<f32>,   // LMS -> Rec.2020
    hue: array<vec4<f32>, 2>,                      // -1..1 (x 30 degrees)
    sat: array<vec4<f32>, 2>,                      // -1..1
    lum: array<vec4<f32>, 2>,                      // -1..1
    m: vec4<f32>,                                  // w, h, _, _
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

const TAU: f32 = 6.283185307;
const PI: f32 = 3.141592654;

fn cbrt(x: f32) -> f32 { return sign(x) * pow(abs(x), 1.0 / 3.0); }

fn pick(a: array<vec4<f32>, 2>, i: i32) -> f32 {
    if (i < 4) { return a[0][i]; }
    return a[1][i - 4];
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.m.x || f32(id.y) >= p.m.y) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let lms = vec3<f32>(dot(p.r0.xyz, c), dot(p.r1.xyz, c), dot(p.r2.xyz, c));
    let q = vec3<f32>(cbrt(lms.x), cbrt(lms.y), cbrt(lms.z));
    var l = 0.2104542553 * q.x + 0.7936177850 * q.y - 0.0040720468 * q.z;
    let a = 1.9779984951 * q.x - 2.4285922050 * q.y + 0.4505937099 * q.z;
    let b = 0.0259040371 * q.x + 0.7827717662 * q.y - 0.8086757660 * q.z;
    var chroma = length(vec2<f32>(a, b));
    var hue = atan2(b, a);
    if (hue < 0.0) { hue = hue + TAU; }

    var centers = array<f32, 8>(0.5061, 0.9599, 1.9199, 2.4784, 3.4034, 4.6077, 5.2883, 5.7247);
    // Find the pair of band centres around `hue` (wrapping past Magenta).
    var lo = 7; var hi = 0;
    var c_lo = centers[7] - TAU; var c_hi = centers[0];
    for (var k = 0; k < 7; k++) {
        if (hue >= centers[k] && hue < centers[k + 1]) {
            lo = k; hi = k + 1; c_lo = centers[k]; c_hi = centers[k + 1];
        }
    }
    if (hue >= centers[7]) { lo = 7; hi = 0; c_lo = centers[7]; c_hi = centers[0] + TAU; }
    let t = (hue - c_lo) / (c_hi - c_lo);
    let w_lo = pow(cos(t * PI * 0.5), 2.0);
    let w_hi = 1.0 - w_lo;
    let fade = smoothstep(0.0, 0.04, chroma);

    let dh = (w_lo * pick(p.hue, lo) + w_hi * pick(p.hue, hi)) * fade;
    let ds = (w_lo * pick(p.sat, lo) + w_hi * pick(p.sat, hi)) * fade;
    let dl = (w_lo * pick(p.lum, lo) + w_hi * pick(p.lum, hi)) * fade;

    hue = hue + dh * 0.5236;
    chroma = chroma * max(1.0 + ds, 0.0);
    l = l + dl * 0.25;
    let a2 = chroma * cos(hue);
    let b2 = chroma * sin(hue);

    let l2 = l + 0.3963377774 * a2 + 0.2158037573 * b2;
    let m2 = l - 0.1055613458 * a2 - 0.0638541728 * b2;
    let s2 = l - 0.0894841775 * a2 - 1.2914855480 * b2;
    let lin = vec3<f32>(l2 * l2 * l2, m2 * m2 * m2, s2 * s2 * s2);
    let o = vec3<f32>(dot(p.i0.xyz, lin), dot(p.i1.xyz, lin), dot(p.i2.xyz, lin));
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(o, 1.0));
}
