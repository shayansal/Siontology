//! AES and Ed25519 for the protocols that name them, by the TLS stack's own
//! implementation (aws-lc-rs): AES in CBC mode, which WeCom's, WeChat's and
//! Feishu's encrypted callbacks use, and in ECB mode, which WeChat's media
//! takes (padding is the caller's: these protocols pad as they please,
//! WeCom to 32 bytes); Ed25519, which signs QQ's webhooks.

use aws_lc_rs::cipher::{
    DecryptingKey, DecryptionContext, EncryptingKey, EncryptionContext, UnboundCipherKey, AES_128,
    AES_256,
};
use aws_lc_rs::iv::FixedLength;

use crate::HttpError;

/// The algorithm for a key of `key`'s length: AES-128 or AES-256.
fn algorithm(key: &[u8]) -> Result<&'static aws_lc_rs::cipher::Algorithm, HttpError> {
    match key.len() {
        16 => Ok(&AES_128),
        32 => Ok(&AES_256),
        n => Err(HttpError::Config(format!(
            "an AES key has 16 or 32 bytes, not {n}"
        ))),
    }
}

fn whole_blocks(data: &[u8]) -> Result<(), HttpError> {
    if data.is_empty() || data.len() % 16 != 0 {
        return Err(HttpError::Config(format!(
            "AES takes whole blocks of 16 bytes, not {}",
            data.len()
        )));
    }
    Ok(())
}

fn failed(_: aws_lc_rs::error::Unspecified) -> HttpError {
    HttpError::Config("AES failed".into())
}

/// `data` decrypted with AES in CBC mode under `key` (16 or 32 bytes) and
/// `iv`, its padding left on.
pub fn aes_cbc_decrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, HttpError> {
    whole_blocks(data)?;
    let key = UnboundCipherKey::new(algorithm(key)?, key).map_err(failed)?;
    let mut out = data.to_vec();
    DecryptingKey::cbc(key)
        .map_err(failed)?
        .decrypt(&mut out, DecryptionContext::Iv128(FixedLength::from(*iv)))
        .map_err(failed)?;
    Ok(out)
}

/// `data` (whole blocks, padded already) encrypted with AES in CBC mode
/// under `key` and `iv`.
pub fn aes_cbc_encrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, HttpError> {
    whole_blocks(data)?;
    let key = UnboundCipherKey::new(algorithm(key)?, key).map_err(failed)?;
    let mut out = data.to_vec();
    EncryptingKey::cbc(key)
        .map_err(failed)?
        .less_safe_encrypt(&mut out, EncryptionContext::Iv128(FixedLength::from(*iv)))
        .map_err(failed)?;
    Ok(out)
}

/// `data` decrypted with AES in ECB mode under `key`, its padding left on.
pub fn aes_ecb_decrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>, HttpError> {
    whole_blocks(data)?;
    let key = UnboundCipherKey::new(algorithm(key)?, key).map_err(failed)?;
    let mut out = data.to_vec();
    DecryptingKey::ecb(key)
        .map_err(failed)?
        .decrypt(&mut out, DecryptionContext::None)
        .map_err(failed)?;
    Ok(out)
}

/// `data` (whole blocks, padded already) encrypted with AES in ECB mode
/// under `key`.
pub fn aes_ecb_encrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>, HttpError> {
    whole_blocks(data)?;
    let key = UnboundCipherKey::new(algorithm(key)?, key).map_err(failed)?;
    let mut out = data.to_vec();
    EncryptingKey::ecb(key)
        .map_err(failed)?
        .less_safe_encrypt(&mut out, EncryptionContext::None)
        .map_err(failed)?;
    Ok(out)
}

/// `data` padded as PKCS#7 has it, to a multiple of `block` bytes (16 for
/// AES; WeCom and WeChat pad to 32).
pub fn pkcs7_pad(data: &[u8], block: usize) -> Vec<u8> {
    let pad = block - data.len() % block;
    let mut out = data.to_vec();
    out.extend(std::iter::repeat(pad as u8).take(pad));
    out
}

/// `data` without its PKCS#7 padding of at most `block` bytes, if it is
/// padded so.
pub fn pkcs7_unpad(data: &[u8], block: usize) -> Result<&[u8], HttpError> {
    let pad = usize::from(*data.last().unwrap_or(&0));
    if pad == 0
        || pad > block
        || pad > data.len()
        || !data[data.len() - pad..]
            .iter()
            .all(|b| usize::from(*b) == pad)
    {
        return Err(HttpError::Config("the padding is not PKCS#7".into()));
    }
    Ok(&data[..data.len() - pad])
}

/// The Ed25519 public key of the 32-byte `seed`.
pub fn ed25519_public(seed: &[u8; 32]) -> Result<[u8; 32], HttpError> {
    use aws_lc_rs::signature::KeyPair;
    let pair = aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(seed)
        .map_err(|e| HttpError::Config(format!("an Ed25519 seed: {e}")))?;
    let mut public = [0u8; 32];
    public.copy_from_slice(pair.public_key().as_ref());
    Ok(public)
}

/// The Ed25519 signature of `message` by the key of `seed`.
pub fn ed25519_sign(seed: &[u8; 32], message: &[u8]) -> Result<[u8; 64], HttpError> {
    let pair = aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(seed)
        .map_err(|e| HttpError::Config(format!("an Ed25519 seed: {e}")))?;
    let mut signature = [0u8; 64];
    signature.copy_from_slice(pair.sign(message).as_ref());
    Ok(signature)
}

/// Whether `signature` is `public`'s Ed25519 signature of `message`.
pub fn ed25519_verify(public: &[u8; 32], message: &[u8], signature: &[u8]) -> bool {
    aws_lc_rs::signature::UnparsedPublicKey::new(&aws_lc_rs::signature::ED25519, public)
        .verify(message, signature)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn aes_gives_nists_answers() {
        // NIST SP 800-38A, F.2.5 (CBC-AES256) and F.1.1 (ECB-AES128): the
        // first block of each.
        let key = hex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4");
        let iv: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let plain = hex("6bc1bee22e409f96e93d7e117393172a");
        let cipher = hex("f58c4c04d6e5f1ba779eabfb5f7bfbd6");
        assert_eq!(aes_cbc_encrypt(&key, &iv, &plain).unwrap(), cipher);
        assert_eq!(aes_cbc_decrypt(&key, &iv, &cipher).unwrap(), plain);
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let cipher = hex("3ad77bb40d7a3660a89ecaf32466ef97");
        assert_eq!(aes_ecb_encrypt(&key, &plain).unwrap(), cipher);
        assert_eq!(aes_ecb_decrypt(&key, &cipher).unwrap(), plain);
        assert!(aes_ecb_decrypt(&key, &cipher[..15]).is_err());
        assert!(aes_ecb_decrypt(&key[..15], &cipher).is_err());
    }

    #[test]
    fn ed25519_gives_rfc_8032s_answers() {
        // RFC 8032, 7.1, test 1: the empty message.
        let seed: [u8; 32] =
            hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
                .try_into()
                .unwrap();
        let public = ed25519_public(&seed).unwrap();
        assert_eq!(
            public.to_vec(),
            hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        );
        let signature = ed25519_sign(&seed, b"").unwrap();
        assert_eq!(
            signature.to_vec(),
            hex("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
        );
        assert!(ed25519_verify(&public, b"", &signature));
        assert!(!ed25519_verify(&public, b"x", &signature));
    }

    #[test]
    fn padding_is_pkcs7s() {
        assert_eq!(pkcs7_pad(b"abc", 16).len(), 16);
        assert_eq!(pkcs7_pad(&[0; 32], 32).len(), 64);
        let padded = pkcs7_pad(b"hello", 32);
        assert_eq!(padded[31], 27);
        assert_eq!(pkcs7_unpad(&padded, 32).unwrap(), b"hello");
        assert!(
            pkcs7_unpad(&padded, 16).is_err(),
            "27 is more than a block of 16"
        );
        assert!(pkcs7_unpad(b"hello\x00", 16).is_err());
        assert!(pkcs7_unpad(b"hell\x02\x03", 16).is_err());
    }
}
