use std::{error::Error, sync::PoisonError};
use thiserror::Error;
use tokio::{io, task::JoinError};

// We can't use thiserror `[from]` on these because they need to be Clone/Eq/PartialEq
// to be used in the build system, so we have a bunch of sad manual impls below.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ApplicationError {
    #[error("build failed")]
    Build,
    #[error("default output not found")]
    DefaultOutputNotFound,
    #[error("Inputs provided to operation are not correct: {0}")]
    WrongInputs(String),
    #[error("Outputs provided to operation are not correct: {0}")]
    WrongOutputs(String),
    #[error("Recipe is not valid: {0}")]
    InvalidRecipe(String),
    #[error("{0}")]
    Other(String),
    #[error("Mutex was poisoned")]
    MutexPoisoned,
    #[error("Font read error: {0}")]
    FontReadError(String),
    #[error("Webfont compression error: {0}")]
    CompressionError(String),
    #[error("Error including a subset font: {0}")]
    IncludeSubsetsError(String),
    #[error("Error removing overlaps: {0}")]
    RemoveOverlapsError(String),
}

impl From<Box<dyn Error>> for ApplicationError {
    fn from(error: Box<dyn Error>) -> Self {
        Self::Other(error.to_string())
    }
}

impl From<io::Error> for ApplicationError {
    fn from(error: io::Error) -> Self {
        Self::Other(error.to_string())
    }
}

impl From<JoinError> for ApplicationError {
    fn from(error: JoinError) -> Self {
        Self::Other(error.to_string())
    }
}

impl<T> From<PoisonError<T>> for ApplicationError {
    fn from(_: PoisonError<T>) -> Self {
        Self::MutexPoisoned
    }
}

impl From<skrifa::raw::ReadError> for ApplicationError {
    fn from(error: skrifa::raw::ReadError) -> Self {
        Self::FontReadError(error.to_string())
    }
}

impl From<ttf2woff2::Error> for ApplicationError {
    fn from(error: ttf2woff2::Error) -> Self {
        Self::CompressionError(error.to_string())
    }
}
