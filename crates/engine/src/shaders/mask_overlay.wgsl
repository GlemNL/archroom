// Stage 5 prelude: tints the selected mask red at 50 % (the `O` overlay).
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.dims.x || f32(id.y) >= p.dims.y) { return; }
    let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
    let pix = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let m = mask_weight(u32(p.dims.w), pix);
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(mix(c, vec3<f32>(0.85, 0.08, 0.08), 0.5 * m), 1.0));
}
