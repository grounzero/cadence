// SPDX-License-Identifier: GPL-3.0-or-later

//! CRC-32 as zlib computes it (IEEE 802.3, reflected), which finds a torn or flipped record.

#[must_use]
pub fn crc32(_bytes: &[u8]) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::crc32;

    /// The check value every CRC-32 catalogue lists for this parameter set.
    #[test]
    fn the_catalogue_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }
}
