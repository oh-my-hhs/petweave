//! Core error type.

use thiserror::Error;

/// PetWeave core error.
#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid config: {0}")]
    Config(String),

    #[error("wayland error: {0}")]
    Wayland(String),

    #[error("input error: {0}")]
    Input(String),
}
