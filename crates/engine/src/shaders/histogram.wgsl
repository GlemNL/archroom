// GPU histogram of the 8-bit output: 256 bins each of R, G, B and luma.
struct P { v: vec4<f32>, }; // w, h
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> bins: array<atomic<u32>, 1024>;

var<workgroup> local: array<atomic<u32>, 1024>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    for (var i = li; i < 1024u; i += 64u) { atomicStore(&local[i], 0u); }
    workgroupBarrier();
    if (f32(id.x) < p.v.x && f32(id.y) < p.v.y) {
        let c = textureLoad(src, vec2<i32>(id.xy), 0).rgb;
        let q = vec3<u32>(round(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0));
        let l = u32(round(clamp(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0) * 255.0));
        atomicAdd(&local[q.x], 1u);
        atomicAdd(&local[256u + q.y], 1u);
        atomicAdd(&local[512u + q.z], 1u);
        atomicAdd(&local[768u + l], 1u);
    }
    workgroupBarrier();
    for (var i = li; i < 1024u; i += 64u) {
        let v = atomicLoad(&local[i]);
        if (v > 0u) { atomicAdd(&bins[i], v); }
    }
}
