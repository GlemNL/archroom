//! `archroom-preview`: thumbnail and preview generation and cache (plan
//! §5.4). Phase A (M1 plan roadmap) covers the `previews.db` index and L1
//! generation; L2/L3 and GPU-rendered previews follow once the Develop
//! engine (M3) exists to render them from.

mod cache;
mod error;
mod generate;

pub use cache::{DEFAULT_PARAMS_HASH, LEVEL_L1, LEVEL_L2, PreviewCache};
pub use error::{Error, Result};
pub use generate::{
    L1_BUDGET_PX, L2_BUDGET_PX, encode_jpeg_from_rgba8, generate_preview_from_display_rgb,
    generate_preview_from_jpeg_bytes, generate_preview_from_linear_rgb, jpeg_quality, l2_budget_px,
    set_preview_options,
};
