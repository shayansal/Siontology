//! Base64, as the protocols the runtime speaks use it (HTTP Basic
//! credentials, MCP's encoded header values, PKCE), without a dependency.

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn encode(bytes: &[u8], table: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(table[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

/// Base64 with `+`, `/` and padding (RFC 4648, section 4).
pub fn base64(bytes: &[u8]) -> String {
    encode(bytes, STANDARD, true)
}

/// Base64 for URLs, with `-`, `_` and no padding (RFC 4648, section 5).
pub fn base64_url(bytes: &[u8]) -> String {
    encode(bytes, URL_SAFE, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vectors_of_rfc_4648() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), coded);
            assert_eq!(base64_url(plain.as_bytes()), coded.trim_end_matches('='));
        }
        assert_eq!(base64_url(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }
}
