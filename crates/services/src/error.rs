pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Catalog(#[from] archroom_catalog::Error),

    #[error(transparent)]
    Preview(#[from] archroom_preview::Error),

    #[error(transparent)]
    Core(#[from] archroom_core::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(path: impl Into<std::path::PathBuf>, source: std::io::Error) -> Self {
        Self::Core(archroom_core::Error::io(path, source))
    }
}
