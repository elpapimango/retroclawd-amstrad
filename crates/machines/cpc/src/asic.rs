//! Amstrad CPC Plus/GX4000 ASIC: the custom chip that folds the CRTC/Gate
//! Array/PPI's classic functions together with real *extra* hardware --
//! a 4096-colour (12-bit RGB) palette, 16 hardware sprites, screen
//! splitting, per-pixel horizontal + per-raster-line vertical soft
//! scrolling, a Programmable Raster Interrupt (PRI), vectored Z80 IM2
//! interrupts, and 3 DMA sound channels -- gated behind a real 15-byte
//! unlock sequence. Every register offset, the unlock sequence itself,
//! and every quirk below are taken directly from CPCTech's own "Extra CPC
//! Plus Hardware Information" (hardware-tested against a real CPC6128+,
//! see `CREDITS.md`), cross-checked against MAME's own `amstrad_m.cpp`
//! ASIC implementation (a real, independently tested source -- also see
//! `CREDITS.md`) wherever the two needed reconciling (the unlock sequence
//! bytes and the DMA-channel-to-vector-cause mapping both came from MAME's
//! source, since CPCTech's own page documents the *mechanism* but not
//! every literal constant). Kept as its own module inside `cpc`, same
//! "single consumer" reasoning as `gate_array`.
//!
//! Picture-affecting ASIC state is part of this crate's per-character video
//! trace (`crate::raster`): palette writes and the soft-scroll register are
//! logged against the exact character clock they landed on, the screen split
//! overrides the CRTC's row address (`Crtc::set_split`), and vertical soft
//! scroll offsets the raster address the video bytes are fetched from. Only
//! the 16 hardware sprites are composited afterwards from live registers
//! (`crate::plus_video`).

/// 15-byte ASIC unlock sequence, written one byte per CRTC
/// select-register (`&BCxx`) write. A non-zero byte followed by a zero
/// resyncs the detector to the start of the sequence; the 15th byte
/// (`0xee`) only locks or unlocks depending on whether the previous 14
/// bytes already matched -- its own value is never compared. Byte values
/// from MAME's own `asic_unlock_seq` (see module doc comment).
const ASIC_UNLOCK_SEQ: [u8; 15] = [
    0xff, 0x77, 0xb3, 0x51, 0xa8, 0xd4, 0x62, 0x39, 0x9c, 0x46, 0x2b, 0x15, 0x8a, 0xcd, 0xee,
];

/// Interrupt-vector low bits per cause: raster = 6, DMA channel `N`
/// (0-2) = `N*2`. Matches MAME's own tested assignment (verified against
/// real Pang, a real game that depends on IM2 vectoring) rather than
/// CPCTech's own less-certain table (that page's author frames it as "I
/// found the following are correct", a lower epistemic tier than a
/// tested, working real game).
const RASTER_IRQ_CAUSE: u8 = 6;

/// 16KB memory-mapped ASIC register window, mirroring `0x4000-0x7FFF`
/// (index 0 = address `0x4000`) once paged in.
const ASIC_RAM_SIZE: usize = 0x4000;

/// One entry of the 32-entry hardware palette table (screen pens 0-15 at
/// entries 0-15, border/soft-scroll-mask at entry 16, sprite colours 1-15
/// reusing entries 17-31 -- see `pen_color`/`sprite_color`'s own doc
/// comments), 2 bytes each.
const PALETTE_BASE: usize = 0x2400;
/// 16 sprites x 8 bytes of position/magnification info, per sprite:
/// X lo/hi, Y lo/hi, magnification (mirrored at the next 4 bytes, read
/// back only -- see `sprite`'s own doc comment).
const SPRITE_INFO_BASE: usize = 0x2000;
/// 16 sprites x 256 bytes (16x16 px, 1 byte/pixel, masked to a nibble on
/// write) of raw pixel data.
const SPRITE_PIXELS_BASE: usize = 0x0000;
const PRI_REGISTER: usize = 0x2800;
const SPLIT_LINE_REGISTER: usize = 0x2801;
const SPLIT_ADDR_HI_REGISTER: usize = 0x2802;
const SPLIT_ADDR_LO_REGISTER: usize = 0x2803;
const SOFT_SCROLL_REGISTER: usize = 0x2804;
const INTERRUPT_VECTOR_REGISTER: usize = 0x2805;
const DMA_ADDR_LO: [usize; 3] = [0x2C00, 0x2C04, 0x2C08];
const DMA_ADDR_HI: [usize; 3] = [0x2C01, 0x2C05, 0x2C09];
const DMA_PRESCALER: [usize; 3] = [0x2C02, 0x2C06, 0x2C0A];
const DCSR_REGISTER: usize = 0x2C0F;
/// Analogue joystick ADC0-3 (2 joysticks' X/Y), real ports `&6808-&680B`.
const ANALOG_BASE: usize = 0x2808;
/// ADC4-7: real, but unconnected on every known peripheral -- CPCTech's
/// own empirical table (no peripheral attached) reads a fixed
/// `0x3f, 0x00, 0x3f, 0x00` pattern here, matched verbatim by MAME's own
/// implementation (see module doc comment). Not user-controllable.
const ANALOG_UNUSED_IDLE: [u8; 4] = [0x3F, 0x00, 0x3F, 0x00];

/// One decoded 16x16 hardware sprite's position/size, in the ASIC's own
/// fixed dot-clock coordinate space (matching mode 2's 640-wide density
/// regardless of the current screen mode -- see `crate::plus_video`'s own
/// doc comment for why).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlusSprite {
    pub x: i16,
    pub y: i16,
    /// Effective pixel width/height (`16`, `32`, or `64` -- 1x/2x/4x
    /// magnification; `0` means the sprite is disabled, either axis'
    /// stored magnification nibble being `0`).
    pub width: u16,
    pub height: u16,
    /// `log2` of the magnification factor (`0`, `1`, or `2`), matching
    /// the shift amounts `sprite_pixel` needs to map a screen offset
    /// back to a source pixel.
    pub x_shift: u8,
    pub y_shift: u8,
}

/// A write to the ASIC register window that changes what the picture looks
/// like from that moment on (see `crate::raster`, which replays these
/// against the character the CRTC was clocking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoWrite {
    /// Screen pen `0..=15` or the border (`16`) now has this colour.
    Ink { pen: u8, rgb: (u8, u8, u8) },
    /// Horizontal soft-scroll offset (Mode-2 pixels).
    HScroll(u8),
}

/// One real DMA channel command's side effects, resolved by
/// `Asic::step_dma_channel` -- the caller (`Cpc::step`) performs the
/// actual AY-3-8910 register write / CPU interrupt request, since `Asic`
/// itself has no access to either.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DmaResult {
    pub psg_load: Option<(u8, u8)>,
    pub interrupt: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Asic {
    enabled: bool,
    seq_ptr: u8,
    prev_seq_byte: u8,
    /// Secondary ROM-mapping register -- only ever written when the Gate
    /// Array's function-2 write has bit 5 set (disambiguating an RMR2
    /// write from a classic MRER write at the very same I/O port).
    rmr2: u8,
    ram: Vec<u8>,
    /// Vector-byte low bits of whichever interrupt source fired most
    /// recently -- real hardware only tracks one at a time (see module
    /// doc comment).
    irq_cause: u8,
    /// Real hardware's own documented, intentional quirk: with DCSR bit 0
    /// left at its reset default (`1`, "clear disabled" -- CPCTech's own
    /// page: "my advice is that you should not use the automatic
    /// interrupt clear feature"), a DMA-caused interrupt's `/INT` line
    /// stays asserted until software explicitly acknowledges it by
    /// writing the matching bit back to DCSR (`0x2C0F`). `Cpc::step`
    /// re-requests the CPU interrupt every instruction while this is
    /// true, exactly matching a real held `/INT` line being re-sampled
    /// every instruction boundary (raster interrupts never set this --
    /// real hardware always auto-clears those, see `interrupt_vector_byte`).
    irq_unacknowledged: bool,
    dma_status: u8,
    dma_clear_disabled: bool,
    dma_addr: [u16; 3],
    dma_prescaler: [u8; 3],
    dma_pause: [u16; 3],
    dma_repeat: [u16; 3],
    dma_loopcount: [u16; 3],
    /// Digital-direction-driven ADC0-3 (2 analogue joysticks' X/Y) --
    /// `0x1F` (centre) is a deliberate engineering default (a real,
    /// usable "virtually attached, centred" joystick) rather than
    /// CPCTech's own measured "nothing attached" reading of `0x3F` --
    /// same epistemic tier as `zx_spectrum::SpeakerType`'s filter
    /// cutoffs: makes the feature actually usable, not a hardware
    /// measurement. `0x00`/`0x3F` are the pressed extremes.
    analog: [u8; 4],
}

impl Default for Asic {
    fn default() -> Self {
        Self {
            enabled: false,
            seq_ptr: 0,
            prev_seq_byte: 0,
            rmr2: 0,
            ram: vec![0; ASIC_RAM_SIZE],
            irq_cause: RASTER_IRQ_CAUSE,
            irq_unacknowledged: false,
            dma_status: 0,
            // Real reset default per CPCTech's own page and MAME's own
            // `MACHINE_RESET_MEMBER(plus/gx4000)`: DMA interrupts must be
            // cleared by writing to DCSR unless software opts out.
            dma_clear_disabled: true,
            dma_addr: [0; 3],
            dma_prescaler: [0; 3],
            dma_pause: [0; 3],
            dma_repeat: [0; 3],
            dma_loopcount: [0; 3],
            analog: [0x1F; 4],
        }
    }
}

impl Asic {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn rmr2(&self) -> u8 {
        self.rmr2
    }

    pub(crate) fn pri(&self) -> u8 {
        self.ram[PRI_REGISTER]
    }

    pub(crate) fn split_line(&self) -> u8 {
        self.ram[SPLIT_LINE_REGISTER]
    }

    pub(crate) fn split_address(&self) -> u16 {
        (u16::from(self.ram[SPLIT_ADDR_HI_REGISTER]) << 8)
            | u16::from(self.ram[SPLIT_ADDR_LO_REGISTER])
    }

    pub(crate) fn h_scroll(&self) -> u8 {
        self.ram[SOFT_SCROLL_REGISTER] & 0x0F
    }

    pub(crate) fn v_scroll(&self) -> u8 {
        (self.ram[SOFT_SCROLL_REGISTER] >> 4) & 0x07
    }

    fn decode_color(low: u8, high: u8) -> (u8, u8, u8) {
        // Real hardware bit layout (CPCTech's own page, see module doc
        // comment): low byte = R(upper nibble) | B(lower nibble), high
        // byte's lower nibble = G. Each 4-bit channel scales to 8-bit
        // exactly (`15 * 17 == 255`).
        let r = (low >> 4) & 0x0F;
        let b = low & 0x0F;
        let g = high & 0x0F;
        (r * 17, g * 17, b * 17)
    }

    /// Screen pen colour (`0..=15`) or border/soft-scroll-mask colour
    /// (`16`).
    pub(crate) fn pen_color(&self, pen: u8) -> (u8, u8, u8) {
        let idx = PALETTE_BASE + (pen.min(16) as usize) * 2;
        Self::decode_color(self.ram[idx], self.ram[idx + 1])
    }

    /// Sprite colour `1..=15` (`0` is always transparent and never
    /// queried). Sprite colours physically reuse palette entries 17-31 --
    /// real hardware fact, not a coincidence (see module doc comment).
    pub(crate) fn sprite_color(&self, index: u8) -> (u8, u8, u8) {
        let idx = PALETTE_BASE + 0x20 + (index as usize) * 2;
        Self::decode_color(self.ram[idx], self.ram[idx + 1])
    }

    pub(crate) fn sprite(&self, index: usize) -> PlusSprite {
        let base = SPRITE_INFO_BASE + index * 8;
        let x = i16::from_le_bytes([self.ram[base], self.ram[base + 1]]);
        let y = i16::from_le_bytes([self.ram[base + 2], self.ram[base + 3]]);
        let mag = self.ram[base + 4];
        let xmag = (mag >> 2) & 0x03;
        let ymag = mag & 0x03;
        if xmag == 0 || ymag == 0 {
            return PlusSprite {
                x,
                y,
                width: 0,
                height: 0,
                x_shift: 0,
                y_shift: 0,
            };
        }
        let x_shift = xmag - 1;
        let y_shift = ymag - 1;
        PlusSprite {
            x,
            y,
            width: 16u16 << x_shift,
            height: 16u16 << y_shift,
            x_shift,
            y_shift,
        }
    }

    /// One pixel of sprite `index`'s raw 16x16 image (palette index
    /// `0..=15`, `0` = transparent). `row`/`col` are source pixel
    /// coordinates in `0..16`, already descaled from screen space by the
    /// caller (`plus_video`) using the sprite's own `x_shift`/`y_shift`.
    pub(crate) fn sprite_pixel(&self, index: usize, row: usize, col: usize) -> u8 {
        self.ram[SPRITE_PIXELS_BASE + index * 256 + row * 16 + col] & 0x0F
    }

    /// Sets one of the 4 analogue-joystick axes from a digital direction.
    /// `positive`: pressing the "increasing" direction (right/down) drives
    /// the extreme high value; pressing "decreasing" (left/up) drives the
    /// extreme low value; releasing returns to centre.
    pub(crate) fn set_analog_axis(&mut self, axis: usize, value: u8) {
        if let Some(slot) = self.analog.get_mut(axis) {
            *slot = value;
        }
    }

    fn analog_read(&self, offset: usize) -> u8 {
        match offset {
            0..=3 => self.analog[offset] & 0x3F,
            4..=7 => ANALOG_UNUSED_IDLE[offset - 4],
            _ => 0xFF,
        }
    }

    /// Checks one byte written to the CRTC select-register port
    /// (`&BCxx`) against the unlock sequence. Called on every such write
    /// regardless of ASIC state -- unlocking and re-locking both go
    /// through this same detector. Sending only the first 14 bytes (no
    /// 15th) while already unlocked locks it (real mechanism -- the 15th
    /// byte, whatever its value, always re-enables; a matching resend of
    /// the *complete* sequence therefore locks only transiently before
    /// re-unlocking at the 15th byte, matching MAME's own tested logic,
    /// see module doc comment). Position resets to the start once a
    /// sequence completes (15 bytes matched) -- MAME's own raw C++ index
    /// keeps incrementing past the 15-entry table there (real, latent
    /// undefined behaviour in that source), so this is a deliberate,
    /// safe clarification: always ready to detect a fresh sequence
    /// immediately after the previous one finishes, rather than staying
    /// stuck until an incidental zero byte happens to resync it.
    pub(crate) fn check_unlock_sequence(&mut self, data: u8) {
        if data == 0 && self.prev_seq_byte != 0 {
            self.seq_ptr = 0;
        }
        let expected = ASIC_UNLOCK_SEQ.get(self.seq_ptr as usize).copied();
        if expected == Some(data) {
            self.seq_ptr += 1;
            if self.seq_ptr == 14 && self.enabled {
                self.enabled = false;
            }
            if self.seq_ptr as usize >= ASIC_UNLOCK_SEQ.len() {
                self.enabled = true;
                self.seq_ptr = 0;
            }
        } else if self.seq_ptr == 14 {
            // The final byte's own value is never compared -- any byte
            // here completes the sequence.
            self.seq_ptr = 0;
            self.enabled = true;
        }
        self.prev_seq_byte = data;
    }

    /// A Gate Array function-2 write with bit 5 set targets RMR2 instead
    /// of the classic MRER -- only takes effect once unlocked. Returns
    /// whether this write was consumed as an RMR2 write (the caller must
    /// not also apply it as a classic MRER write when this is `true`).
    pub(crate) fn maybe_write_rmr2(&mut self, val: u8) -> bool {
        if self.enabled && val & 0x20 != 0 {
            self.rmr2 = val;
            true
        } else {
            false
        }
    }

    /// One byte of the full `0x4000-0x7FFF` memory-mapped window,
    /// `offset` relative to `0x4000` (`0..0x4000`). Routes ADC reads
    /// through `analog_read`; everything else is plain register storage.
    pub(crate) fn read_window(&self, offset: usize) -> u8 {
        if (ANALOG_BASE..ANALOG_BASE + 8).contains(&offset) {
            return self.analog_read(offset - ANALOG_BASE);
        }
        self.ram[offset]
    }

    /// `0x4000-0x5FFF` write (sprite pixel data) -- real hardware ties
    /// the upper 4 data-bus bits low in this range.
    pub(crate) fn write_low(&mut self, offset: usize, val: u8) {
        self.ram[offset] = val & 0x0F;
    }

    /// `0x6000-0x7FFF` write (`offset` relative to `0x6000`) -- position/
    /// palette/misc/DMA registers.
    pub(crate) fn write_high(&mut self, offset: usize, val: u8) {
        let idx = offset + 0x2000;
        self.ram[idx] = val;
        // Green byte of each palette entry masks to a nibble on
        // readback, per CPCTech's own page.
        if (0x0400..0x0440).contains(&offset) && offset & 1 == 1 {
            self.ram[idx] = val & 0x0F;
        }
        match idx {
            INTERRUPT_VECTOR_REGISTER => self.dma_clear_disabled = val & 0x01 != 0,
            _ if DMA_ADDR_LO.contains(&idx) || DMA_ADDR_HI.contains(&idx) => {
                for channel in 0..3 {
                    if idx == DMA_ADDR_LO[channel] || idx == DMA_ADDR_HI[channel] {
                        self.dma_addr[channel] = u16::from(self.ram[DMA_ADDR_LO[channel]])
                            | (u16::from(self.ram[DMA_ADDR_HI[channel]]) << 8);
                        self.dma_status &= !(1 << channel);
                    }
                }
            }
            _ if DMA_PRESCALER.contains(&idx) => {
                for (channel, &reg) in DMA_PRESCALER.iter().enumerate() {
                    if idx == reg {
                        self.dma_prescaler[channel] = val.wrapping_add(1);
                    }
                }
            }
            DCSR_REGISTER => self.write_dcsr(val),
            _ => {}
        }
    }

    /// What the just-completed `write_high(offset, ..)` changed on screen,
    /// if anything: a screen-pen/border palette entry (sprite colours are
    /// read live at render time and aren't traced) or the soft-scroll
    /// register.
    pub(crate) fn video_effect_of_high_write(&self, offset: usize) -> Option<VideoWrite> {
        const PALETTE_OFFSET: usize = PALETTE_BASE - 0x2000;
        const SOFT_SCROLL_OFFSET: usize = SOFT_SCROLL_REGISTER - 0x2000;
        if (PALETTE_OFFSET..PALETTE_OFFSET + 17 * 2).contains(&offset) {
            let pen = ((offset - PALETTE_OFFSET) / 2) as u8;
            return Some(VideoWrite::Ink {
                pen,
                rgb: self.pen_color(pen),
            });
        }
        (offset == SOFT_SCROLL_OFFSET).then(|| VideoWrite::HScroll(self.h_scroll()))
    }

    fn write_dcsr(&mut self, val: u8) {
        self.dma_status = val;
        let mut acked = false;
        for (channel, bit) in [(0usize, 0x40u8), (1, 0x20), (2, 0x10)] {
            if val & bit != 0 {
                self.ram[DCSR_REGISTER] &= !bit;
                acked = true;
                let _ = channel;
            }
        }
        if acked {
            self.irq_unacknowledged = false;
            self.irq_cause = RASTER_IRQ_CAUSE;
        }
        self.ram[DCSR_REGISTER] = (self.ram[DCSR_REGISTER] & 0xF8) | (val & 0x07);
    }

    /// Fires the Programmable Raster Interrupt. Real hardware always
    /// auto-clears a raster-caused `/INT` on the CPU's own acknowledge
    /// cycle (unlike a DMA-caused one, see `irq_unacknowledged`'s own doc
    /// comment) -- a plain one-shot pulse, matching every other interrupt
    /// source in this codebase.
    pub(crate) fn trigger_raster(&mut self) {
        self.irq_cause = RASTER_IRQ_CAUSE;
    }

    fn trigger_dma_interrupt(&mut self, channel: usize) {
        self.irq_cause = (channel as u8) * 2;
        self.ram[DCSR_REGISTER] |= 0x40 >> channel;
        self.irq_unacknowledged = self.dma_clear_disabled;
    }

    pub(crate) fn irq_unacknowledged(&self) -> bool {
        self.irq_unacknowledged
    }

    /// The byte a real IM2 acknowledge cycle reads from the data bus --
    /// see `machine::Bus::interrupt_vector_byte`'s own doc comment. Also
    /// performs the same acknowledge-time bookkeeping real hardware does
    /// (DCSR status-bit updates), since this is the one point `Cpc::step`
    /// reliably calls exactly once per real CPU interrupt acceptance.
    pub(crate) fn interrupt_vector_byte(&mut self) -> u8 {
        if !self.enabled {
            // CPCTech's own page: "when the ASIC advanced features are
            // *not* enabled, this byte is always 0x00" -- a real, CPC+-
            // specific quirk, distinct from classic CPC's floating-bus
            // 0xFF (the `machine::Bus` trait default every other machine
            // here still uses).
            return 0x00;
        }
        if self.irq_cause != RASTER_IRQ_CAUSE && self.dma_clear_disabled {
            // Real, documented hardware bug: with auto-clear disabled,
            // acknowledging a DMA-caused interrupt does *not* clear the
            // line -- see `irq_unacknowledged`'s own doc comment. Only
            // the raster-interrupt status bit is reset here.
            self.ram[DCSR_REGISTER] &= !0x80;
            return (self.ram[INTERRUPT_VECTOR_REGISTER] & 0xF8) | self.irq_cause;
        }
        if self.irq_cause == RASTER_IRQ_CAUSE {
            self.ram[DCSR_REGISTER] |= 0x80;
        } else {
            self.ram[DCSR_REGISTER] &= !0x80;
            self.ram[DCSR_REGISTER] &= 0x40 >> (self.irq_cause / 2);
        }
        self.irq_unacknowledged = false;
        (self.ram[INTERRUPT_VECTOR_REGISTER] & 0xF8) | self.irq_cause
    }

    /// Runs exactly one DMA instruction step for each of the 3 channels
    /// that's currently enabled (`dma_status` bits 0-2), matching real
    /// hardware's "one instruction fetch per channel, per HSYNC" cadence
    /// (see module doc comment). `primary_ram`: the CPC's own unbanked
    /// 64K address space DMA command lists live in (same "CRTC always
    /// reads primary RAM" convention this crate's video already uses).
    pub(crate) fn step_dma(&mut self, primary_ram: &[u8]) -> [DmaResult; 3] {
        let mut results = [DmaResult::default(); 3];
        for (channel, result) in results.iter_mut().enumerate() {
            if self.dma_status & (1 << channel) != 0 {
                *result = self.step_dma_channel(channel, primary_ram);
            }
        }
        results
    }

    fn step_dma_channel(&mut self, channel: usize, primary_ram: &[u8]) -> DmaResult {
        if self.dma_addr[channel] & 1 != 0 {
            self.dma_addr[channel] = self.dma_addr[channel].wrapping_add(1);
        }
        if self.dma_pause[channel] != 0 {
            self.dma_prescaler[channel] = self.dma_prescaler[channel].wrapping_sub(1);
            if self.dma_prescaler[channel] == 0 {
                self.dma_pause[channel] -= 1;
                self.dma_prescaler[channel] = self.ram[DMA_PRESCALER[channel]].wrapping_add(1);
            }
            return DmaResult::default();
        }

        let addr = self.dma_addr[channel] as usize;
        let lo = *primary_ram.get(addr).unwrap_or(&0);
        let hi = *primary_ram
            .get(addr.wrapping_add(1) % primary_ram.len().max(1))
            .unwrap_or(&0);
        let command = u16::from(lo) | (u16::from(hi) << 8);
        let mut result = DmaResult::default();

        match command & 0xF000 {
            0x0000 => {
                // LOAD R,D -- load 8-bit data D into PSG register R.
                let register = ((command & 0x0F00) >> 8) as u8;
                let data = (command & 0x00FF) as u8;
                result.psg_load = Some((register, data));
            }
            0x1000 => {
                // PAUSE N -- pause for N prescaled HSYNC ticks.
                self.dma_pause[channel] = (command & 0x0FFF).wrapping_sub(1);
            }
            0x2000 => {
                // REPEAT N -- mark this instruction as the loop start.
                self.dma_repeat[channel] = self.dma_addr[channel];
                self.dma_loopcount[channel] = command & 0x0FFF;
            }
            0x4000 => {
                if command & 0x01 != 0 && self.dma_loopcount[channel] > 0 {
                    self.dma_addr[channel] = self.dma_repeat[channel];
                    self.dma_loopcount[channel] -= 1;
                }
                if command & 0x10 != 0 {
                    self.trigger_dma_interrupt(channel);
                    result.interrupt = true;
                }
                if command & 0x20 != 0 {
                    self.dma_status &= !(1 << channel);
                }
            }
            _ => {} // 0x3000 (reserved) and any other pattern: no effect, still advances below.
        }

        self.dma_addr[channel] = self.dma_addr[channel].wrapping_add(2);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unlock(asic: &mut Asic) {
        for &byte in ASIC_UNLOCK_SEQ.iter() {
            asic.check_unlock_sequence(byte);
        }
    }

    #[test]
    fn unlock_sequence_enables_asic() {
        let mut asic = Asic::new();
        assert!(!asic.enabled());
        unlock(&mut asic);
        assert!(asic.enabled());
    }

    #[test]
    fn a_mismatched_byte_after_a_nonzero_byte_resyncs_detection() {
        let mut asic = Asic::new();
        asic.check_unlock_sequence(0xAA); // garbage, non-zero
        asic.check_unlock_sequence(0x00); // resyncs (nonzero -> zero rule)
        unlock(&mut asic);
        assert!(asic.enabled());
    }

    #[test]
    fn sending_only_the_first_14_bytes_locks_while_already_unlocked() {
        // The 15th byte always re-enables regardless of its own value
        // (see `check_unlock_sequence`'s own doc comment) -- resending
        // the *complete* sequence only locks transiently before
        // immediately re-unlocking. Real locking requires stopping right
        // after the matching 14th byte.
        let mut asic = Asic::new();
        unlock(&mut asic);
        assert!(asic.enabled());
        for &byte in &ASIC_UNLOCK_SEQ[..14] {
            asic.check_unlock_sequence(byte);
        }
        assert!(!asic.enabled());
    }

    #[test]
    fn rmr2_write_requires_unlock_and_bit5() {
        let mut asic = Asic::new();
        assert!(!asic.maybe_write_rmr2(0x20));
        unlock(&mut asic);
        assert!(!asic.maybe_write_rmr2(0x00)); // bit5 clear: classic MRER write
        assert!(asic.maybe_write_rmr2(0x25));
        assert_eq!(asic.rmr2(), 0x25);
    }

    #[test]
    fn palette_entry_decodes_12_bit_rgb() {
        let mut asic = Asic::new();
        // Pen 3: low byte 0xF0 = R=15,B=0; high byte 0x0F -> G=15.
        asic.write_high(0x0406, 0xF0); // 0x2406 = PALETTE_BASE + 3*2
        asic.write_high(0x0407, 0xFF); // masked to 0x0F -> G=15
        assert_eq!(asic.pen_color(3), (255, 255, 0));
    }

    #[test]
    fn sprite_position_and_magnification_decode() {
        let mut asic = Asic::new();
        let base = 0x0000; // relative to 0x6000 -> sprite 0's info at &6000
        asic.write_high(base, 0x10); // X lo
        asic.write_high(base + 1, 0x00); // X hi
        asic.write_high(base + 2, 0x20); // Y lo
        asic.write_high(base + 3, 0x00); // Y hi
        asic.write_high(base + 4, 0b0110); // xmag=1(->0shift,16px) ymag=2(->1shift,32px)
        let sprite = asic.sprite(0);
        assert_eq!(sprite.x, 0x10);
        assert_eq!(sprite.y, 0x20);
        assert_eq!(sprite.width, 16);
        assert_eq!(sprite.height, 32);
    }

    #[test]
    fn sprite_pixel_data_masks_to_nibble() {
        let mut asic = Asic::new();
        asic.write_low(0, 0xFF);
        assert_eq!(asic.sprite_pixel(0, 0, 0), 0x0F);
    }

    #[test]
    fn dma_load_command_reports_psg_write() {
        let mut asic = Asic::new();
        let mut ram = vec![0u8; 0x10000];
        // LOAD register 7, data 0x3F at address 0x8000: command word
        // 0x073F (0RDD -- R=7, DD=0x3F), stored little-endian.
        ram[0x8000] = 0x3F;
        ram[0x8001] = 0x07;
        asic.write_high(0x0C00, 0x00); // addr lo
        asic.write_high(0x0C01, 0x80); // addr hi -> dma_addr[0]=0x8000
        asic.write_high(0x0C0F, 0x01); // enable channel 0
        let results = asic.step_dma(&ram);
        assert_eq!(results[0].psg_load, Some((7, 0x3F)));
    }

    #[test]
    fn dma_int_command_sets_held_interrupt_when_auto_clear_disabled() {
        let mut asic = Asic::new();
        let mut ram = vec![0u8; 0x10000];
        ram[0x8000] = 0x10; // INT
        ram[0x8001] = 0x40;
        asic.write_high(0x0C00, 0x00);
        asic.write_high(0x0C01, 0x80);
        asic.write_high(0x0C0F, 0x01);
        unlock(&mut asic);
        let results = asic.step_dma(&ram);
        assert!(results[0].interrupt);
        assert!(asic.irq_unacknowledged());
        // Vector byte reflects DMA channel 0's cause (0) without clearing.
        assert_eq!(asic.interrupt_vector_byte() & 0x07, 0);
        assert!(asic.irq_unacknowledged());
    }

    #[test]
    fn dcsr_write_acknowledges_and_clears_held_interrupt() {
        let mut asic = Asic::new();
        let mut ram = vec![0u8; 0x10000];
        ram[0x8000] = 0x10;
        ram[0x8001] = 0x40;
        asic.write_high(0x0C00, 0x00);
        asic.write_high(0x0C01, 0x80);
        asic.write_high(0x0C0F, 0x01);
        unlock(&mut asic);
        asic.step_dma(&ram);
        assert!(asic.irq_unacknowledged());
        asic.write_high(0x0C0F, 0x40); // ack DMA0
        assert!(!asic.irq_unacknowledged());
    }

    #[test]
    fn locked_asic_reports_zero_vector_byte() {
        let mut asic = Asic::new();
        assert_eq!(asic.interrupt_vector_byte(), 0x00);
    }

    #[test]
    fn unlocked_idle_asic_reports_raster_vector() {
        let mut asic = Asic::new();
        unlock(&mut asic);
        // Interrupt vector register defaults to 0 -- vector byte is just
        // the raster cause (6).
        assert_eq!(asic.interrupt_vector_byte(), 6);
    }

    #[test]
    fn analog_axis_defaults_centered_and_is_settable() {
        let asic = Asic::new();
        assert_eq!(asic.analog_read(0), 0x1F);
        let mut asic = Asic::new();
        asic.set_analog_axis(0, 0x3F);
        assert_eq!(asic.analog_read(0), 0x3F);
    }

    #[test]
    fn unused_analog_channels_report_fixed_idle_pattern() {
        let asic = Asic::new();
        assert_eq!(asic.analog_read(4), 0x3F);
        assert_eq!(asic.analog_read(5), 0x00);
        assert_eq!(asic.analog_read(6), 0x3F);
        assert_eq!(asic.analog_read(7), 0x00);
    }
}
