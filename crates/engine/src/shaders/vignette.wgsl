// Post-crop vignette: a superellipse distance from the frame centre, a
// smooth falloff between Midpoint and Midpoint + Feather, darkening (or
// lightening toward white) by Amount. Highlights protects bright pixels.
struct P {
    a: vec4<f32>,     // amount -1..1, midpoint 0..1, roundness -1..1, feather 0..1
    b: vec4<f32>,     // highlights 0..1, _, _, _
    dims: vec4<f32>,  // w, h
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / p.dims.xy;
    let q = (uv - vec2<f32>(0.5)) * 2.0;             // -1..1, ellipse inscribed in the frame
    let aspect = p.dims.x / p.dims.y;
    let k = p.a.z;
    // Rounder: blend toward a circle whose radius is half the short side.
    let scale = vec2<f32>(max(aspect, 1.0), max(1.0 / aspect, 1.0));
    let kr = max(k, 0.0);
    let qm = mix(q, q * scale, kr);
    var e = 2.0;
    if (k < 0.0) { e = 2.0 + 6.0 * (-k); }           // squarer
    let corner = mix(pow(2.0, 1.0 / e), length(scale), kr);
    let d = pow(pow(abs(qm.x), e) + pow(abs(qm.y), e), 1.0 / e) / corner;
    // d is 1 at the corners: the falloff starts at Midpoint and Feather sets
    // how much of the remaining distance it takes to reach full strength.
    let start = p.a.y;
    let width = max((1.0 - start) * mix(0.1, 1.0, p.a.w), 0.001);
    let s = smoothstep(start, start + width, d);

    let y = dot(c, vec3<f32>(0.2627, 0.6780, 0.0593));
    var o = c;
    if (p.a.x < 0.0) {
        let protect = p.b.x * smoothstep(0.35, 1.0, y);
        o = c * (1.0 + p.a.x * s * (1.0 - protect));
    } else {
        o = c + (vec3<f32>(1.0) - c) * (p.a.x * s);
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(o, 1.0));
}
