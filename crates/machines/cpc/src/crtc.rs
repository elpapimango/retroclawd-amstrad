//! Motorola/Hitachi 6845-compatible CRTC timing as wired into the Amstrad CPC.
//!
//! The CRTC runs at 1 MHz, one character clock for every four 4 MHz Z80
//! T-states. Its horizontal, raster, vertical, sync, and vertical-adjust
//! counters are driven directly from R0/R2-R5/R7/R9, and its memory-address
//! generator (`VMA`/`VMA'`, R12/R13) follows the standard 6845 rules, so the
//! host can ask, for every character clock, exactly which video-RAM address
//! the chip is presenting and whether display is enabled (see [`CharClock`]).
//!
//! **CRTC types.** Real CPCs shipped with several silicon variants that
//! differ in register read-back, the status register, sync-width handling and
//! a few start-address rules. [`CrtcType`] selects one of the five
//! commonly-numbered types (0 HD6845S/UM6845, 1 UM6845R, 2 MC6845,
//! 3 AMS40489 = CPC+ ASIC, 4 AMS40226 = cost-down "pre-ASIC"). The per-type
//! facts modelled here come from the "Amstrad CPC CRTC Compendium" (ACCC) by
//! Longshot as transcribed, with section references, in CPCSyntaxError's
//! `crtc_type_*` sources (MIT; see `CREDITS.md`), cross-checked against
//! Caprice32's `crtc.cpp` for the type-independent counter/address rules.
//!
//! Deliberately *not* modelled (documented scope, not oversights):
//! interlace (`R8` bits 1-0), cursor and light-pen registers (`R16`/`R17`
//! always read `0`), the ACCC's sub-character (microsecond) effects and its
//! rarely-exercised counter-overflow / "rupture" corner cases beyond the
//! start-address behaviour described on [`Crtc::advance_scanline`].

use serde::{Deserialize, Serialize};

const REGISTER_COUNT: usize = 18;
const TSTATES_PER_CHARACTER: u32 = 4;

/// Which CRTC silicon variant a machine carries. Numbering follows the
/// community convention (CPCWiki/ACCC): types 0-2 are separate 6845-family
/// chips, types 3 and 4 are CRTC cores integrated into Amstrad ASICs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CrtcType {
    /// Hitachi HD6845S / UMC UM6845 (behave identically).
    #[default]
    Type0,
    /// UMC UM6845R -- the only type with a readable status register.
    Type1,
    /// Motorola MC6845.
    Type2,
    /// Amstrad AMS40489 (CPC+ ASIC).
    Type3,
    /// Amstrad AMS40226 ("pre-ASIC" cost-down CPC6128).
    Type4,
}

impl CrtcType {
    pub const ALL: [CrtcType; 5] = [
        CrtcType::Type0,
        CrtcType::Type1,
        CrtcType::Type2,
        CrtcType::Type3,
        CrtcType::Type4,
    ];

    pub const fn index(self) -> u8 {
        match self {
            Self::Type0 => 0,
            Self::Type1 => 1,
            Self::Type2 => 2,
            Self::Type3 => 3,
            Self::Type4 => 4,
        }
    }

    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Type0),
            1 => Some(Self::Type1),
            2 => Some(Self::Type2),
            3 => Some(Self::Type3),
            4 => Some(Self::Type4),
            _ => None,
        }
    }

    pub const fn chip_name(self) -> &'static str {
        match self {
            Self::Type0 => "HD6845S/UM6845",
            Self::Type1 => "UM6845R",
            Self::Type2 => "MC6845",
            Self::Type3 => "AMS40489",
            Self::Type4 => "AMS40226",
        }
    }

    /// ACCC §14.2/§16.2.1: R3's high nibble sets the VSYNC length on types
    /// 0, 3 and 4 only; types 1 and 2 have it fixed at 16 lines.
    const fn programmable_vsync_width(self) -> bool {
        matches!(self, Self::Type0 | Self::Type3 | Self::Type4)
    }

    /// ACCC §19.2: the `SKEW DISP` field (R8 bits 5-4) exists on types 0, 3
    /// and 4 only.
    const fn has_display_skew(self) -> bool {
        matches!(self, Self::Type0 | Self::Type3 | Self::Type4)
    }

    /// ACCC §14.6: "When R3=0, CRTC's 0 and 1 do not produce HSYNC"; on
    /// types 2, 3 and 4 an `R3l` of 0 is a 16-character HSYNC instead.
    const fn zero_hsync_width_means_none(self) -> bool {
        matches!(self, Self::Type0 | Self::Type1)
    }

    /// Bits of each register that a write actually stores (ACCC §4.3,
    /// p.22). `R16`/`R17` (light pen) are read-only and `R18+` don't exist,
    /// so both take no writes.
    const fn write_mask(self, register: usize) -> u8 {
        match register {
            0..=2 => 0xFF,
            3 => {
                if self.programmable_vsync_width() {
                    0xFF
                } else {
                    0x0F
                }
            }
            4 => 0x7F,
            5 => 0x1F,
            6 | 7 => 0x7F,
            8 => {
                if self.has_display_skew() {
                    0xF3
                } else {
                    0x03
                }
            }
            9 => 0x1F,
            10 => 0x7F,
            11 => 0x1F,
            12 | 14 => 0x3F,
            13 | 15 => 0xFF,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CrtcEvents {
    /// Falling HSYNC edges. The CPC Gate Array advances its interrupt counter
    /// on this edge, not at a fixed wall-clock interval.
    pub hsync_falls: u8,
    /// Rising VSYNC edges. The Gate Array uses this to frame-lock its six-per-
    /// frame interrupt sequence.
    pub vsync_rises: u8,
    pub frame_starts: u8,
}

impl CrtcEvents {
    /// Accumulates another span's events (one instruction's tick can be
    /// split at the T-states of its video-affecting writes).
    pub(crate) fn merge(&mut self, other: CrtcEvents) {
        self.hsync_falls = self.hsync_falls.saturating_add(other.hsync_falls);
        self.vsync_rises = self.vsync_rises.saturating_add(other.vsync_rises);
        self.frame_starts = self.frame_starts.saturating_add(other.frame_starts);
    }
}

/// What the CRTC pins present during one character clock -- everything the
/// Gate Array (and so the picture) needs from the chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CharClock {
    /// 14-bit `MA` address of this character. Screen bytes live at
    /// `ram_address(ma, ra)` (first byte) and that `+ 1` (second byte).
    pub ma: u16,
    /// Raster address (`RA`, 5-bit counter) inside the character row.
    pub ra: u8,
    /// `DISPEN` as the Gate Array sees it: horizontal window (with any
    /// display skew) AND vertical window.
    pub display: bool,
    pub hsync: bool,
    pub vsync: bool,
    /// Horizontal character counter (`C0`) of this clock.
    pub c0: u8,
    /// Register `R0`: the last character of this line has `c0 == r0`.
    pub r0: u8,
    /// Register `R2` (HSYNC position) at this clock.
    pub r2: u8,
    /// Register `R1` (characters displayed per row): also the row stride
    /// that separates one character row's `MA` from the next.
    pub r1: u8,
    /// Register `R9` (maximum raster address), the last `RA` of a row.
    pub max_raster: u8,
}

/// CPC video-RAM byte address for the first byte of a character: `MA0-9`
/// -> `A1-A10`, `MA12-13` -> `A14-A15`, `RA0-2` -> `A11-A13` (`MA10-11`
/// aren't wired to the address bus). The second byte of the character is at
/// the returned address `| 1`.
pub(crate) const fn ram_address(ma: u16, ra: u8) -> u16 {
    let doubled = (ma & 0x3FFF) << 1;
    ((doubled & 0x07FE) | ((doubled & 0x6000) << 1)) + ((ra as u16 & 7) << 11)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crtc {
    crtc_type: CrtcType,
    selected_register: u8,
    registers: [u8; REGISTER_COUNT],
    tstate_remainder: u8,
    horizontal_counter: u8,
    raster_counter: u8,
    vertical_counter: u8,
    vertical_adjust_counter: u8,
    in_vertical_adjust: bool,
    hsync_remaining: u8,
    /// Lines the current VSYNC has already lasted (0 at its first line).
    vsync_counter: u8,
    hsync: bool,
    vsync: bool,
    /// Vertical `DISPEN`: set at frame start, cleared when `C4` reaches R6.
    vertical_display: bool,
    /// `VMA`: `MA` of the first character of the current character row.
    row_address: u16,
    /// `VMA'`: the address the *next* character row starts at (latched when
    /// `C0` reaches R1 on the row's last raster).
    next_row_address: u16,
    /// UM6845R status bit 5 latch (ACCC §21.3.1).
    border_r6_status: bool,
    /// UM6845R: the light-pen register has been read since power-on (reading R16 or R17 clears the status
    /// register's bit 6, which is set from power-on -- the CPC acid test `type1ctrl`).
    lpen_read: bool,
    frame_counter: u32,
    /// Scanlines since the frame started (0 on its first line).
    frame_line: u16,
    /// CPC+ screen split: `(line, MA)`. From `frame_line == line` on, the
    /// row address restarts from that `MA` (`line == 0` means no split).
    split: Option<(u8, u16)>,
}

impl Default for Crtc {
    fn default() -> Self {
        Self {
            crtc_type: CrtcType::default(),
            selected_register: 0,
            registers: [0; REGISTER_COUNT],
            tstate_remainder: 0,
            horizontal_counter: 0,
            raster_counter: 0,
            vertical_counter: 0,
            vertical_adjust_counter: 0,
            in_vertical_adjust: false,
            hsync_remaining: 0,
            vsync_counter: 0,
            hsync: false,
            vsync: false,
            vertical_display: true,
            row_address: 0,
            next_row_address: 0,
            border_r6_status: false,
            lpen_read: false,
            frame_counter: 0,
            frame_line: 0,
            split: None,
        }
    }
}

impl Crtc {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_type(crtc_type: CrtcType) -> Self {
        Self {
            crtc_type,
            ..Self::default()
        }
    }

    pub fn crtc_type(&self) -> CrtcType {
        self.crtc_type
    }

    /// Swaps the silicon variant, re-applying its register write masks to
    /// what is already stored (a real chip never held bits it can't store).
    pub fn set_crtc_type(&mut self, crtc_type: CrtcType) {
        self.crtc_type = crtc_type;
        for register in 0..16 {
            self.registers[register] &= crtc_type.write_mask(register);
        }
    }

    /// Latches the register number (5 bits significant, ACCC §21.2.1).
    pub fn select_register(&mut self, index: u8) {
        self.selected_register = index & 0x1F;
    }

    pub fn write_register(&mut self, val: u8) {
        let register = usize::from(self.selected_register);
        if register >= 16 {
            return;
        }
        let value = val & self.crtc_type.write_mask(register);
        self.registers[register] = value;
        // The R6 comparator is continuous: writing the value `C4` currently
        // holds turns the vertical display off at once (it stays off until
        // the next frame start even if R6 is changed again).
        if register == 6 && self.vertical_counter == value {
            self.vertical_display = false;
        }
    }

    /// Register read (CPC port `&BFxx`, function 3). What comes back
    /// depends on the type: ACCC §21.2 / §28.1.9.
    pub fn read_register(&self) -> u8 {
        let selected = self.selected_register;
        let reg = |index: usize| self.registers[index];
        match self.crtc_type {
            // "R12-R17 are the readable ones, R12, R14 and R16 six bits wide."
            CrtcType::Type0 => match selected {
                12 | 14 | 16 => reg(usize::from(selected)) & 0x3F,
                13 | 15 | 17 => reg(usize::from(selected)),
                _ => 0,
            },
            // "returns 0 on all registers, except for R14-R17 and R31."
            CrtcType::Type1 => match selected {
                14 | 16 => reg(usize::from(selected)) & 0x3F,
                15 | 17 => reg(usize::from(selected)),
                31 => 0xFF,
                _ => 0,
            },
            // "used to read registers R16 and R17. 0 on all other numbers."
            CrtcType::Type2 => match selected {
                16 => reg(16) & 0x3F,
                17 => reg(17),
                _ => 0,
            },
            CrtcType::Type3 | CrtcType::Type4 => self.asic_read(selected & 7),
        }
    }

    /// `read_register` as the CPU port sees it: on a UM6845R reading R16 or R17 clears the status
    /// register's light-pen bit.
    pub fn read_register_clearing(&mut self) -> u8 {
        let value = self.read_register();
        if self.crtc_type == CrtcType::Type1 && matches!(self.selected_register, 16 | 17) {
            self.lpen_read = true;
        }
        value
    }

    /// Status-register read (CPC port `&BExx`, function 2). Only the
    /// UM6845R has one; types 0 and 2 leave the bus floating high, and the
    /// ASIC types mirror the register-read port (ACCC §21.3).
    pub fn read_status(&self) -> u8 {
        match self.crtc_type {
            // Real type-0 chips "randomly return 255 or 127"; the floating
            // high value is used here.
            CrtcType::Type0 | CrtcType::Type2 => 0xFF,
            // Bit 6: the light-pen register is "full" from power-on until R16/R17 is read (no light pen
            // here to strobe it again). Bit 5: the BORDER-R6 latch.
            CrtcType::Type1 => {
                (if self.border_r6_status { 0x20 } else { 0x00 })
                    | (if self.lpen_read { 0x00 } else { 0x40 })
            }
            CrtcType::Type3 | CrtcType::Type4 => self.asic_read(self.selected_register & 7),
        }
    }

    /// ACCC §21.2.3: on types 3 and 4 only the 3 low bits of the selected
    /// register number are considered when reading.
    fn asic_read(&self, index: u8) -> u8 {
        match index {
            0 => self.registers[16] & 0x3F,
            1 => self.registers[17],
            2 => self.asic_status1(),
            3 => self.asic_status2(),
            4 => self.registers[12] & 0x3F,
            5 => self.registers[13],
            6 => self.registers[14] & 0x3F,
            _ => self.registers[15],
        }
    }

    /// One status bit of the ASIC CRTCs: `value_on_event` is the value the
    /// bit takes *while its event holds* (most are active low), the
    /// opposite otherwise (ACCC §21.3.4 tables).
    fn status_bit(bit: u8, event: bool, value_on_event: u8) -> u8 {
        (if event {
            value_on_event
        } else {
            value_on_event ^ 1
        } & 1)
            << bit
    }

    fn asic_status1(&self) -> u8 {
        let c0 = self.horizontal_counter;
        let r0 = self.registers[0];
        let r1 = self.registers[1];
        let r2 = self.registers[2];
        let mut status = 0x40;
        status |= Self::status_bit(0, c0 == r0, 1);
        status |= Self::status_bit(1, c0 == r0 >> 1, 0);
        status |= Self::status_bit(2, r0 >= r1 && c0 == r1.wrapping_sub(1), 0);
        status |= Self::status_bit(3, c0 == r2, 0);
        status |= Self::status_bit(4, c0 == r2.wrapping_add(self.hsync_width_or_16()), 0);
        let vsync_width = self.registers[3] >> 4;
        if vsync_width > 0 {
            status |= Self::status_bit(5, self.vsync && self.vsync_counter == vsync_width - 1, 0);
        } else {
            status |= Self::status_bit(5, self.vsync && self.vsync_counter < 15, 1);
        }
        // Bit 7: "on the next CRTC character, the less significant byte of the
        // video pointer will be reset to 0".
        let lsb_wraps = if c0 == r0 {
            self.next_row_address & 0xFF == 0
        } else {
            (self.row_address.wrapping_add(u16::from(c0)).wrapping_add(1)) & 0xFF == 0
        };
        status |= Self::status_bit(7, lsb_wraps, 0);
        status
    }

    fn asic_status2(&self) -> u8 {
        let c0 = self.horizontal_counter;
        let last_raster = self.raster_counter == self.maximum_raster();
        let end_of_line = c0 == self.registers[0];
        let c4 = self.vertical_counter;
        let mut status = 0x10;
        status |= Self::status_bit(
            0,
            c4 == self.vertical_total() && last_raster && end_of_line,
            0,
        );
        status |= Self::status_bit(
            1,
            c4 == self.registers[6].wrapping_sub(1) & 0x7F && last_raster && end_of_line,
            0,
        );
        status |= Self::status_bit(
            2,
            c4 == self.registers[7].wrapping_sub(1) & 0x7F && last_raster && end_of_line,
            0,
        );
        if (self.frame_counter >> 4) & 1 != 0 {
            status |= 0x08;
        }
        status |= Self::status_bit(5, last_raster, 0);
        status |= Self::status_bit(
            7,
            (last_raster && end_of_line) || (self.raster_counter == 0 && c0 < self.registers[0]),
            1,
        );
        status
    }

    pub(crate) fn snapshot_state(&self) -> (u8, [u8; REGISTER_COUNT]) {
        (self.selected_register, self.registers)
    }

    pub(crate) fn restore_snapshot_state(
        &mut self,
        selected_register: u8,
        registers: [u8; REGISTER_COUNT],
    ) {
        self.selected_register = selected_register & 0x1F;
        self.registers = registers;
        for register in 0..REGISTER_COUNT {
            self.registers[register] &= self.crtc_type.write_mask(register);
        }
    }

    /// Advances the CRTC by real CPU T-states without observing the
    /// characters -- the events-only form the unit tests use.
    #[cfg(test)]
    pub(crate) fn tick(&mut self, tstates: u32) -> CrtcEvents {
        self.tick_with(tstates, |_| {})
    }

    /// Advances the CRTC by real CPU T-states, reporting what it presented
    /// on every character clock that elapsed. The sub-character remainder is
    /// retained so instruction boundaries cannot phase-shift the 1 MHz
    /// character clock.
    pub(crate) fn tick_with<F: FnMut(&CharClock)>(
        &mut self,
        tstates: u32,
        mut on_char: F,
    ) -> CrtcEvents {
        let total = u32::from(self.tstate_remainder) + tstates;
        self.tstate_remainder = (total % TSTATES_PER_CHARACTER) as u8;

        let mut events = CrtcEvents::default();
        for _ in 0..total / TSTATES_PER_CHARACTER {
            self.tick_character(&mut events, &mut on_char);
        }
        events
    }

    pub(crate) fn vsync(&self) -> bool {
        self.vsync
    }

    /// Real CRTC character-line counter, needed by the CPC Plus ASIC's
    /// Programmable Raster Interrupt compare (see `crate::asic`'s own
    /// doc comment) -- exposed read-only since only that one real
    /// consumer needs it from outside this module.
    pub(crate) fn vertical_counter(&self) -> u8 {
        self.vertical_counter
    }

    /// Real CRTC raster-row-within-character counter, same ASIC PRI use
    /// as `vertical_counter`.
    pub(crate) fn raster_counter(&self) -> u8 {
        self.raster_counter
    }

    #[cfg(test)]
    fn raster_position(&self) -> (u8, u8, u8) {
        (
            self.horizontal_counter,
            self.vertical_counter,
            self.raster_counter,
        )
    }

    fn tick_character<F: FnMut(&CharClock)>(&mut self, events: &mut CrtcEvents, on_char: &mut F) {
        if self.hsync_remaining != 0 {
            self.hsync_remaining -= 1;
            if self.hsync_remaining == 0 {
                self.hsync = false;
                events.hsync_falls = events.hsync_falls.saturating_add(1);
            }
        }

        if !self.hsync && self.horizontal_counter == self.registers[2] {
            let width = self.hsync_width();
            if width != 0 {
                self.hsync = true;
                self.hsync_remaining = width;
            }
        }

        // `MA` is latched into VMA' when C0 reaches R1 on a row's last
        // raster; on the MC6845 the last line of the frame takes R12/R13
        // instead (ACCC §20.3.3).
        if self.horizontal_counter == self.registers[1]
            && self.raster_counter == self.maximum_raster()
        {
            self.next_row_address =
                if self.crtc_type == CrtcType::Type2 && self.is_last_frame_line() {
                    self.requested_start_address()
                } else {
                    self.row_address
                        .wrapping_add(u16::from(self.horizontal_counter))
                        & 0x3FFF
                };
        }

        on_char(&CharClock {
            ma: self
                .row_address
                .wrapping_add(u16::from(self.horizontal_counter))
                & 0x3FFF,
            ra: self.raster_counter,
            display: self.display_enabled(),
            hsync: self.hsync,
            vsync: self.vsync,
            c0: self.horizontal_counter,
            r0: self.registers[0],
            r2: self.registers[2],
            r1: self.registers[1],
            max_raster: self.maximum_raster(),
        });

        if self.horizontal_counter == self.registers[0] {
            self.horizontal_counter = 0;
            self.advance_scanline(events);
        } else {
            self.horizontal_counter = self.horizontal_counter.wrapping_add(1);
        }
    }

    /// `DISPEN` for the character about to be clocked out.
    fn display_enabled(&self) -> bool {
        if !self.vertical_display {
            return false;
        }
        // ACCC §19.2: the UM6845R gates the display on R6 != 0.
        if self.crtc_type == CrtcType::Type1 && self.registers[6] == 0 {
            return false;
        }
        let skew = self.display_skew();
        // Skew value 3 switches the display off (Caprice32 `update_skew`,
        // ACCC §19.2).
        if skew == 3 {
            return false;
        }
        let c0 = u16::from(self.horizontal_counter);
        let start = u16::from(skew);
        c0 >= start && c0 < start + u16::from(self.registers[1])
    }

    fn display_skew(&self) -> u8 {
        if self.crtc_type.has_display_skew() {
            (self.registers[8] >> 4) & 3
        } else {
            0
        }
    }

    /// `true` on the line whose end restarts the frame.
    fn is_last_frame_line(&self) -> bool {
        if self.in_vertical_adjust {
            self.vertical_adjust_counter + 1 >= self.vertical_total_adjust()
        } else {
            self.vertical_counter == self.vertical_total()
                && self.raster_counter == self.maximum_raster()
                && self.vertical_total_adjust() == 0
        }
    }

    /// Scanline boundary: raster/row/frame counters and the address
    /// generator. Start-address rules, by type:
    /// * every type: `VMA' -> VMA` at the end of a row's last raster, and
    ///   `VMA = VMA' = R12/R13` when a new frame starts (Caprice32
    ///   `reload_addr`/`restart_frame`);
    /// * UM6845R: R12/R13 are re-read at the start of *every* line while
    ///   `C4 == 0` (ACCC §13.3/§20.3: "the offset is considered as long as
    ///   C4=0, whatever the value of C9, unlike all other CRTC's");
    /// * MC6845: the frame start takes `VMA'`, which the previous frame's
    ///   last line loaded from R12/R13 -- "it is impossible to change offset
    ///   if C0 does not reach R1" (ACCC §13.4).
    fn advance_scanline(&mut self, events: &mut CrtcEvents) {
        self.frame_line = self.frame_line.saturating_add(1);
        if self.vsync {
            self.vsync_counter = self.vsync_counter.saturating_add(1);
            if self.vsync_counter >= self.vsync_width() {
                self.vsync = false;
                self.vsync_counter = 0;
            }
        }

        if self.in_vertical_adjust {
            self.raster_counter = self.raster_counter.wrapping_add(1) & 0x1F;
            self.vertical_adjust_counter += 1;
            if self.vertical_adjust_counter >= self.vertical_total_adjust() {
                self.restart_frame(events);
            }
        } else if self.raster_counter == self.maximum_raster() {
            self.raster_counter = 0;
            self.row_address = self.next_row_address;
            if self.vertical_counter == self.vertical_total() {
                let adjust = self.vertical_total_adjust();
                if adjust == 0 {
                    self.restart_frame(events);
                } else {
                    self.in_vertical_adjust = true;
                    self.vertical_adjust_counter = 0;
                }
            } else {
                self.vertical_counter = self.vertical_counter.wrapping_add(1) & 0x7F;
                if self.vertical_counter == self.registers[6] {
                    self.vertical_display = false;
                }
            }
        } else {
            self.raster_counter = self.raster_counter.wrapping_add(1) & 0x1F;
        }

        if self.crtc_type == CrtcType::Type1
            && self.vertical_counter == 0
            && !self.in_vertical_adjust
        {
            self.row_address = self.requested_start_address();
            self.next_row_address = self.row_address;
        }

        // CPC+ screen split: from the programmed line on, rows continue
        // from the split address instead.
        if let Some((line, address)) = self.split
            && self.frame_line == u16::from(line)
        {
            self.row_address = address;
            self.next_row_address = address;
        }

        // ACCC §21.3.1: bit 5 of the UM6845R status register is updated as
        // C0 returns to 0 -- false when C4=C9=0, true when C4=R6 and C9=0.
        if self.raster_counter == 0 {
            if self.vertical_counter == 0 {
                self.border_r6_status = false;
            } else if self.vertical_counter == self.registers[6] {
                self.border_r6_status = true;
            }
        }

        self.start_vsync_if_due(events);
    }

    fn restart_frame(&mut self, events: &mut CrtcEvents) {
        self.vertical_counter = 0;
        self.raster_counter = 0;
        self.vertical_adjust_counter = 0;
        self.in_vertical_adjust = false;
        self.frame_counter = self.frame_counter.wrapping_add(1);
        self.frame_line = 0;
        let start = if self.crtc_type == CrtcType::Type2 {
            self.next_row_address
        } else {
            self.requested_start_address()
        };
        self.row_address = start;
        self.next_row_address = start;
        // Vertical display comes back on with the frame -- unless R6 is 0,
        // where C4 already equals it.
        self.vertical_display = self.registers[6] != 0;
        events.frame_starts = events.frame_starts.saturating_add(1);
    }

    fn start_vsync_if_due(&mut self, events: &mut CrtcEvents) {
        if !self.vsync
            && !self.in_vertical_adjust
            && self.raster_counter == 0
            && self.vertical_counter == self.vertical_sync_position()
        {
            self.vsync = true;
            self.vsync_counter = 0;
            events.vsync_rises = events.vsync_rises.saturating_add(1);
        }
    }

    /// HSYNC length in characters, `0` meaning "no HSYNC at all".
    fn hsync_width(&self) -> u8 {
        let width = self.registers[3] & 0x0F;
        if width != 0 {
            width
        } else if self.crtc_type.zero_hsync_width_means_none() {
            0
        } else {
            16
        }
    }

    fn hsync_width_or_16(&self) -> u8 {
        let width = self.registers[3] & 0x0F;
        if width == 0 { 16 } else { width }
    }

    /// VSYNC length in scanlines (ACCC §14.2).
    fn vsync_width(&self) -> u8 {
        if !self.crtc_type.programmable_vsync_width() {
            return 16;
        }
        match self.registers[3] >> 4 {
            0 => 16,
            width => width,
        }
    }

    fn vertical_total(&self) -> u8 {
        self.registers[4] & 0x7F
    }

    fn vertical_total_adjust(&self) -> u8 {
        self.registers[5] & 0x1F
    }

    fn vertical_sync_position(&self) -> u8 {
        self.registers[7] & 0x7F
    }

    fn maximum_raster(&self) -> u8 {
        self.registers[9] & 0x1F
    }

    /// CPC+ screen split (`SPLT`/`SSA` registers): from scanline `line` of
    /// each frame the row address restarts from `address` (14-bit `MA`, the
    /// same format as R12/R13). `line == 0` means no split.
    pub(crate) fn set_split(&mut self, line: u8, address: u16) {
        self.split = (line != 0).then_some((line, address & 0x3FFF));
    }

    /// Loads `VMA`/`VMA'` from R12/R13 as a frame start would, without
    /// touching any counter. A power-on CRTC only does this at its first
    /// frame restart; a caller that wants a frame drawn from "the current
    /// registers" right now (see the synthetic frame in `crate::raster`)
    /// asks for it explicitly.
    pub(crate) fn reload_start_address(&mut self) {
        let start = self.requested_start_address();
        self.row_address = start;
        self.next_row_address = start;
    }

    /// `MA` value R12/R13 request as the screen start.
    fn requested_start_address(&self) -> u16 {
        (u16::from(self.registers[12] & 0x3F) << 8) | u16::from(self.registers[13])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(crtc: &mut Crtc, register: u8, value: u8) {
        crtc.select_register(register);
        crtc.write_register(value);
    }

    /// Programs the standard firmware layout (R0=63 ... R9=7) on `crtc`.
    fn firmware_layout(crtc: &mut Crtc) {
        for (register, value) in [
            (0, 63),
            (1, 40),
            (2, 46),
            (3, 0x8E),
            (4, 38),
            (5, 0),
            (6, 25),
            (7, 30),
            (9, 7),
            (12, 0x30),
            (13, 0x00),
        ] {
            write(crtc, register, value);
        }
    }

    /// Collects every character clock over `characters` clocks.
    fn collect(crtc: &mut Crtc, characters: u32) -> Vec<CharClock> {
        let mut clocks = Vec::new();
        crtc.tick_with(characters * 4, |clock| clocks.push(*clock));
        clocks
    }

    #[test]
    fn horizontal_total_and_sync_width_drive_hsync_falling_edges() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 0, 3); // four character clocks per line
        write(&mut crtc, 2, 1); // HSYNC begins at character 1
        write(&mut crtc, 3, 2); // two-character HSYNC width

        assert_eq!(crtc.tick(12).hsync_falls, 0);
        assert_eq!(crtc.tick(4).hsync_falls, 1);
        assert_eq!(crtc.raster_position().0, 0);
    }

    #[test]
    fn vertical_registers_drive_raster_vsync_and_frame_length() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 0, 0); // one character clock per scanline
        write(&mut crtc, 3, 0x20); // VSYNC width two scanlines
        write(&mut crtc, 4, 1); // two character rows
        write(&mut crtc, 5, 2); // two vertical-adjust scanlines
        write(&mut crtc, 7, 1); // VSYNC at character row 1
        write(&mut crtc, 9, 1); // two rasters per character row

        let mut events = CrtcEvents::default();
        for _ in 0..6 {
            let next = crtc.tick(4);
            events.vsync_rises += next.vsync_rises;
            events.frame_starts += next.frame_starts;
        }

        assert_eq!(events.vsync_rises, 1);
        assert_eq!(events.frame_starts, 1);
        assert_eq!(crtc.raster_position(), (0, 0, 0));
        assert!(!crtc.vsync());
    }

    fn partial_cpu_steps_preserve_character_clock_phase() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 0, 1);
        crtc.tick(3);
        assert_eq!(crtc.raster_position(), (0, 0, 0));
        crtc.tick(1);
        assert_eq!(crtc.raster_position(), (1, 0, 0));
    }

    // ---- type profiles -------------------------------------------------

    fn type_indexes_and_names_round_trip() {
        for (index, ty) in CrtcType::ALL.into_iter().enumerate() {
            assert_eq!(usize::from(ty.index()), index);
            assert_eq!(CrtcType::from_index(index as u8), Some(ty));
            assert!(!ty.chip_name().is_empty());
        }
        assert_eq!(CrtcType::from_index(5), None);
    }

    fn write_masks_drop_the_bits_each_type_cannot_store() {
        // R3's high nibble (VSYNC width) and R8's skew field exist on
        // types 0, 3 and 4 only (ACCC §4.3).
        for (ty, r3, r8) in [
            (CrtcType::Type0, 0xFE, 0xF3),
            (CrtcType::Type1, 0x0E, 0x03),
            (CrtcType::Type2, 0x0E, 0x03),
            (CrtcType::Type3, 0xFE, 0xF3),
            (CrtcType::Type4, 0xFE, 0xF3),
        ] {
            let mut crtc = Crtc::with_type(ty);
            write(&mut crtc, 3, 0xFE);
            write(&mut crtc, 8, 0xFF);
            assert_eq!(crtc.registers[3], r3, "{ty:?} R3");
            assert_eq!(crtc.registers[8], r8, "{ty:?} R8");
        }
        let mut crtc = Crtc::new();
        write(&mut crtc, 12, 0xFF);
        assert_eq!(crtc.registers[12], 0x3F);
        write(&mut crtc, 4, 0xFF);
        assert_eq!(crtc.registers[4], 0x7F);
        write(&mut crtc, 5, 0xFF);
        assert_eq!(crtc.registers[5], 0x1F);
    }

    fn switching_type_reapplies_the_new_masks_to_stored_registers() {
        let mut crtc = Crtc::with_type(CrtcType::Type0);
        write(&mut crtc, 3, 0x8E);
        crtc.set_crtc_type(CrtcType::Type1);
        assert_eq!(crtc.registers[3], 0x0E);
    }

    fn read_only_light_pen_and_out_of_range_registers_ignore_writes() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 16, 0x55);
        write(&mut crtc, 17, 0x55);
        write(&mut crtc, 20, 0x55);
        assert_eq!(&crtc.registers[16..], &[0, 0]);
    }
    /// Merged (every check kept): `type_indexes_and_names_round_trip`, `write_masks_drop_the_bits_each_type_cannot_store`, `switching_type_reapplies_the_new_masks_to_stored_registers`, `read_only_light_pen_and_out_of_range_registers_ignore_writes`.
    #[test]
    fn crtc_types_and_masks() {
        type_indexes_and_names_round_trip();
        write_masks_drop_the_bits_each_type_cannot_store();
        switching_type_reapplies_the_new_masks_to_stored_registers();
        read_only_light_pen_and_out_of_range_registers_ignore_writes();
    }

    fn load_readback_values(crtc: &mut Crtc) {
        for register in 12..16u8 {
            write(crtc, register, 0xF0 | register);
        }
        crtc.registers[16] = 0xF1;
        crtc.registers[17] = 0xF2;
    }

    fn read_back(crtc: &mut Crtc, register: u8) -> u8 {
        crtc.select_register(register);
        crtc.read_register()
    }

    fn type0_reads_back_r12_to_r17_only() {
        let mut crtc = Crtc::with_type(CrtcType::Type0);
        load_readback_values(&mut crtc);
        assert_eq!(read_back(&mut crtc, 12), 0x3C); // 0xFC & 0x3F
        assert_eq!(read_back(&mut crtc, 13), 0xFD);
        assert_eq!(read_back(&mut crtc, 14), 0x3E);
        assert_eq!(read_back(&mut crtc, 15), 0xFF);
        assert_eq!(read_back(&mut crtc, 16), 0x31);
        assert_eq!(read_back(&mut crtc, 17), 0xF2);
        assert_eq!(read_back(&mut crtc, 0), 0);
        assert_eq!(read_back(&mut crtc, 11), 0);
        // Only the 5 low bits of the register number count.
        crtc.select_register(0x20 | 12);
        assert_eq!(crtc.read_register(), 0x3C);
    }

    fn type1_reads_back_r14_to_r17_and_r31() {
        let mut crtc = Crtc::with_type(CrtcType::Type1);
        load_readback_values(&mut crtc);
        assert_eq!(read_back(&mut crtc, 12), 0, "R12 unreadable on the UM6845R");
        assert_eq!(read_back(&mut crtc, 13), 0);
        assert_eq!(read_back(&mut crtc, 14), 0x3E);
        assert_eq!(read_back(&mut crtc, 15), 0xFF);
        assert_eq!(read_back(&mut crtc, 16), 0x31);
        assert_eq!(read_back(&mut crtc, 17), 0xF2);
        assert_eq!(read_back(&mut crtc, 31), 0xFF);
    }

    fn type2_reads_back_only_the_light_pen_registers() {
        let mut crtc = Crtc::with_type(CrtcType::Type2);
        load_readback_values(&mut crtc);
        for register in 12..16 {
            assert_eq!(read_back(&mut crtc, register), 0, "R{register}");
        }
        assert_eq!(read_back(&mut crtc, 16), 0x31);
        assert_eq!(read_back(&mut crtc, 17), 0xF2);
    }

    fn asic_types_read_through_a_three_bit_register_window() {
        for ty in [CrtcType::Type3, CrtcType::Type4] {
            let mut crtc = Crtc::with_type(ty);
            load_readback_values(&mut crtc);
            // 0:R16 1:R17 4:R12 5:R13 6:R14 7:R15, and numbers wrap at 8.
            assert_eq!(read_back(&mut crtc, 0), 0x31, "{ty:?}");
            assert_eq!(read_back(&mut crtc, 1), 0xF2);
            assert_eq!(read_back(&mut crtc, 4), 0x3C);
            assert_eq!(read_back(&mut crtc, 5), 0xFD);
            assert_eq!(read_back(&mut crtc, 6), 0x3E);
            assert_eq!(read_back(&mut crtc, 7), 0xFF);
            assert_eq!(read_back(&mut crtc, 12), 0x3C, "R12 also answers as 4");
            assert_eq!(read_back(&mut crtc, 20), 0x3C, "R20 also answers as 4");
            // The status port mirrors the read port.
            crtc.select_register(5);
            assert_eq!(crtc.read_status(), crtc.read_register());
        }
    }
    /// Merged (every check kept): `type0_reads_back_r12_to_r17_only`, `type1_reads_back_r14_to_r17_and_r31`, `type2_reads_back_only_the_light_pen_registers`, `asic_types_read_through_a_three_bit_register_window`.
    #[test]
    fn crtc_readback() {
        type0_reads_back_r12_to_r17_only();
        type1_reads_back_r14_to_r17_and_r31();
        type2_reads_back_only_the_light_pen_registers();
        asic_types_read_through_a_three_bit_register_window();
    }

    fn only_the_um6845r_has_a_status_register() {
        for ty in [CrtcType::Type0, CrtcType::Type2] {
            assert_eq!(Crtc::with_type(ty).read_status(), 0xFF, "{ty:?}");
        }
        assert_eq!(Crtc::with_type(CrtcType::Type1).read_status() & 0x9F, 0);
    }

    fn um6845r_status_bit5_latches_border_r6_once_per_frame_line_zero() {
        let mut crtc = Crtc::with_type(CrtcType::Type1);
        firmware_layout(&mut crtc);
        // Run into the row where C4 == R6 (25): rasters 0 of row 25.
        let line_characters = 64;
        let mut saw_set_at = None;
        for line in 0..(26 * 8) {
            collect(&mut crtc, line_characters);
            if crtc.read_status() & 0x20 != 0 && saw_set_at.is_none() {
                saw_set_at = Some(line + 1);
            }
        }
        assert_eq!(saw_set_at, Some(25 * 8), "set when C4 reaches R6, raster 0");
        // ... and cleared again once the counters return to C4=C9=0.
        for _ in 0..(14 * 8 + 1) {
            collect(&mut crtc, line_characters);
        }
        assert_eq!(crtc.read_status() & 0x20, 0);
    }

    fn asic_status1_bit0_pulses_on_the_last_character_of_the_line() {
        let mut crtc = Crtc::with_type(CrtcType::Type3);
        firmware_layout(&mut crtc);
        crtc.select_register(2);
        collect(&mut crtc, 63); // C0 is now 63 == R0
        assert_eq!(crtc.read_register() & 0x01, 0x01);
        collect(&mut crtc, 1); // wrapped to C0 = 0
        assert_eq!(crtc.read_register() & 0x01, 0x00);
        assert_eq!(crtc.read_register() & 0x40, 0x40, "bit 6 always reads 1");
    }
    /// Merged (every check kept): `only_the_um6845r_has_a_status_register`, `um6845r_status_bit5_latches_border_r6_once_per_frame_line_zero`, `asic_status1_bit0_pulses_on_the_last_character_of_the_line`.
    #[test]
    fn crtc_status() {
        only_the_um6845r_has_a_status_register();
        um6845r_status_bit5_latches_border_r6_once_per_frame_line_zero();
        asic_status1_bit0_pulses_on_the_last_character_of_the_line();
    }

    // ---- sync widths ---------------------------------------------------

    fn zero_hsync_width_means_no_hsync_on_types_0_and_1_and_sixteen_elsewhere() {
        for (ty, expected_falls) in [
            (CrtcType::Type0, 0),
            (CrtcType::Type1, 0),
            (CrtcType::Type2, 1),
            (CrtcType::Type3, 1),
            (CrtcType::Type4, 1),
        ] {
            let mut crtc = Crtc::with_type(ty);
            write(&mut crtc, 0, 63);
            write(&mut crtc, 2, 10);
            write(&mut crtc, 3, 0x00);
            let falls = crtc.tick(64 * 4).hsync_falls;
            assert_eq!(falls, expected_falls, "{ty:?}");
        }
    }

    fn hsync_lasts_sixteen_characters_when_r3l_is_zero_on_type2() {
        let mut crtc = Crtc::with_type(CrtcType::Type2);
        write(&mut crtc, 0, 63);
        write(&mut crtc, 2, 10);
        let clocks = collect(&mut crtc, 64);
        let high = clocks.iter().filter(|clock| clock.hsync).count();
        assert_eq!(high, 16);
    }
    /// Merged (every check kept): `zero_hsync_width_means_no_hsync_on_types_0_and_1_and_sixteen_elsewhere`, `hsync_lasts_sixteen_characters_when_r3l_is_zero_on_type2`.
    #[test]
    fn hsync_width() {
        zero_hsync_width_means_no_hsync_on_types_0_and_1_and_sixteen_elsewhere();
        hsync_lasts_sixteen_characters_when_r3l_is_zero_on_type2();
    }

    fn vsync_lines(ty: CrtcType, r3: u8) -> u32 {
        let mut crtc = Crtc::with_type(ty);
        write(&mut crtc, 0, 0); // one character per line
        write(&mut crtc, 3, r3);
        write(&mut crtc, 4, 63); // 64 lines
        write(&mut crtc, 7, 5);
        write(&mut crtc, 9, 0);
        let mut lines = 0;
        for line in 0..64 {
            let mut vsync = false;
            crtc.tick_with(4, |clock| vsync = clock.vsync);
            if vsync {
                lines += 1;
            }
            let _ = line;
        }
        lines
    }

    #[test]
    fn vsync_width_is_programmable_only_on_types_0_3_and_4() {
        assert_eq!(vsync_lines(CrtcType::Type0, 0x30), 3);
        assert_eq!(vsync_lines(CrtcType::Type3, 0x30), 3);
        assert_eq!(vsync_lines(CrtcType::Type4, 0x30), 3);
        // Types 1 and 2 ignore R3's high nibble: always 16 lines.
        assert_eq!(vsync_lines(CrtcType::Type1, 0x30), 16);
        assert_eq!(vsync_lines(CrtcType::Type2, 0x30), 16);
        // A programmed 0 means 16 on the types that honour it.
        assert_eq!(vsync_lines(CrtcType::Type0, 0x00), 16);
    }

    // ---- address generation & display enable -----------------------------

    #[test]
    fn ram_address_matches_the_cpc_address_wiring() {
        // MA 0x3000 (R12=0x30) is the classic &C000 screen.
        assert_eq!(ram_address(0x3000, 0), 0xC000);
        // Each character is two bytes; RA0-2 select the 2KB interleave.
        assert_eq!(ram_address(0x3001, 0), 0xC002);
        assert_eq!(ram_address(0x3000, 1), 0xC800);
        assert_eq!(ram_address(0x3000, 7), 0xF800);
        // MA10/MA11 don't reach the address bus: 0x3400 wraps back.
        assert_eq!(ram_address(0x3400, 0), 0xC000);
        // MA12/MA13 select the 16K page.
        assert_eq!(ram_address(0x1000, 0), 0x4000);
        assert_eq!(ram_address(0x2000, 0), 0x8000);
        // The 10 low MA bits span 2K of line data.
        assert_eq!(ram_address(0x03FF, 0), 0x07FE);
    }

    #[test]
    fn a_frame_walks_ma_ra_and_row_addresses_like_a_6845() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 0, 3); // 4 characters per line
        write(&mut crtc, 1, 2); // 2 displayed
        write(&mut crtc, 4, 2); // 3 character rows
        write(&mut crtc, 6, 3);
        write(&mut crtc, 9, 1); // 2 rasters per row
        write(&mut crtc, 12, 0x10);
        write(&mut crtc, 13, 0x20);
        let start = 0x1020;
        crtc.reload_start_address();

        let clocks = collect(&mut crtc, 4 * 6); // exactly one frame
        // Row 0: both rasters restart at the row's first MA.
        assert_eq!((clocks[0].ma, clocks[0].ra), (start, 0));
        assert_eq!((clocks[3].ma, clocks[3].ra), (start + 3, 0));
        assert_eq!((clocks[4].ma, clocks[4].ra), (start, 1));
        // Row 1 starts R1 (2) characters on.
        assert_eq!((clocks[8].ma, clocks[8].ra), (start + 2, 0));
        assert_eq!((clocks[12].ma, clocks[12].ra), (start + 2, 1));
        // Row 2 starts another R1 on.
        assert_eq!((clocks[16].ma, clocks[16].ra), (start + 4, 0));
        // Next frame reloads R12/R13.
        let next = collect(&mut crtc, 1);
        assert_eq!((next[0].ma, next[0].ra), (start, 0));
    }

    #[test]
    fn display_enable_follows_r1_horizontally_and_r6_vertically() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 0, 5); // 6 characters per line
        write(&mut crtc, 1, 3);
        write(&mut crtc, 4, 3);
        write(&mut crtc, 6, 2); // rows 0-1 displayed
        write(&mut crtc, 9, 0); // one raster per row
        let clocks = collect(&mut crtc, 6 * 4);
        let row: Vec<Vec<bool>> = clocks
            .chunks(6)
            .map(|line| line.iter().map(|clock| clock.display).collect())
            .collect();
        let shown = vec![true, true, true, false, false, false];
        let blank = vec![false; 6];
        assert_eq!(row, vec![shown.clone(), shown, blank.clone(), blank]);
    }

    #[test]
    fn display_skew_delays_the_window_on_types_that_have_it() {
        for (ty, skew, expected) in [
            (CrtcType::Type0, 1, [false, true, true, true, false, false]),
            (CrtcType::Type0, 2, [false, false, true, true, true, false]),
            (CrtcType::Type0, 3, [false; 6]),
            (CrtcType::Type3, 1, [false, true, true, true, false, false]),
            // The UM6845R/MC6845 have no skew field: R8 bits 5-4 are ignored.
            (CrtcType::Type1, 1, [true, true, true, false, false, false]),
            (CrtcType::Type2, 2, [true, true, true, false, false, false]),
        ] {
            let mut crtc = Crtc::with_type(ty);
            write(&mut crtc, 0, 5);
            write(&mut crtc, 1, 3);
            write(&mut crtc, 4, 3);
            write(&mut crtc, 6, 2);
            write(&mut crtc, 8, skew << 4);
            let clocks = collect(&mut crtc, 6);
            let line: Vec<bool> = clocks.iter().map(|clock| clock.display).collect();
            assert_eq!(line, expected.to_vec(), "{ty:?} skew {skew}");
        }
    }

    #[test]
    fn um6845r_treats_r6_of_zero_as_no_display_and_others_show_nothing_via_the_counter() {
        let mut crtc = Crtc::with_type(CrtcType::Type1);
        write(&mut crtc, 0, 3);
        write(&mut crtc, 1, 2);
        write(&mut crtc, 4, 3);
        write(&mut crtc, 6, 0);
        assert!(collect(&mut crtc, 8).iter().all(|clock| !clock.display));
    }

    #[test]
    fn writing_r6_equal_to_the_row_counter_turns_display_off_immediately() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 0, 3);
        write(&mut crtc, 1, 4);
        write(&mut crtc, 4, 10);
        write(&mut crtc, 6, 8);
        write(&mut crtc, 9, 0);
        collect(&mut crtc, 4 * 2); // two rows in: C4 == 2
        assert!(collect(&mut crtc, 1)[0].display);
        write(&mut crtc, 6, 2);
        assert!(!collect(&mut crtc, 1)[0].display);
    }

    #[test]
    fn a_new_start_address_only_applies_at_the_next_frame_start_on_type0() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 0, 1);
        write(&mut crtc, 1, 1);
        write(&mut crtc, 4, 1); // 2 rows
        write(&mut crtc, 9, 1); // 2 rasters/row -> 4 lines per frame
        write(&mut crtc, 12, 0x10);
        write(&mut crtc, 13, 0x00);
        collect(&mut crtc, 2); // line 0 done, C4=C9... raster 1 next
        write(&mut crtc, 13, 0x40);
        let rest = collect(&mut crtc, 6); // remaining 3 lines of this frame
        assert!(
            rest.iter().all(|clock| clock.ma < 0x1040),
            "old base until the frame ends"
        );
        let next_frame = collect(&mut crtc, 1);
        assert_eq!(next_frame[0].ma, 0x1040);
    }

    #[test]
    fn um6845r_rereads_the_start_address_every_line_while_c4_is_zero() {
        let mut crtc = Crtc::with_type(CrtcType::Type1);
        write(&mut crtc, 0, 1);
        write(&mut crtc, 1, 1);
        write(&mut crtc, 4, 1);
        write(&mut crtc, 9, 3); // 4 rasters/row
        write(&mut crtc, 12, 0x10);
        write(&mut crtc, 13, 0x00);
        crtc.reload_start_address();
        let first = collect(&mut crtc, 2);
        assert_eq!(first[0].ma, 0x1000);
        // Raster 1 had already loaded its address when the line started.
        write(&mut crtc, 13, 0x08);
        let second = collect(&mut crtc, 2);
        assert_eq!(second[0].ma, 0x1000);
        // Raster 2 re-reads R12/R13 -- a type-0 chip would keep 0x1000.
        let third = collect(&mut crtc, 2);
        assert_eq!(
            third[0].ma, 0x1008,
            "R12/R13 re-read on the next line start"
        );
        // Once C4 > 0 it no longer follows R12/R13.
        collect(&mut crtc, 2); // raster 3 (last of row 0)
        write(&mut crtc, 13, 0x50);
        let row1 = collect(&mut crtc, 2);
        assert_eq!(
            row1[0].ma,
            0x1008 + 1,
            "row 1 starts R1 past the latched row"
        );
    }

    #[test]
    fn a_plus_screen_split_restarts_the_row_address_at_the_programmed_line() {
        let mut crtc = Crtc::new();
        write(&mut crtc, 0, 1);
        write(&mut crtc, 1, 1);
        write(&mut crtc, 4, 3); // 4 rows
        write(&mut crtc, 9, 1); // 2 rasters/row -> 8 lines per frame
        write(&mut crtc, 12, 0x10);
        write(&mut crtc, 13, 0x00);
        crtc.reload_start_address();
        crtc.set_split(4, 0x2345); // from line 4 (row 2, raster 0)
        let clocks = collect(&mut crtc, 2 * 8);
        let row_starts: Vec<u16> = clocks.iter().step_by(2).map(|clock| clock.ma).collect();
        // Lines 0-3: rows 0 and 1 from the frame start (0x1000, +R1 = 0x1001).
        assert_eq!(&row_starts[..4], &[0x1000, 0x1000, 0x1001, 0x1001]);
        // From line 4 the split address takes over, and later rows advance
        // from it.
        assert_eq!(&row_starts[4..], &[0x2345, 0x2345, 0x2346, 0x2346]);
        // The next frame starts from R12/R13 again.
        assert_eq!(collect(&mut crtc, 1)[0].ma, 0x1000);
        // A split line of 0 disables it.
        crtc.set_split(0, 0x2345);
        let clocks = collect(&mut crtc, 2 * 8 - 1);
        assert!(clocks.iter().all(|clock| clock.ma < 0x2000));
    }

    fn events_from_split_ticks_merge() {
        let mut total = CrtcEvents::default();
        total.merge(CrtcEvents {
            hsync_falls: 1,
            vsync_rises: 0,
            frame_starts: 2,
        });
        total.merge(CrtcEvents {
            hsync_falls: 2,
            vsync_rises: 1,
            frame_starts: 250,
        });
        assert_eq!(
            total,
            CrtcEvents {
                hsync_falls: 3,
                vsync_rises: 1,
                frame_starts: 252,
            }
        );
    }
    /// Merged (every check kept): `events_from_split_ticks_merge`, `partial_cpu_steps_preserve_character_clock_phase`.
    #[test]
    fn split_ticks() {
        events_from_split_ticks_merge();
        partial_cpu_steps_preserve_character_clock_phase();
    }

    #[test]
    fn mc6845_takes_the_frame_start_address_from_vma_prime() {
        let mut crtc = Crtc::with_type(CrtcType::Type2);
        write(&mut crtc, 0, 1);
        write(&mut crtc, 1, 1);
        write(&mut crtc, 4, 0); // 1 row
        write(&mut crtc, 9, 1); // 2 lines per frame
        write(&mut crtc, 12, 0x10);
        write(&mut crtc, 13, 0x00);
        collect(&mut crtc, 4); // frame 0 (VMA' loaded at C0=R1 on its last line)
        write(&mut crtc, 13, 0x30);
        // The write landed after frame 0's last-line C0=R1 point... the
        // *next* frame still starts from the VMA' latched during frame 0.
        let frame1 = collect(&mut crtc, 2);
        assert_eq!(frame1[0].ma, 0x1000);
        // ...and only the frame after that sees the new value.
        collect(&mut crtc, 2);
        let frame2 = collect(&mut crtc, 2);
        assert_eq!(frame2[0].ma, 0x1030);
    }
}
