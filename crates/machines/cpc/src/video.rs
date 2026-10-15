//! CPC Gate Array pixel decoding: how one video-RAM byte becomes pens in
//! each of the screen modes. The picture itself -- which bytes, when, with
//! what palette -- is assembled by `crate::raster` from the CRTC's
//! character clocks; this module only knows the per-byte bit layouts.

/// Mode 0 byte layout, from most- to least-significant bit:
/// `L0 R0 L2 R2 L1 R1 L3 R3`.
pub(crate) fn decode_mode0(byte: u8) -> [u8; 2] {
    let left = ((byte >> 7) & 1) | ((byte >> 2) & 2) | ((byte >> 3) & 4) | ((byte << 2) & 8);
    let right = ((byte >> 6) & 1) | ((byte >> 1) & 2) | ((byte >> 2) & 4) | ((byte << 3) & 8);
    [left, right]
}

/// Mode 1 byte layout, from most- to least-significant bit:
/// `P0.0 P1.0 P2.0 P3.0 P0.1 P1.1 P2.1 P3.1`.
#[inline]
pub(crate) fn decode_mode1(byte: u8, pixel: u8) -> u8 {
    ((byte >> (7 - pixel)) & 1) | (((byte >> (3 - pixel)) & 1) << 1)
}

/// Mode 2 byte layout: 8 monochrome pixels, MSB first.
pub(crate) fn decode_mode2(byte: u8) -> [u8; 8] {
    std::array::from_fn(|i| (byte >> (7 - i)) & 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode0_decodes_two_four_bit_interleaved_pixels() {
        // Layout L0 R0 L2 R2 L1 R1 L3 R3: left=13, right=6.
        assert_eq!(decode_mode0(0xB6), [13, 6]);
        assert_eq!(decode_mode0(0x00), [0, 0]);
        assert_eq!(decode_mode0(0xFF), [15, 15]);
        assert_eq!(decode_mode0(0x80), [1, 0]);
        assert_eq!(decode_mode0(0x40), [0, 1]);
    }

    #[test]
    fn mode1_uses_the_high_nibble_for_pen_bit_zero() {
        assert_eq!(decode_mode1(0x80, 0), 1);
        assert_eq!(decode_mode1(0x08, 0), 2);
        assert_eq!(decode_mode1(0x88, 0), 3);
        assert_eq!(decode_mode1(0x88, 1), 0);
        assert_eq!(decode_mode1(0x01, 3), 2);
        assert_eq!(decode_mode1(0x10, 3), 1);
    }

    #[test]
    fn mode2_decodes_eight_one_bit_pixels_msb_first() {
        assert_eq!(decode_mode2(0b1010_0000), [1, 0, 1, 0, 0, 0, 0, 0]);
        assert_eq!(decode_mode2(0x01), [0, 0, 0, 0, 0, 0, 0, 1]);
    }
}
