//! Lowercase hex for byte strings in JSON (serde only; Borsh is untouched).

/// `#[serde(with = "hex_bytes")]` for `Vec<u8>` fields.
pub(crate) mod hex_bytes {
    use serde::{de, Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::encode(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        super::decode(&text).map_err(de::Error::custom)
    }
}

const DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Lowercase hex encoding.
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decodes hex (either case); rejects odd lengths and other characters.
pub(crate) fn decode(text: &str) -> Result<Vec<u8>, &'static str> {
    let bytes = text.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err("hex string has odd length");
    }
    bytes
        .chunks(2)
        .map(|pair| match (nibble(pair[0]), nibble(pair[1])) {
            (Some(high), Some(low)) => Ok((high << 4) | low),
            _ => Err("invalid hex digit"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        let bytes = vec![0x00, 0x7f, 0x80, 0xff, 0x12];
        let text = encode(&bytes);
        assert_eq!(text, "007f80ff12");
        assert_eq!(decode(&text).unwrap(), bytes);
        assert_eq!(decode("ABcd").unwrap(), vec![0xab, 0xcd]);
        assert!(decode("abc").is_err());
        assert!(decode("zz").is_err());
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
    }
}
