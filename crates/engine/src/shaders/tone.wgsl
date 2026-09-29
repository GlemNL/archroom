// Stage 3: the profile's base curve (a Hable filmic on luminance, applied
// as a scale so hue is preserved) plus Contrast, Whites and Blacks.
// `tone_curve` in tone.rs is the CPU reference of exactly this math.
struct P {
    abcd: vec4<f32>,  // A B C D
    efwb: vec4<f32>,  // E F white_point bias
    cbm: vec4<f32>,   // contrast_exp black_lift mode(0 hable, 1 clamp) _
    dims: vec4<f32>,  // w h
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

fn hable(x: f32) -> f32 {
    let a = p.abcd.x; let b = p.abcd.y; let c = p.abcd.z; let d = p.abcd.w;
    let e = p.efwb.x; let f = p.efwb.y;
    return ((x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f)) - e / f;
}

fn curve(y: f32) -> f32 {
    var o = min(y, 1.0);
    if (p.cbm.z < 0.5) {
        o = hable(p.efwb.w * y) / hable(p.efwb.z);
    }
    o = clamp(o, 0.0, 1.0);
    o = 0.18 * pow(o / 0.18, p.cbm.x);
    o = clamp(o, 0.0, 1.0);
    let t = p.cbm.y;
    let k = 1.0 - o;
    o = clamp(o + t * k * k, 0.0, 1.0);
    return o;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let c = max(textureLoad(src, vec2<i32>(id.xy), 0).rgb, vec3<f32>(0.0));
    let y = dot(c, vec3<f32>(0.2627, 0.6780, 0.0593));
    var o = vec3<f32>(0.0);
    if (y > 1e-6) {
        let yo = curve(y);
        o = c * (yo / y);
        // Path to white: pull an over-range colour back to 1 keeping its luma.
        let m = max(o.r, max(o.g, o.b));
        if (m > 1.0) {
            o = vec3<f32>(yo) + (o - vec3<f32>(yo)) * ((1.0 - yo) / (m - yo));
        }
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(o, 1.0));
}
