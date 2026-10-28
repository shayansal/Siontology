//! Networking errors.

use thiserror::Error;

/// Why a network operation failed.
#[derive(Debug, Error)]
pub enum NetError {
    /// A value could not be parsed.
    #[error("parse: {0}")]
    Parse(String),
    /// A frame was malformed or too large.
    #[error("frame: {0}")]
    Frame(#[from] crate::frame::FrameError),
}
