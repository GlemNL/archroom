// Guided filter output: base = mean(a) * L + mean(b).
@group(0) @binding(0) var ab: texture_2d<f32>;
@group(0) @binding(1) var luma: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = vec2<i32>(textureDimensions(luma));
    if (i32(id.x) >= dims.x || i32(id.y) >= dims.y) { return; }
    let c = textureLoad(ab, vec2<i32>(id.xy), 0).rg;
    let l = textureLoad(luma, vec2<i32>(id.xy), 0).r;
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(c.x * l + c.y, 0.0, 0.0, 1.0));
}
