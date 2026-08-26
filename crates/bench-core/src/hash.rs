//! The ONE sha256→lowercase-hex helper.
//!
//! #58: this formatting was open-coded at ~7 sites (`golden::load_golden_fixture`,
//! `golden::verify_golden_integrity`, `tape::load_timed_prompt_tape`, benchctl's
//! `score::sha256_hex`, and three test-local copies). Every one produced the same
//! bytes, but each was an independent chance to drift — and a golden's identity pin
//! (`GoldenIntegrityPin`) is only as trustworthy as the digest it is compared against.
//! One helper, reused everywhere, removes that class of drift entirely.

use sha2::{Digest, Sha256};

/// Lowercase-hex sha256 of `bytes`.
///
/// Byte-identical to `shasum -a 256` / Swift `SHA256.hash(data:)` rendered lowercase —
/// the form the golden integrity pins, the `.sha256` score sidecar, and the weights
/// dir digest all record.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}

/// Render already-computed digest bytes as lowercase hex. Split out so a streaming
/// hasher (multi-GB weights files are hashed in chunks, never buffered whole) shares
/// the exact same rendering as the one-shot [`sha256_hex`].
pub fn hex_lower(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_vectors() {
        // FIPS 180-2 / RFC 6234 test vectors — pins the helper to the standard, so a
        // reuse site can never be "the same as the other sites" but wrong.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hex_lower_is_zero_padded_and_lowercase() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xff, 0xa0]), "000fffa0");
    }
}
