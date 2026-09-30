pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Catalog(#[from] viberoom_catalog::Error),

    #[error(transparent)]
    Preview(#[from] viberoom_preview::Error),

    #[error(transparent)]
    Core(#[from] viberoom_core::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(path: impl Into<std::path::PathBuf>, source: std::io::Error) -> Self {
        Self::Core(viberoom_core::Error::io(path, source))
    }
}
