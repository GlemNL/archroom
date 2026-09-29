// Stage 2a: decoded camera RGB -> linear Rec.2020, white balance (a CAT
// folded into the matrix) and exposure (folded in as a gain).
struct P {
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    dims: vec4<f32>, // w, h
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let o = vec3<f32>(dot(p.r0.xyz, c), dot(p.r1.xyz, c), dot(p.r2.xyz, c));
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(o, 1.0));
}
