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
    /// A network key was malformed.
    #[error("network key: {0}")]
    Key(String),
    /// A network record was malformed or did not verify.
    #[error("network record: {0}")]
    Record(String),
    /// The TLS configuration could not be built.
    #[error("tls: {0}")]
    Tls(String),
    /// A connection could not be established or its hello was refused.
    #[error("handshake: {0}")]
    Handshake(String),
    /// I/O.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
