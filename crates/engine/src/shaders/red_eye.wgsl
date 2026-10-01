// Stage 2: red-eye / pet-eye correction on scene-linear Rec.2020, right
// after the scene matrix. Spots arrive already in output pixels (the CPU
// maps them through the geometry), so the kernel needs no transform.
struct Spot {
    a: vec4<f32>, // centre x, centre y, radius (px), darken 0..1
    b: vec4<f32>, // mode (0 red, 1 pet), catchlight, _, _
};
struct P {
    dims: vec4<f32>, // w, h, count, _
    spots: array<Spot, 64>,
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

const LUMA: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    var c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let pix = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let n = u32(p.dims.z);
    for (var i = 0u; i < n; i++) {
        let s = p.spots[i];
        let r = s.a.z;
        let d = distance(pix, s.a.xy);
        if (d > r) { continue; }
        let darken = s.a.w;
        if (s.b.x < 0.5) {
            // Red eye: only clearly red pixels change, so skin and iris
            // inside an oversized circle survive. Linear-light skin reads
            // R/((G+B)/2) up to ~2.5; flash-red pupils are well above 4.
            let e = 1.0 - smoothstep(r * 0.85, r, d);
            let rho = c.r / max((c.g + c.b) * 0.5, 1e-4);
            let w = smoothstep(2.5, 4.0, rho) * e * smoothstep(0.002, 0.01, c.r);
            // Pull every channel down to the weaker of G/B, so a saturated red
            // (whose G and B differ in Rec.2020) ends up neutral, then darken.
            let corrected = vec3<f32>(min(c.g, c.b)) * mix(1.0, 0.15, darken);
            c = mix(c, corrected, w);
        } else {
            // Pet eye: the circle is the pupil; anything brighter than the
            // target goes to a dark neutral that keeps a trace of texture.
            let e = 1.0 - smoothstep(r * 0.7, r, d);
            let t = mix(0.04, 0.005, darken);
            let y = dot(c, LUMA);
            let w = smoothstep(t, 4.0 * t, y) * e;
            let ny = t * pow(max(y, t) / t, 0.1);
            c = mix(c, vec3<f32>(ny), w);
            if (s.b.y > 0.5) {
                // Upper left in the output, whatever the photo's rotation.
                let cc = s.a.xy + vec2<f32>(-0.35, -0.35) * r;
                let k = 1.0 - smoothstep(0.075 * r, 0.15 * r, distance(pix, cc));
                c = mix(c, vec3<f32>(0.51), k * e); // 1.5 EV above middle gray
            }
        }
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(c, 1.0));
}
