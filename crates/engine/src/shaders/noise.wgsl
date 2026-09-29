// Noise reduction on scene-linear data, in a square-root (roughly
// variance-stabilised) domain. Luminance: a 7x7 bilateral filter. Colour:
// chroma smoothing over 13x13, guided by luma differences. Radii scale with
// the proxy's zoom, so this is exact only at 1:1 (plan §6.7).
struct P {
    a: vec4<f32>,     // luma strength, luma detail, luma contrast (0..1), zoom
    b: vec4<f32>,     // colour strength, colour detail, smoothness (0..1), _
    dims: vec4<f32>,  // w, h
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

const W: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

fn fetch(q: vec2<i32>, dims: vec2<i32>) -> vec3<f32> {
    let qq = clamp(q, vec2<i32>(0), dims - vec2<i32>(1));
    return sqrt(max(textureLoad(src, qq, 0).rgb, vec3<f32>(0.0)));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let dims = vec2<i32>(i32(p.dims.x), i32(p.dims.y));
    let xy = vec2<i32>(id.xy);
    let centre = fetch(xy, dims);
    let lc = dot(centre, W);
    let cc = centre - vec3<f32>(lc);

    let sig_sl = (1.0 + 2.0 * p.a.x) * p.a.w;
    let sig_rl = (0.01 + 0.12 * p.a.x * (1.0 - 0.7 * p.a.y)) * (1.0 - 0.6 * p.a.z);
    let sig_sc = (2.0 + 4.0 * p.b.z) * p.a.w;
    let sig_rc = mix(0.15, 0.03, p.b.y);

    var lsum = 0.0; var lw = 0.0;
    var csum = vec3<f32>(0.0); var cw = 0.0;
    for (var dy = -6; dy <= 6; dy++) {
        for (var dx = -6; dx <= 6; dx++) {
            let s = fetch(xy + vec2<i32>(dx, dy), dims);
            let l = dot(s, W);
            let d2 = f32(dx * dx + dy * dy);
            let dl = l - lc;
            if (p.b.x > 0.0) {
                let wc = exp(-0.5 * d2 / (sig_sc * sig_sc)) * exp(-0.5 * dl * dl / (sig_rc * sig_rc));
                csum += (s - vec3<f32>(l)) * wc;
                cw += wc;
            }
            if (p.a.x > 0.0 && abs(dx) <= 3 && abs(dy) <= 3) {
                let wl = exp(-0.5 * d2 / (sig_sl * sig_sl)) * exp(-0.5 * dl * dl / (sig_rl * sig_rl));
                lsum += l * wl;
                lw += wl;
            }
        }
    }
    var l_out = lc;
    if (p.a.x > 0.0 && lw > 0.0) { l_out = mix(lc, lsum / lw, min(p.a.x * 1.5, 1.0)); }
    var c_out = cc;
    if (p.b.x > 0.0 && cw > 0.0) { c_out = mix(cc, csum / cw, p.b.x); }
    let v = max(vec3<f32>(l_out) + c_out, vec3<f32>(0.0));
    textureStore(dst, xy, vec4<f32>(v * v, 1.0));
}
