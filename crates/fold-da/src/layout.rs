//! Mapping a Pulse blob onto a `k × k` square of field symbols.
//!
//! Layout (row-major):
//!
//! - symbol 0 holds the blob length in bytes;
//! - symbols `1..` hold the blob, seven bytes per symbol, big-endian, the
//!   last one zero-padded;
//! - every remaining symbol is zero.
//!
//! Seven-byte symbols are below `2^56 < p`, so every byte string maps to field
//! elements without reduction, and decoding is exact. The layout is
//! canonical: decoding rejects non-zero padding and out-of-range symbols, so
//! one blob has exactly one square.

use crate::error::DaError;
use crate::field::Fp;

/// Bytes carried by one data symbol.
pub const SYMBOL_BYTES: usize = 7;
/// Smallest supported data-square side.
pub const MIN_K: usize = 2;
/// Largest supported data-square side (about 29 MB of blob data).
pub const MAX_K: usize = 2048;

const SYMBOL_LIMIT: u64 = 1 << 56;

/// Symbols needed for a blob of `len` bytes, including the length symbol.
pub fn symbols_needed(len: usize) -> usize {
    1 + len.div_ceil(SYMBOL_BYTES)
}

/// Checks that `k` is a supported square side.
pub fn validate_k(k: usize) -> Result<(), DaError> {
    if k.is_power_of_two() && (MIN_K..=MAX_K).contains(&k) {
        Ok(())
    } else {
        Err(DaError::InvalidK(k))
    }
}

/// The smallest supported `k` whose square holds a blob of `len` bytes.
pub fn choose_k(len: usize) -> Result<usize, DaError> {
    let needed = symbols_needed(len);
    let mut k = MIN_K;
    while k * k < needed {
        k *= 2;
        if k > MAX_K {
            return Err(DaError::BlobTooLarge {
                len,
                needed,
                k: MAX_K,
                capacity: MAX_K * MAX_K,
            });
        }
    }
    Ok(k)
}

fn symbol_from_chunk(chunk: &[u8]) -> Fp {
    debug_assert!(chunk.len() <= SYMBOL_BYTES);
    let mut bytes = [0u8; 8];
    bytes[1..1 + chunk.len()].copy_from_slice(chunk);
    Fp::new(u64::from_be_bytes(bytes))
}

fn symbol_bytes(symbol: Fp, index: usize) -> Result<[u8; SYMBOL_BYTES], DaError> {
    let value = symbol.value();
    if value >= SYMBOL_LIMIT {
        return Err(DaError::NonCanonicalSymbol { index });
    }
    let bytes = value.to_be_bytes();
    let mut out = [0u8; SYMBOL_BYTES];
    out.copy_from_slice(&bytes[1..]);
    Ok(out)
}

/// Lays a blob out as `k²` symbols (row-major).
pub fn blob_to_square(blob: &[u8], k: usize) -> Result<Vec<Fp>, DaError> {
    validate_k(k)?;
    let needed = symbols_needed(blob.len());
    let capacity = k * k;
    if needed > capacity {
        return Err(DaError::BlobTooLarge {
            len: blob.len(),
            needed,
            k,
            capacity,
        });
    }
    let mut symbols = vec![Fp::ZERO; capacity];
    symbols[0] = Fp::new(blob.len() as u64);
    for (i, chunk) in blob.chunks(SYMBOL_BYTES).enumerate() {
        symbols[1 + i] = symbol_from_chunk(chunk);
    }
    Ok(symbols)
}

/// Recovers the blob from `k²` data symbols (row-major), rejecting any
/// non-canonical layout.
pub fn square_to_blob(symbols: &[Fp]) -> Result<Vec<u8>, DaError> {
    let Some(first) = symbols.first() else {
        return Err(DaError::InvalidPadding("empty square"));
    };
    let length = first.value();
    if length >= SYMBOL_LIMIT {
        return Err(DaError::InvalidPadding("length symbol out of range"));
    }
    let length =
        usize::try_from(length).map_err(|_| DaError::InvalidPadding("length too large"))?;
    let used = symbols_needed(length);
    if used > symbols.len() {
        return Err(DaError::InvalidPadding("length exceeds square capacity"));
    }
    let mut blob = Vec::with_capacity(length + SYMBOL_BYTES);
    for (offset, symbol) in symbols[1..used].iter().enumerate() {
        blob.extend_from_slice(&symbol_bytes(*symbol, offset + 1)?);
    }
    if blob[length..].iter().any(|byte| *byte != 0) {
        return Err(DaError::InvalidPadding("non-zero bytes after the blob"));
    }
    blob.truncate(length);
    if symbols[used..].iter().any(|symbol| !symbol.is_zero()) {
        return Err(DaError::InvalidPadding("non-zero padding symbol"));
    }
    Ok(blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_k_is_minimal() {
        assert_eq!(choose_k(0).unwrap(), 2);
        assert_eq!(choose_k(21).unwrap(), 2); // 1 + 3 = 4 symbols
        assert_eq!(choose_k(22).unwrap(), 4); // 1 + 4 = 5 symbols
        for len in [0usize, 1, 7, 100, 1000, 100_000] {
            let k = choose_k(len).unwrap();
            assert!(k * k >= symbols_needed(len));
            if k > MIN_K {
                assert!((k / 2) * (k / 2) < symbols_needed(len));
            }
        }
    }

    #[test]
    fn round_trip_many_sizes() {
        for len in 0..200usize {
            let blob: Vec<u8> = (0..len).map(|i| (i * 31 + 7) as u8).collect();
            let k = choose_k(len).unwrap();
            let symbols = blob_to_square(&blob, k).unwrap();
            assert_eq!(square_to_blob(&symbols).unwrap(), blob);
        }
    }

    #[test]
    fn caller_chosen_k() {
        let blob = vec![0xab; 30];
        let symbols = blob_to_square(&blob, 8).unwrap();
        assert_eq!(symbols.len(), 64);
        assert_eq!(square_to_blob(&symbols).unwrap(), blob);
        assert!(matches!(
            blob_to_square(&blob, 2),
            Err(DaError::BlobTooLarge { .. })
        ));
        assert!(matches!(
            blob_to_square(&blob, 3),
            Err(DaError::InvalidK(3))
        ));
        assert!(matches!(
            blob_to_square(&blob, 1),
            Err(DaError::InvalidK(1))
        ));
    }

    #[test]
    fn rejects_non_canonical_layouts() {
        let blob = vec![1u8, 2, 3];
        let mut symbols = blob_to_square(&blob, 2).unwrap();
        symbols[3] = Fp::ONE;
        assert!(matches!(
            square_to_blob(&symbols),
            Err(DaError::InvalidPadding(_))
        ));

        let mut symbols = blob_to_square(&blob, 2).unwrap();
        // Non-zero byte after the blob inside the last data symbol.
        symbols[1] = Fp::new(symbols[1].value() | 1);
        assert!(matches!(
            square_to_blob(&symbols),
            Err(DaError::InvalidPadding(_))
        ));

        let mut symbols = blob_to_square(&blob, 2).unwrap();
        symbols[1] = Fp::new(1 << 60);
        assert!(matches!(
            square_to_blob(&symbols),
            Err(DaError::NonCanonicalSymbol { index: 1 })
        ));

        let mut symbols = blob_to_square(&blob, 2).unwrap();
        symbols[0] = Fp::new(1000);
        assert!(matches!(
            square_to_blob(&symbols),
            Err(DaError::InvalidPadding(_))
        ));
    }
}
