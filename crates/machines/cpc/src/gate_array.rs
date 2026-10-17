//! Amstrad Gate Array -- the CPC's own custom chip: pen/border/ink color
//! selection, screen mode + ROM enable/disable, and the real HSYNC-driven
//! interrupt counter. Port-decoded by the host bus at any address with
//! `A15=0, A14=1` (real hardware only decodes those 2 bits -- see
//! `CREDITS.md`), disambiguated purely by the top 2 bits of the *data
//! byte* written, not a second address. Kept as its own module inside
//! `cpc` rather than a shared crate -- single consumer so far, same
//! "don't extract from one example" principle this codebase already
//! applies elsewhere (e.g. `sinclair-keyboard` only became a shared
//! crate once a second, identical consumer existed).

/// The Amstrad CPC hardware palette, indexed by the real *hardware*
/// color number (`0..32`, the raw value the Gate Array's ink-select
/// register actually receives -- firmware translates its own 0-26
/// "firmware color number" scheme into one of these before writing the
/// port, this Gate Array module only ever sees the hardware number, so
/// that's what this table is indexed by). Values fetched directly from
/// CPCTech's own Gate Array documentation
/// <https://cpctech.cpcwiki.de/docs/garray.html> (see `CREDITS.md`) --
/// its firmware-to-hardware-to-RGB-level conversion table, inverted here
/// to be hardware-number-indexed; each RGB level (`0`/`1`/`2` = off/
/// half/full) mapped to `0x00`/`0x80`/`0xFF`, the same 3-level
/// approximation as before, just now with the *correct* per-channel
/// assignment instead of a structurally-plausible guess. Only 27 of the
/// 32 possible hardware values are real, documented colors -- the
/// remaining 5 (`1`, `8`, `9`, `16`, `17`) are unused/undocumented on
/// real silicon and never legitimately written by real firmware, so
/// they're filled with black here rather than guessed.
pub const PALETTE: [(u8, u8, u8); 32] = {
    const LEVELS: [u8; 3] = [0x00, 0x80, 0xFF];
    // (hardware number, R level, G level, B level) for each of the 27
    // real colors, per the cited table.
    const ENTRIES: [(usize, u8, u8, u8); 27] = [
        (20, 0, 0, 0),
        (4, 0, 0, 1),
        (21, 0, 0, 2),
        (28, 1, 0, 0),
        (24, 1, 0, 1),
        (29, 1, 0, 2),
        (12, 2, 0, 0),
        (5, 2, 0, 1),
        (13, 2, 0, 2),
        (22, 0, 1, 0),
        (6, 0, 1, 1),
        (23, 0, 1, 2),
        (30, 1, 1, 0),
        (0, 1, 1, 1),
        (31, 1, 1, 2),
        (14, 2, 1, 0),
        (7, 2, 1, 1),
        (15, 2, 1, 2),
        (18, 0, 2, 0),
        (2, 0, 2, 1),
        (19, 0, 2, 2),
        (26, 1, 2, 0),
        (25, 1, 2, 1),
        (27, 1, 2, 2),
        (10, 2, 2, 0),
        (3, 2, 2, 1),
        (11, 2, 2, 2),
    ];
    let mut table = [(0u8, 0u8, 0u8); 32];
    let mut i = 0;
    while i < 27 {
        let (hw, r, g, b) = ENTRIES[i];
        table[hw] = (LEVELS[r as usize], LEVELS[g as usize], LEVELS[b as usize]);
        i += 1;
    }
    table
};

/// The Gate Array advances its six-bit interrupt counter on each falling
/// CRTC HSYNC edge. It normally fires at 52, while a rising VSYNC schedules
/// the real frame-lock rule two HSYNCs later: reset the counter and also fire
/// when its pre-reset value is at least 32.
const HSYNC_INTERRUPT_COUNT: u8 = 52;
const HSYNCS_AFTER_VSYNC: u8 = 2;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GateArray {
    selected_pen: u8,
    pen_colors: [u8; 17], // index 0-15 = pens, index 16 = border
    screen_mode: u8,
    requested_screen_mode: u8,
    lower_rom_disabled: bool,
    upper_rom_disabled: bool,
    hsync_counter: u8,
    hsyncs_after_vsync: u8,
    /// The Gate Array's `/INT` output. Real hardware raises it when the
    /// counter fires and holds it until the Z80 acknowledges the interrupt
    /// (or an `RMR` write with bit 4 set clears it), so an interrupt that
    /// arrives while the CPU has interrupts disabled is still waiting once
    /// they are enabled again -- not lost. Caprice32 (`z80.int_pending`),
    /// CPCEC (`z80_irq`) and ep128emu (`triggerInterrupt`/`clearInterrupt`)
    /// all behave this way.
    interrupt_line: bool,
}

impl GateArray {
    /// `screen_mode` starts at `1`, not the derived-`Default` `0` --
    /// real CPC hardware's Gate Array powers on into Mode 1 (the same
    /// mode the firmware's own boot screen uses, before it ever issues
    /// its own explicit mode-select `OUT`), not Mode 0. Getting this
    /// right matters beyond cosmetics: when CPC video first landed,
    /// `Cpc::render_frame` implemented only Mode 1 and the GUI queried it
    /// before running any instructions. The old Mode 0 default therefore
    /// failed startup even though firmware later selected Mode 1. Modes
    /// 0/1/2 are all implemented now, but matching the hardware power-on
    /// state still gives callers the correct initial resolution.
    pub fn new() -> Self {
        Self {
            screen_mode: 1,
            requested_screen_mode: 1,
            ..Self::default()
        }
    }

    pub fn screen_mode(&self) -> u8 {
        self.screen_mode
    }

    pub fn lower_rom_disabled(&self) -> bool {
        self.lower_rom_disabled
    }

    pub fn upper_rom_disabled(&self) -> bool {
        self.upper_rom_disabled
    }

    pub fn pen_color(&self, pen: u8) -> (u8, u8, u8) {
        // `pen_colors` already stores a 5-bit-masked value (`write`'s
        // `val & 0x1F`), the exact real range `PALETTE` is now sized for
        // -- no further masking needed.
        PALETTE[self.pen_colors[(pen as usize).min(16)] as usize]
    }

    pub(crate) fn selected_pen(&self) -> u8 {
        self.selected_pen
    }

    pub(crate) fn pen_colors(&self) -> [u8; 17] {
        self.pen_colors
    }

    pub(crate) fn rom_config(&self) -> u8 {
        (self.requested_screen_mode & 0x03)
            | u8::from(self.lower_rom_disabled) << 2
            | u8::from(self.upper_rom_disabled) << 3
    }

    /// Function-select write -- the top 2 bits of `val` pick which of
    /// the Gate Array's 4 real functions this write targets (pen select/
    /// ink select/mode+rom+interrupt-reset/RAM banking), per this
    /// module's own doc comment.
    pub fn write(&mut self, val: u8) {
        match (val >> 6) & 0x03 {
            0b00 => self.selected_pen = val & 0x1F,
            0b01 => {
                let pen = (self.selected_pen as usize).min(16);
                self.pen_colors[pen] = val & 0x1F;
            }
            0b10 => {
                // Real mode changes are latched at the next falling HSYNC.
                self.requested_screen_mode = val & 0x03;
                self.lower_rom_disabled = val & 0x04 != 0;
                self.upper_rom_disabled = val & 0x08 != 0;
                if val & 0x10 != 0 {
                    // "Delay interrupt generation": reset the counter and
                    // clear any interrupt still waiting to be taken.
                    self.hsync_counter = 0;
                    self.interrupt_line = false;
                }
            }
            0b11 => {} // Separate RAM PAL/expansion sees this write; host bus applies it.
            _ => unreachable!(),
        }
    }

    /// Whether `/INT` is currently asserted (raised and not yet
    /// acknowledged or cleared).
    pub fn interrupt_line(&self) -> bool {
        self.interrupt_line
    }

    /// Raises `/INT`. It stays up until `acknowledge_interrupt` or an `RMR`
    /// write with bit 4 set.
    pub fn assert_interrupt(&mut self) {
        self.interrupt_line = true;
    }

    /// The Z80 acknowledged the interrupt: `/INT` drops and bit 5 of the
    /// scanline counter is cleared (so the next regular interrupt comes
    /// 32..52 scanlines after a late acknowledge instead of 52).
    pub fn acknowledge_interrupt(&mut self) {
        self.interrupt_line = false;
        self.hsync_counter &= 0x1F;
    }

    /// Marks the CRTC's rising VSYNC edge. The Gate Array waits for two
    /// falling HSYNC edges before applying its frame-lock reset rule.
    pub fn vsync_started(&mut self) {
        self.hsyncs_after_vsync = HSYNCS_AFTER_VSYNC;
    }

    /// Processes one falling CRTC HSYNC edge and returns whether the Gate
    /// Array raised its Z80 interrupt on this edge.
    pub fn hsync_falling_edge(&mut self) -> bool {
        self.screen_mode = self.requested_screen_mode;
        self.hsync_counter = self.hsync_counter.wrapping_add(1);

        let mut interrupt = false;
        if self.hsyncs_after_vsync != 0 {
            self.hsyncs_after_vsync -= 1;
            if self.hsyncs_after_vsync == 0 {
                interrupt = self.hsync_counter >= 32;
                self.hsync_counter = 0;
            }
        }

        if self.hsync_counter >= HSYNC_INTERRUPT_COUNT {
            self.hsync_counter = 0;
            interrupt = true;
        }
        interrupt
    }

    /// CPC Plus/GX4000 ASIC-only interaction: a Programmable Raster
    /// Interrupt resets this counter's top bit, keeping the classic
    /// interrupt mechanism loosely synchronized so a pending classic
    /// threshold doesn't fire immediately once PRI is later disabled --
    /// see `crate::asic`'s own doc comment and CREDITS.md.
    pub(crate) fn reset_top_bit(&mut self) {
        self.hsync_counter &= 0x1F;
    }

    pub(crate) fn restore_snapshot_state(
        &mut self,
        selected_pen: u8,
        pen_colors: [u8; 17],
        rom_config: u8,
    ) {
        self.selected_pen = selected_pen & 0x1F;
        self.pen_colors = pen_colors.map(|color| color & 0x1F);
        self.requested_screen_mode = rom_config & 0x03;
        self.screen_mode = self.requested_screen_mode;
        self.lower_rom_disabled = rom_config & 0x04 != 0;
        self.upper_rom_disabled = rom_config & 0x08 != 0;
        self.hsync_counter = 0;
        self.hsyncs_after_vsync = 0;
        self.interrupt_line = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both_roms_enabled_by_default() {
        let ga = GateArray::new();
        assert!(!ga.lower_rom_disabled());
        assert!(!ga.upper_rom_disabled());
    }

    /// Real hardware powers on into Mode 1, not Mode 0 -- see `new`'s own
    /// doc comment for the real bug this fixes (`Cpc::render_frame`
    /// returning `None` before any instruction had run, failing
    /// `retroclawd`'s GUI startup preflight check).
    fn screen_mode_defaults_to_1_matching_real_hardware_power_on() {
        assert_eq!(GateArray::new().screen_mode(), 1);
    }
    /// Merged (every check kept): `both_roms_enabled_by_default`, `screen_mode_defaults_to_1_matching_real_hardware_power_on`.
    #[test]
    fn power_on_defaults() {
        both_roms_enabled_by_default();
        screen_mode_defaults_to_1_matching_real_hardware_power_on();
    }

    #[test]
    fn function_10_latches_mode_on_the_next_hsync_and_sets_rom_bits_immediately() {
        let mut ga = GateArray::new();
        ga.write(0x8E); // function 10, upper+lower ROM disabled, request mode 2
        assert_eq!(ga.screen_mode(), 1);
        assert!(ga.lower_rom_disabled());
        assert!(ga.upper_rom_disabled());
        ga.hsync_falling_edge();
        assert_eq!(ga.screen_mode(), 2);
    }

    #[test]
    fn pen_and_ink_select_are_two_separate_writes() {
        let mut ga = GateArray::new();
        ga.write(0b00_000101); // function 00: select pen 5
        ga.write(0b01_010100); // function 01: set that pen's ink to 20
        assert_eq!(ga.pen_color(5), PALETTE[20]);
    }

    /// Locks in a real cited value, not just write/read consistency:
    /// hardware color 4 is "Blue" per CPCTech's own table (see
    /// `PALETTE`'s doc comment) -- RGB levels 0/0/1, i.e. blue channel
    /// only, at half intensity.
    fn hardware_color_4_is_blue_per_the_cited_reference_table() {
        assert_eq!(PALETTE[4], (0x00, 0x00, 0x80));
    }

    /// Hardware color 0 is "White" (RGB levels 1/1/1, all channels at
    /// half) -- deliberately *not* black, a real, slightly counter-
    /// intuitive fact worth locking in since it's easy to assume 0
    /// means black.
    fn hardware_color_0_is_white_not_black() {
        assert_eq!(PALETTE[0], (0x80, 0x80, 0x80));
    }
    /// Merged (every check kept): `hardware_color_4_is_blue_per_the_cited_reference_table`, `hardware_color_0_is_white_not_black`.
    #[test]
    fn hardware_colours() {
        hardware_color_4_is_blue_per_the_cited_reference_table();
        hardware_color_0_is_white_not_black();
    }

    fn interrupt_fires_and_resets_at_52_hsyncs() {
        let mut ga = GateArray::new();
        for _ in 0..51 {
            assert!(!ga.hsync_falling_edge());
        }
        assert!(
            ga.hsync_falling_edge(),
            "52nd HSYNC should fire the interrupt"
        );
        assert!(!ga.hsync_falling_edge(), "counter should have reset to 0");
    }

    fn vsync_resets_counter_two_hsyncs_later_and_fires_at_32_or_more() {
        let mut ga = GateArray::new();
        for _ in 0..30 {
            assert!(!ga.hsync_falling_edge());
        }
        ga.vsync_started();
        assert!(!ga.hsync_falling_edge());
        assert!(ga.hsync_falling_edge());
        assert!(!ga.hsync_falling_edge());
    }

    fn vsync_suppresses_early_interrupt_below_32_hsyncs() {
        let mut ga = GateArray::new();
        for _ in 0..20 {
            assert!(!ga.hsync_falling_edge());
        }
        ga.vsync_started();
        assert!(!ga.hsync_falling_edge());
        assert!(!ga.hsync_falling_edge());
        for _ in 0..51 {
            assert!(!ga.hsync_falling_edge());
        }
        assert!(ga.hsync_falling_edge());
    }
    /// Merged (every check kept): `interrupt_fires_and_resets_at_52_hsyncs`, `vsync_resets_counter_two_hsyncs_later_and_fires_at_32_or_more`, `vsync_suppresses_early_interrupt_below_32_hsyncs`.
    #[test]
    fn interrupt_counter() {
        interrupt_fires_and_resets_at_52_hsyncs();
        vsync_resets_counter_two_hsyncs_later_and_fires_at_32_or_more();
        vsync_suppresses_early_interrupt_below_32_hsyncs();
    }

    fn the_interrupt_line_is_held_until_it_is_acknowledged() {
        let mut ga = GateArray::new();
        assert!(!ga.interrupt_line());
        ga.assert_interrupt();
        // Further scanlines (even another counter wrap) leave it asserted.
        for _ in 0..120 {
            ga.hsync_falling_edge();
            assert!(ga.interrupt_line());
        }
        ga.acknowledge_interrupt();
        assert!(!ga.interrupt_line());
    }

    fn acknowledging_clears_bit_5_of_the_scanline_counter() {
        // Counter at 40 when the CPU finally acknowledges: bit 5 (32) is
        // cleared, leaving 8, so the next interrupt is 44 scanlines away
        // instead of 12 (Caprice32 `sl_count &= 0x1f`, CPCEC `irq_timer &=
        // ~32`, ep128emu `gateArrayIRQCounter & 0x1F`).
        let mut ga = GateArray::new();
        for _ in 0..40 {
            assert!(!ga.hsync_falling_edge());
        }
        ga.acknowledge_interrupt();
        for _ in 0..43 {
            assert!(!ga.hsync_falling_edge());
        }
        assert!(ga.hsync_falling_edge(), "52 - 8 = 44th scanline");

        // Below 32 nothing changes.
        let mut ga = GateArray::new();
        for _ in 0..20 {
            ga.hsync_falling_edge();
        }
        ga.acknowledge_interrupt();
        for _ in 0..31 {
            assert!(!ga.hsync_falling_edge());
        }
        assert!(ga.hsync_falling_edge(), "still the 32nd one after 20");
    }
    /// Merged (every check kept): `the_interrupt_line_is_held_until_it_is_acknowledged`, `acknowledging_clears_bit_5_of_the_scanline_counter`.
    #[test]
    fn interrupt_acknowledge() {
        the_interrupt_line_is_held_until_it_is_acknowledged();
        acknowledging_clears_bit_5_of_the_scanline_counter();
    }

    fn the_delay_bit_of_the_mode_register_clears_a_pending_interrupt() {
        let mut ga = GateArray::new();
        ga.assert_interrupt();
        ga.write(0x8D); // bit 4 clear: leaves it alone
        assert!(ga.interrupt_line());
        ga.write(0x9D); // bit 4 set
        assert!(!ga.interrupt_line());
    }

    fn interrupt_counter_reset_bit_clears_pending_count() {
        let mut ga = GateArray::new();
        for _ in 0..20 {
            assert!(!ga.hsync_falling_edge());
        }
        ga.write(0x90);
        for _ in 0..51 {
            assert!(!ga.hsync_falling_edge());
        }
        assert!(ga.hsync_falling_edge());
    }
    /// Merged (every check kept): `the_delay_bit_of_the_mode_register_clears_a_pending_interrupt`, `interrupt_counter_reset_bit_clears_pending_count`.
    #[test]
    fn interrupt_clearing() {
        the_delay_bit_of_the_mode_register_clears_a_pending_interrupt();
        interrupt_counter_reset_bit_clears_pending_count();
    }
}
