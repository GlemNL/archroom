//! `archroom-engine`: the `Op` registry, pipeline graph, stage cache and
//! `wgpu` device (plan §4.1/§6). This is M3 work (plan roadmap); M0's GPU
//! validation happens as a standalone spike instead of through this crate,
//! so the `Op` registry can be designed once, correctly, in M3 rather than
//! rushed for a proof-of-concept.
