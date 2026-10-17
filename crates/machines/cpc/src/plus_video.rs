//! CPC Plus/GX4000 hardware sprites. The rest of the Plus picture (4096-
//! colour palette, screen split, horizontal/vertical soft scroll) is part
//! of the shared per-character trace in `crate::raster`, driven by the CRTC
//! and the ASIC registers; sprites are the one ASIC feature composited over
//! the finished picture, from the ASIC's live sprite registers.
//!
//! Real hardware composites sprites at the ASIC's own fixed dot-clock
//! density, matching Mode 2's 640-pixel width, *regardless* of the current
//! screen mode -- confirmed by the sprite-position formula in `crate::asic`'s
//! own module doc comment (`CRTC char containing the first pixel = (spriteX
//! & 0xFFF8) >> 3`, i.e. sprite X is *always* in Mode-2-equivalent units).
//! The trace's window uses that same unit, so a sprite at (X, Y) lands at
//! the display area's origin plus (X, Y).

use crate::asic::Asic;

/// Overlays the 16 hardware sprites on the finished `width` x `height`
/// picture. `origin` is where the display area's top-left corner sits in
/// that picture (sprite coordinates are relative to it).
pub(crate) fn composite_sprites(
    rgb: &mut [u8],
    width: u32,
    height: u32,
    origin: (u32, u32),
    asic: &Asic,
) {
    let border = asic.pen_color(16);
    let sprite_rgb: [(u8, u8, u8); 16] = std::array::from_fn(|i| {
        if i == 0 {
            border
        } else {
            asic.sprite_color(i as u8)
        }
    });
    let (origin_x, origin_y) = (origin.0 as i32, origin.1 as i32);
    for index in 0..16 {
        let sprite = asic.sprite(index);
        if sprite.width == 0 || sprite.height == 0 {
            continue;
        }
        let left = origin_x + i32::from(sprite.x);
        let top = origin_y + i32::from(sprite.y);
        let x0 = left.clamp(0, width as i32) as u32;
        let y0 = top.clamp(0, height as i32) as u32;
        let x1 = (left + i32::from(sprite.width)).clamp(0, width as i32) as u32;
        let y1 = (top + i32::from(sprite.height)).clamp(0, height as i32) as u32;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        for screen_y in y0..y1 {
            let src_row = ((screen_y as i32 - top) >> sprite.y_shift) as usize;
            let row_start = screen_y as usize * width as usize * 3;
            for screen_x in x0..x1 {
                let src_col = ((screen_x as i32 - left) >> sprite.x_shift) as usize;
                let pixel = asic.sprite_pixel(index, src_row.min(15), src_col.min(15));
                if pixel == 0 {
                    continue; // transparent
                }
                let color = sprite_rgb[pixel as usize];
                let out = row_start + screen_x as usize * 3;
                rgb[out] = color.0;
                rgb[out + 1] = color.1;
                rgb[out + 2] = color.2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite_zero_all_colour_one(asic: &mut Asic, x: u8, y: u8) {
        asic.write_high(0x0000, x); // X lo
        asic.write_high(0x0001, 0); // X hi
        asic.write_high(0x0002, y); // Y lo
        asic.write_high(0x0003, 0); // Y hi
        asic.write_high(0x0004, 0b0101); // xmag=1 ymag=1
        for i in 0..256u16 {
            asic.write_low(i as usize, 1);
        }
        // Sprite colour 1: bright yellow, distinguishable from black.
        asic.write_high(0x0422, 0xF0); // R=15,B=0
        asic.write_high(0x0423, 0x0F); // G=15
    }

    fn sprite_pixel_composites_over_the_picture_relative_to_the_display_origin() {
        let mut asic = Asic::new();
        sprite_zero_all_colour_one(&mut asic, 10, 10);
        let (width, height) = (100u32, 80u32);
        let mut rgb = vec![0u8; (width * height * 3) as usize];
        composite_sprites(&mut rgb, width, height, (20, 30), &asic);
        let at = |x: u32, y: u32| ((y * width + x) * 3) as usize;
        // Sprite (10,10) + origin (20,30) = picture (30,40).
        assert_eq!(&rgb[at(30, 40)..at(30, 40) + 3], &[255, 255, 0]);
        assert_eq!(&rgb[at(45, 55)..at(45, 55) + 3], &[255, 255, 0]);
        assert_eq!(&rgb[at(29, 40)..at(29, 40) + 3], &[0, 0, 0]);
        assert_eq!(&rgb[at(46, 55)..at(46, 55) + 3], &[0, 0, 0]);
        assert_eq!(&rgb[at(30, 39)..at(30, 39) + 3], &[0, 0, 0]);
    }

    fn sprites_are_clipped_at_the_picture_edges() {
        let mut asic = Asic::new();
        sprite_zero_all_colour_one(&mut asic, 0, 0);
        // Push it half off the left/top edge with a negative position.
        asic.write_high(0x0000, 0xF8); // X = -8
        asic.write_high(0x0001, 0xFF);
        asic.write_high(0x0002, 0xF8); // Y = -8
        asic.write_high(0x0003, 0xFF);
        let (width, height) = (32u32, 32u32);
        let mut rgb = vec![0u8; (width * height * 3) as usize];
        composite_sprites(&mut rgb, width, height, (0, 0), &asic);
        let at = |x: u32, y: u32| ((y * width + x) * 3) as usize;
        assert_eq!(&rgb[at(0, 0)..at(0, 0) + 3], &[255, 255, 0]);
        assert_eq!(&rgb[at(7, 7)..at(7, 7) + 3], &[255, 255, 0]);
        assert_eq!(&rgb[at(8, 8)..at(8, 8) + 3], &[0, 0, 0]);
    }
    /// Merged (every check kept): `sprite_pixel_composites_over_the_picture_relative_to_the_display_origin`, `sprites_are_clipped_at_the_picture_edges`.
    #[test]
    fn plus_sprites() {
        sprite_pixel_composites_over_the_picture_relative_to_the_display_origin();
        sprites_are_clipped_at_the_picture_edges();
    }
}
