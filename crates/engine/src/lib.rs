//! `archroom-engine`: the `Op` registry, pipeline graph, stage cache and
//! `wgpu` device (plan §4.1/§6). The real registry and stage pipeline are
//! M3 work (plan roadmap); `spike` is the M0 GPU-architecture validation
//! (decode → wgpu → WGSL → egui), kept deliberately separate so the real
//! `Op` registry gets designed once, correctly, in M3 rather than grown out
//! of a proof-of-concept.

pub mod spike;
