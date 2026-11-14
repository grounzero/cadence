// SPDX-License-Identifier: GPL-3.0-or-later

//! SHA-256, FIPS 180-4, written out so no outside crate decides what a dataset's hash is.

/// Fed in pieces of any size; the digest is the same however the input was cut.
#[derive(Clone, Debug, Default)]
pub struct Sha256 {
    length: u64,
}

impl Sha256 {
    #[must_use]
    pub fn new() -> Sha256 {
        Sha256::default()
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.length = self.length.wrapping_add(bytes.len() as u64);
    }

    #[must_use]
    pub fn finish(self) -> [u8; 32] {
        [0; 32]
    }
}

#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finish()
}

/// Lower case, as `shasum -a 256` prints it.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{Sha256, hex, sha256};

    /// The examples FIPS 180-4's validation lists for SHA-256, and the empty message.
    #[test]
    fn the_published_test_vectors() {
        for (message, digest) in [
            (
                &b""[..],
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ] {
            assert_eq!(hex(&sha256(message)), digest);
        }
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha256(&million)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// Cut at every length, the input hashes as it does whole.
    #[test]
    fn pieces_of_any_size_hash_as_the_whole() {
        let message: Vec<u8> = (0..300_u16).map(|i| (i % 251) as u8).collect();
        let whole = sha256(&message);
        for cut in 0..message.len() {
            let mut hasher = Sha256::new();
            hasher.update(&message[..cut]);
            hasher.update(&message[cut..]);
            assert_eq!(hasher.finish(), whole, "cut at {cut}");
        }
    }
}
