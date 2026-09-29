#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no usable GPU adapter")]
    NoGpu,
    #[error("image {w}×{h} exceeds the GPU's {max}px texture limit")]
    TooLarge { w: u32, h: u32, max: u32 },
    #[error("camera colour matrix is singular")]
    BadCameraMatrix,
    #[error("display transform: {0}")]
    Icc(#[from] archroom_color::icc::Error),
    #[error("GPU readback failed: {0}")]
    Readback(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
