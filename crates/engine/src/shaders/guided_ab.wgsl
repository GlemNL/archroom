// Guided filter (self-guided) coefficients from the box means of L and L^2:
// a = var / (var + eps), b = mean * (1 - a).
struct P { v: vec4<f32>, }; // eps
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var means: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = vec2<i32>(textureDimensions(means));
    if (i32(id.x) >= dims.x || i32(id.y) >= dims.y) { return; }
    let m = textureLoad(means, vec2<i32>(id.xy), 0).rg;
    let v = max(m.y - m.x * m.x, 0.0);
    let a = v / (v + p.v.x);
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(a, m.x * (1.0 - a), 0.0, 1.0));
}
