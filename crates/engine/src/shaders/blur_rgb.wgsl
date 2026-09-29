// Separable Gaussian blur of all three channels (clamped edges).
// v = dir_x, dir_y, sigma (texels), _.
struct P { v: vec4<f32>, };
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = vec2<i32>(textureDimensions(src));
    if (i32(id.x) >= dims.x || i32(id.y) >= dims.y) { return; }
    let sigma = max(p.v.z, 0.05);
    let r = min(i32(ceil(sigma * 3.0)), 24);
    let dir = vec2<i32>(i32(p.v.x), i32(p.v.y));
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var k = -r; k <= r; k++) {
        let w = exp(-0.5 * f32(k * k) / (sigma * sigma));
        let q = clamp(vec2<i32>(id.xy) + dir * k, vec2<i32>(0), dims - vec2<i32>(1));
        acc += textureLoad(src, q, 0).rgb * w;
        wsum += w;
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(acc / wsum, 1.0));
}
