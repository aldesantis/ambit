//! SHA-256, hex-encoded.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// The lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Lowercase hex of a finished digest, such as `Sha256::finalize`'s output.
///
/// Written by hand rather than through `LowerHex`, which sha2 0.11's output array does not promise.
pub fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a String cannot fail");
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_the_empty_string() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
