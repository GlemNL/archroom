// Stage 4: Vibrance, Saturation and the B&W treatment in OkLab.
struct P {
    r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>,     // Rec.2020 -> LMS
    i0: vec4<f32>, i1: vec4<f32>, i2: vec4<f32>,     // LMS -> Rec.2020
    v: vec4<f32>,                                    // vibrance, saturation, bw, _
    dims: vec4<f32>,
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

fn cbrt(x: f32) -> f32 { return sign(x) * pow(abs(x), 1.0 / 3.0); }

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let lms = vec3<f32>(dot(p.r0.xyz, c), dot(p.r1.xyz, c), dot(p.r2.xyz, c));
    let q = vec3<f32>(cbrt(lms.x), cbrt(lms.y), cbrt(lms.z));
    let l = 0.2104542553 * q.x + 0.7936177850 * q.y - 0.0040720468 * q.z;
    var a = 1.9779984951 * q.x - 2.4285922050 * q.y + 0.4505937099 * q.z;
    var b = 0.0259040371 * q.x + 0.7827717662 * q.y - 0.8086757660 * q.z;

    let chroma = length(vec2<f32>(a, b));
    var k = 1.0 + p.v.y;                       // saturation
    let hue = atan2(b, a);
    let dh = (hue - 0.9) / 0.45;
    let skin = 1.0 - 0.5 * exp(-dh * dh);      // vibrance eases off on skin tones
    k = k * (1.0 + p.v.x * clamp(1.0 - chroma / 0.32, 0.0, 1.0) * skin);
    if (p.v.z > 0.5) { k = 0.0; }
    a = a * max(k, 0.0);
    b = b * max(k, 0.0);

    let l2 = l + 0.3963377774 * a + 0.2158037573 * b;
    let m2 = l - 0.1055613458 * a - 0.0638541728 * b;
    let s2 = l - 0.0894841775 * a - 1.2914855480 * b;
    let lin = vec3<f32>(l2 * l2 * l2, m2 * m2 * m2, s2 * s2 * s2);
    let o = vec3<f32>(dot(p.i0.xyz, lin), dot(p.i1.xyz, lin), dot(p.i2.xyz, lin));
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(o, 1.0));
}
