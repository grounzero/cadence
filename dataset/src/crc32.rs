// SPDX-License-Identifier: GPL-3.0-or-later

//! CRC-32 as zlib computes it (IEEE 802.3, reflected), which finds a torn or flipped record.

const TABLE: [u32; 256] = table();

const fn table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = !0_u32;
    for &byte in bytes {
        c = TABLE[((c ^ u32::from(byte)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
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
