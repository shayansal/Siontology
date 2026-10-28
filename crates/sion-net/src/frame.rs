//! Frames: one class byte, a four-byte big-endian length, then the body.
//!
//! A reader parses the five-byte header first and refuses a length above
//! the class's limit before it reads (or allocates for) a single byte of the
//! body.

use thiserror::Error;

use crate::class::{ClassLimits, MessageClass};

/// Bytes in a frame header.
pub const FRAME_HEADER_BYTES: usize = 5;

/// Why a frame was refused.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum FrameError {
    /// The class byte names no class.
    #[error("unknown message class {0}")]
    UnknownClass(u8),
    /// The body is longer than its class allows.
    #[error("{class:?} frame of {len} bytes exceeds its limit of {limit}")]
    TooLarge {
        /// The class.
        class: MessageClass,
        /// The announced length.
        len: usize,
        /// The class's limit.
        limit: usize,
    },
    /// The frame arrived on a stream of another class.
    #[error("{got:?} frame on a {expected:?} stream")]
    WrongStream {
        /// The stream's class.
        expected: MessageClass,
        /// The frame's class.
        got: MessageClass,
    },
    /// The bytes end inside a frame.
    #[error("truncated frame: {needed} more bytes needed")]
    Truncated {
        /// How many more bytes the frame needs.
        needed: usize,
    },
}

/// A parsed frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// The body's class.
    pub class: MessageClass,
    /// The body's length in bytes.
    pub len: usize,
}

impl FrameHeader {
    /// The header's five bytes.
    pub fn encode(&self) -> [u8; FRAME_HEADER_BYTES] {
        let len = u32::try_from(self.len).unwrap_or(u32::MAX).to_be_bytes();
        [self.class.tag(), len[0], len[1], len[2], len[3]]
    }

    /// Parses a header and checks its length against `limits`.
    pub fn parse(
        bytes: [u8; FRAME_HEADER_BYTES],
        limits: &ClassLimits,
    ) -> Result<FrameHeader, FrameError> {
        let class = MessageClass::from_tag(bytes[0]).ok_or(FrameError::UnknownClass(bytes[0]))?;
        let len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
        let limit = limits.get(class);
        if len > limit {
            return Err(FrameError::TooLarge { class, len, limit });
        }
        Ok(FrameHeader { class, len })
    }
}

/// Frames `body` as `class`, refusing a body over the class's limit (the
/// receiver would refuse it anyway).
pub fn frame(
    class: MessageClass,
    body: &[u8],
    limits: &ClassLimits,
) -> Result<Vec<u8>, FrameError> {
    let limit = limits.get(class);
    if body.len() > limit {
        return Err(FrameError::TooLarge {
            class,
            len: body.len(),
            limit,
        });
    }
    let mut out = Vec::with_capacity(FRAME_HEADER_BYTES + body.len());
    out.extend_from_slice(
        &FrameHeader {
            class,
            len: body.len(),
        }
        .encode(),
    );
    out.extend_from_slice(body);
    Ok(out)
}

/// Splits a byte stream into frames, checking each header before taking its
/// body. The streaming reader in the endpoint applies the same checks.
pub fn split_frames<'a>(
    mut bytes: &'a [u8],
    limits: &ClassLimits,
) -> Result<Vec<(MessageClass, &'a [u8])>, FrameError> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < FRAME_HEADER_BYTES {
            return Err(FrameError::Truncated {
                needed: FRAME_HEADER_BYTES - bytes.len(),
            });
        }
        let mut head = [0u8; FRAME_HEADER_BYTES];
        head.copy_from_slice(&bytes[..FRAME_HEADER_BYTES]);
        let header = FrameHeader::parse(head, limits)?;
        let rest = &bytes[FRAME_HEADER_BYTES..];
        if rest.len() < header.len {
            return Err(FrameError::Truncated {
                needed: header.len - rest.len(),
            });
        }
        out.push((header.class, &rest[..header.len]));
        bytes = &rest[header.len..];
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn small() -> ClassLimits {
        ClassLimits {
            control: 64,
            consensus: 256,
            ingress: 128,
            bulk: 1024,
        }
    }

    proptest! {
        #[test]
        fn frames_round_trip_within_their_limits(
            bodies in prop::collection::vec((0u8..4, prop::collection::vec(any::<u8>(), 0..300)), 0..12)
        ) {
            let limits = small();
            let mut stream = Vec::new();
            let mut expected = Vec::new();
            for (tag, body) in &bodies {
                let class = MessageClass::from_tag(*tag).unwrap();
                match frame(class, body, &limits) {
                    Ok(bytes) => {
                        stream.extend_from_slice(&bytes);
                        expected.push((class, body.clone()));
                    }
                    Err(FrameError::TooLarge { len, limit, .. }) => {
                        prop_assert!(len > limit);
                        prop_assert_eq!(len, body.len());
                    }
                    Err(other) => prop_assert!(false, "unexpected {}", other),
                }
            }
            let split = split_frames(&stream, &limits).unwrap();
            let got: Vec<(MessageClass, Vec<u8>)> =
                split.into_iter().map(|(c, b)| (c, b.to_vec())).collect();
            prop_assert_eq!(got, expected);
        }

        #[test]
        fn arbitrary_bytes_never_panic_and_never_yield_an_oversized_body(
            bytes in prop::collection::vec(any::<u8>(), 0..600)
        ) {
            let limits = small();
            if let Ok(frames) = split_frames(&bytes, &limits) {
                for (class, body) in frames {
                    prop_assert!(body.len() <= limits.get(class));
                }
            }
        }
    }

    #[test]
    fn a_header_over_its_limit_is_refused_before_the_body() {
        let limits = small();
        let header = FrameHeader {
            class: MessageClass::Control,
            len: 65,
        }
        .encode();
        assert_eq!(
            FrameHeader::parse(header, &limits),
            Err(FrameError::TooLarge {
                class: MessageClass::Control,
                len: 65,
                limit: 64
            })
        );
        // Only the header is present: the length alone decides.
        assert!(matches!(
            split_frames(&header, &limits),
            Err(FrameError::TooLarge { .. })
        ));
        assert_eq!(
            FrameHeader::parse([9, 0, 0, 0, 0], &limits),
            Err(FrameError::UnknownClass(9))
        );
        let ok = frame(MessageClass::Bulk, &[7; 3], &limits).unwrap();
        assert_eq!(
            split_frames(&ok[..6], &limits),
            Err(FrameError::Truncated { needed: 2 })
        );
    }
}
