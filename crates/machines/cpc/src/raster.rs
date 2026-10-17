//! Per-character CPC picture trace and the monitor window that turns it
//! into pixels.
//!
//! Real CPC video is produced live: every microsecond the CRTC presents one
//! character (`MA`/`RA`, `DISPEN`, `HSYNC`, `VSYNC`), the Gate Array fetches
//! its two video-RAM bytes and shifts them out through whatever mode and
//! palette it holds *at that instant*. Programs lean on this -- raster bars
//! (palette changed every scanline or even mid-line), split-mode screens,
//! hardware scrolling (R12/R13 rewritten), overscan (R1/R2/R6 reprogrammed)
//! -- so a static decode of "the registers right now" (this crate's previous
//! design) cannot show them.
//!
//! Instead [`FrameBuilder`] records, for every character clock the CRTC
//! emits, the two fetched bytes plus display/sync flags, and logs every
//! Gate Array/ASIC video change (mode latch, ink write, scroll) against the
//! exact cell it takes effect on. [`Frame::render`] then replays that trace
//! into an RGB picture, so a change made at character *n* of scanline *m*
//! shows up from exactly there.
//!
//! **Monitor window.** The picture a monitor shows is defined relative to
//! the sync pulses, not to the CRTC's own counters: the beam restarts each
//! line on HSYNC and each field on VSYNC. [`Frame`]s therefore run from one
//! VSYNC start to the next, and each scanline is shown starting
//! [`HSYNC_TO_WINDOW_CHARS`] characters after its HSYNC begins for
//! [`WINDOW_CHARS`] characters, beginning [`VSYNC_TO_WINDOW_LINES`] lines
//! after VSYNC for [`WINDOW_HEIGHT`] lines. Moving R2/R7 therefore moves the
//! picture in the window exactly as it does on a real monitor. The three
//! constants are engineering choices (like `machine::with_border`'s 32 px
//! and `zx_spectrum::video::BORDER_PX`), fixed so the standard firmware
//! layout (R2=46, R7=30) shows the 40x25 display centred with 4 characters
//! (64 px) of border either side and 44 lines above and below; they are not
//! broadcast measurements. The window is 768x288: with each scanline shown
//! twice (see [`double_scanlines`]) that is the 768x576 square-pixel 4:3 PAL
//! frame, so the GUI window and a dumped PNG have a monitor's own shape.
//!
//! Output units are Mode-2 pixels (a character is 16 of them wide; a Mode 1
//! pixel is 2, a Mode 0 pixel 4), the shared coordinate space the Plus
//! sprites already used. [`Frame::render`] maps scan lines 1:1 to rows (the
//! space CPC+ sprite coordinates live in); `Cpc::render_frame` then doubles
//! every row for presentation.
//!
//! Approximations, documented rather than hidden: the Gate Array applies
//! changes at whole-character granularity (real silicon switches inks and
//! modes a fraction of a character in, chip- and mode-dependent); video RAM
//! is read at the character clock but the CPU's writes are only ordered
//! against it to instruction accuracy; and a change is stamped with the
//! T-state its I/O cycle ended on, which is exact for the memory/I/O bus
//! cycles the Z80 core reports and up to a T-state or two early/late for
//! purely internal cycles.

use crate::crtc::{CharClock, Crtc, ram_address};
use crate::video::{decode_mode0, decode_mode1, decode_mode2};

/// Visible characters per scanline, in the window.
pub(crate) const WINDOW_CHARS: usize = 48;
/// Window width in Mode-2 pixels (16 per character).
pub(crate) const WINDOW_WIDTH: u32 = (WINDOW_CHARS * 16) as u32;
/// Visible scanlines in the window.
pub(crate) const WINDOW_HEIGHT: u32 = 288;
/// The window's left edge sits this many characters after HSYNC begins
/// (R2 + 14: with the firmware's R2=46 that is character 60, giving 4
/// characters of left border before display starts at character 0).
pub(crate) const HSYNC_TO_WINDOW_CHARS: usize = 14;
/// The window's top edge sits this many lines after VSYNC begins.
pub(crate) const VSYNC_TO_WINDOW_LINES: usize = 28;
/// A monitor with no VSYNC free-runs at roughly this many lines per field.
const FREE_RUN_FRAME_LINES: usize = 312;
/// While VSYNC pulses are arriving a monitor locks to them whatever the field
/// length, so a field is only closed for lack of one after this many lines.
/// Arkanoid's title uses `R4=38, R5=4` -- a 316-line field -- and closing it
/// at 312 lines left a 4-line stub as the "most recent frame" (black) 89 times
/// out of 90.
const SYNCED_MAX_FRAME_LINES: usize = 2 * FREE_RUN_FRAME_LINES;
/// Safety bound on one frame's recorded characters (a program can set R0 to
/// 255 and R4/R9 large; never let the trace grow without limit).
const MAX_FRAME_CELLS: usize = 1 << 19;

pub(crate) type Rgb = [u8; 3];

const FLAG_DISPLAY: u8 = 1;
const FLAG_HSYNC: u8 = 2;
const FLAG_VSYNC: u8 = 4;

/// What the Gate Array (and ASIC) currently hold that affects the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VideoState {
    /// Mode in effect now (a requested change applies at the next HSYNC end).
    pub mode: u8,
    /// Mode last requested, waiting for the HSYNC falling edge to latch.
    pub requested_mode: u8,
    /// CPC+ soft-scroll horizontal offset in Mode-2 pixels (0 elsewhere).
    pub h_scroll: u8,
    /// Pens 0-15 plus the border (index 16), already resolved to RGB.
    pub inks: [Rgb; 17],
}

impl VideoState {
    pub(crate) fn new(mode: u8, h_scroll: u8, inks: [Rgb; 17]) -> Self {
        Self {
            mode,
            requested_mode: mode,
            h_scroll,
            inks,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    data: [u8; 2],
    flags: u8,
}

#[derive(Debug, Clone, Copy)]
enum Change {
    Mode(u8),
    Ink(u8, Rgb),
    HScroll(u8),
}

/// One field's worth of characters, VSYNC start to next VSYNC start.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    cells: Vec<Cell>,
    line_starts: Vec<u32>,
    /// `(cell index, change)`: the change applies from that cell onward.
    changes: Vec<(u32, Change)>,
    initial: VideoState,
}

/// A rendered window plus where the CRTC's display area landed in it.
pub(crate) struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// Window pixel position of the first displayed character of the first
    /// displayed line -- the origin CPC+ sprite coordinates are relative to.
    /// `None` when the frame displayed nothing.
    pub display_origin: Option<(u32, u32)>,
}

impl Frame {
    fn new(initial: VideoState) -> Self {
        Self {
            cells: Vec::new(),
            line_starts: Vec::new(),
            changes: Vec::new(),
            initial,
        }
    }

    #[cfg(test)]
    pub(crate) fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    #[cfg(test)]
    pub(crate) fn display_cells(&self) -> usize {
        self.cells
            .iter()
            .filter(|cell| cell.flags & FLAG_DISPLAY != 0)
            .count()
    }

    #[cfg(test)]
    pub(crate) fn vsync_cells(&self) -> usize {
        self.cells
            .iter()
            .filter(|cell| cell.flags & FLAG_VSYNC != 0)
            .count()
    }

    fn line_range(&self, line: usize) -> (usize, usize) {
        let start = self.line_starts[line] as usize;
        let end = self
            .line_starts
            .get(line + 1)
            .map_or(self.cells.len(), |&next| next as usize);
        (start, end)
    }

    /// Replays the trace into the monitor window.
    pub(crate) fn render(&self) -> Rendered {
        let mut rgb = vec![0u8; (WINDOW_WIDTH * WINDOW_HEIGHT) as usize * 3];
        let mut state = self.initial;
        let mut next_change = 0usize;
        let mut origin_char = 0usize;
        let mut display_origin = None;
        let mut line_pixels: Vec<Rgb> = Vec::new();

        for line in 0..self.line_starts.len() {
            let (start, end) = self.line_range(line);
            let cells = &self.cells[start..end];
            let in_window = line >= VSYNC_TO_WINDOW_LINES
                && line < VSYNC_TO_WINDOW_LINES + WINDOW_HEIGHT as usize;

            if !in_window {
                // Still apply the changes made on this line so the state at
                // the window's first line is right.
                while next_change < self.changes.len()
                    && (self.changes[next_change].0 as usize) < end
                {
                    apply_change(&mut state, self.changes[next_change].1);
                    next_change += 1;
                }
                continue;
            }

            line_pixels.clear();
            line_pixels.resize(cells.len() * 16, [0, 0, 0]);
            let mut first_display = None;
            let mut last_display = 0usize;
            let mut scroll_at_display = state.h_scroll;
            for (offset, cell) in cells.iter().enumerate() {
                while next_change < self.changes.len()
                    && (self.changes[next_change].0 as usize) <= start + offset
                {
                    apply_change(&mut state, self.changes[next_change].1);
                    next_change += 1;
                }
                if cell.flags & FLAG_DISPLAY != 0 {
                    if first_display.is_none() {
                        first_display = Some(offset);
                        scroll_at_display = state.h_scroll;
                    }
                    last_display = offset;
                }
                let strip: &mut [Rgb; 16] = (&mut line_pixels[offset * 16..offset * 16 + 16])
                    .try_into()
                    .expect("16-pixel strip");
                decode_cell(&state, *cell, strip);
            }

            // CPC+ horizontal soft scroll: the displayed span slides right,
            // the freed leading pixels showing the border colour.
            if let Some(first) = first_display
                && scroll_at_display != 0
            {
                let span = &mut line_pixels[first * 16..(last_display + 1) * 16];
                let shift = usize::from(scroll_at_display).min(span.len());
                span.rotate_right(shift);
                span[..shift].fill(state.inks[16]);
            }

            // HSYNC anchors the line: the window starts HSYNC_TO_WINDOW_CHARS
            // after it begins. A line without HSYNC keeps the previous line's
            // alignment (the monitor free-runs).
            let len = cells.len().max(1);
            if let Some(hsync) = cells.iter().position(|cell| cell.flags & FLAG_HSYNC != 0) {
                origin_char = hsync + HSYNC_TO_WINDOW_CHARS;
            }
            let row = line - VSYNC_TO_WINDOW_LINES;
            let row_base = row * WINDOW_WIDTH as usize * 3;
            for column in 0..WINDOW_CHARS {
                let character = (origin_char + column) % len;
                if display_origin.is_none()
                    && cells
                        .get(character)
                        .is_some_and(|c| c.flags & FLAG_DISPLAY != 0)
                {
                    display_origin = Some(((column * 16) as u32, row as u32));
                }
                let source = &line_pixels[character * 16..character * 16 + 16];
                let out = row_base + column * 16 * 3;
                for (pixel, color) in source.iter().enumerate() {
                    let at = out + pixel * 3;
                    rgb[at..at + 3].copy_from_slice(color);
                }
            }
        }

        // A field shorter than the window's last row (the standard 312-line
        // field ends 4 lines short of it) shows the border colour there, not
        // black: the beam is still in the border when the field ends.
        let lines_in_window = self
            .line_starts
            .len()
            .saturating_sub(VSYNC_TO_WINDOW_LINES)
            .min(WINDOW_HEIGHT as usize);
        let border = state.inks[16];
        let (pixels, _) = rgb[lines_in_window * WINDOW_WIDTH as usize * 3..].as_chunks_mut::<3>();
        pixels.fill(border);

        Rendered {
            width: WINDOW_WIDTH,
            height: WINDOW_HEIGHT,
            rgb,
            display_origin,
        }
    }
}

/// Shows every scanline twice: the monitor window is 288 lines of a 625-line
/// PAL field, i.e. 576 visible rows, so a 768x288-line window becomes the
/// 768x576 square-pixel 4:3 frame (the ITU-R BT.601 square-pixel PAL raster).
/// Presentation only -- CPC+ sprites and every other trace consumer work in
/// the undoubled one-row-per-line space.
pub(crate) fn double_scanlines(width: u32, rgb: &[u8]) -> Vec<u8> {
    let row_bytes = width as usize * 3;
    let mut out = Vec::with_capacity(rgb.len() * 2);
    for row in rgb.chunks_exact(row_bytes) {
        out.extend_from_slice(row);
        out.extend_from_slice(row);
    }
    out
}

fn apply_change(state: &mut VideoState, change: Change) {
    match change {
        Change::Mode(mode) => state.mode = mode,
        Change::Ink(pen, color) => state.inks[usize::from(pen).min(16)] = color,
        Change::HScroll(scroll) => state.h_scroll = scroll,
    }
}

/// Expands one character (two video bytes) into 16 Mode-2 pixels.
fn decode_cell(state: &VideoState, cell: Cell, out: &mut [Rgb; 16]) {
    // Blanking wins over everything: the Gate Array cuts the picture to black
    // for the duration of HSYNC/VSYNC.
    if cell.flags & (FLAG_HSYNC | FLAG_VSYNC) != 0 {
        out.fill([0, 0, 0]);
        return;
    }
    if cell.flags & FLAG_DISPLAY == 0 {
        out.fill(state.inks[16]);
        return;
    }
    let inks = &state.inks;
    for (index, &byte) in cell.data.iter().enumerate() {
        let base = index * 8;
        match state.mode {
            0 => {
                let [left, right] = decode_mode0(byte);
                out[base..base + 4].fill(inks[usize::from(left)]);
                out[base + 4..base + 8].fill(inks[usize::from(right)]);
            }
            1 => {
                for pixel in 0..4u8 {
                    let color = inks[usize::from(decode_mode1(byte, pixel))];
                    let at = base + usize::from(pixel) * 2;
                    out[at..at + 2].fill(color);
                }
            }
            2 => {
                for (pixel, pen) in decode_mode2(byte).into_iter().enumerate() {
                    out[base + pixel] = inks[usize::from(pen)];
                }
            }
            _ => {
                // Undocumented Mode 3: Mode 1's first two pixels per byte,
                // four Mode-2 dots each.
                out[base..base + 4].fill(inks[usize::from(decode_mode1(byte, 0))]);
                out[base + 4..base + 8].fill(inks[usize::from(decode_mode1(byte, 1))]);
            }
        }
    }
}

/// The two video-RAM bytes the Gate Array fetches for one character clock
/// (`[0, 0]` when display is off). `v_scroll` is the CPC+ vertical
/// soft-scroll offset: the raster address is advanced by it, spilling into
/// the next character row past `R9` (0 on every classic model).
pub(crate) fn fetch_video_bytes(ram: &[u8], clock: &CharClock, v_scroll: u8) -> [u8; 2] {
    if !clock.display {
        return [0, 0];
    }
    let (mut ma, mut ra) = (clock.ma, clock.ra);
    if v_scroll != 0 {
        let effective = ra + v_scroll;
        if effective <= clock.max_raster {
            ra = effective;
        } else {
            ma = ma.wrapping_add(u16::from(clock.r1)) & 0x3FFF;
            ra = effective - (clock.max_raster + 1);
        }
    }
    let address = usize::from(ram_address(ma, ra));
    [ram[address], ram[address | 1]]
}

/// A picture built from "the registers right now": a copy of the CRTC is
/// run through one whole field with the Gate Array state frozen. Used until
/// the live machine has completed a field of its own (a freshly created or
/// freshly state-loaded machine), so `render_frame` never has nothing to
/// show.
pub(crate) fn synthetic_frame(
    crtc: &Crtc,
    state: VideoState,
    ram: &[u8],
    split: Option<(u8, u16)>,
    v_scroll: u8,
) -> Frame {
    let mut crtc = crtc.clone();
    crtc.reload_start_address();
    if let Some((line, address)) = split {
        crtc.set_split(line, address);
    }
    let mut builder = FrameBuilder::new(state);
    // The first field seen begins mid-frame; the second is complete.
    let budget = 2 * (FREE_RUN_FRAME_LINES + 2) * 256;
    let mut clocked = 0;
    while builder.frames_completed() < 2 && clocked < budget {
        crtc.tick_with(64 * 4, |clock| {
            builder.on_char(clock, fetch_video_bytes(ram, clock, v_scroll));
        });
        clocked += 64;
    }
    builder.last.unwrap_or_else(|| Frame::new(state))
}

/// Collects the character stream into [`Frame`]s and owns the "shadow"
/// Gate Array video state the trace's changes are recorded against.
#[derive(Debug, Clone)]
pub(crate) struct FrameBuilder {
    current: Frame,
    last: Option<Frame>,
    state: VideoState,
    previous_hsync: bool,
    previous_vsync: bool,
    /// A VSYNC has begun since the last field was closed for lack of one: the
    /// monitor is locked to the sync pulses, so fields run VSYNC to VSYNC
    /// however long (see [`SYNCED_MAX_FRAME_LINES`]).
    synced: bool,
    frames_completed: u64,
}

impl FrameBuilder {
    pub(crate) fn new(state: VideoState) -> Self {
        Self {
            current: Frame::new(state),
            last: None,
            state,
            previous_hsync: false,
            previous_vsync: false,
            synced: false,
            frames_completed: 0,
        }
    }

    /// Drops every recorded frame and adopts `state` (a machine load/reset
    /// replaced the Gate Array behind the builder's back).
    pub(crate) fn reset(&mut self, state: VideoState) {
        *self = Self::new(state);
    }

    /// The most recently completed frame, if any.
    pub(crate) fn last_frame(&self) -> Option<&Frame> {
        self.last.as_ref()
    }

    pub(crate) fn frames_completed(&self) -> u64 {
        self.frames_completed
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> &VideoState {
        &self.state
    }

    /// Gate Array function-2 write: the new mode waits for the next HSYNC.
    pub(crate) fn request_mode(&mut self, mode: u8) {
        self.state.requested_mode = mode & 3;
    }

    pub(crate) fn set_ink(&mut self, pen: u8, color: Rgb) {
        let pen = pen.min(16);
        if self.state.inks[usize::from(pen)] != color {
            self.state.inks[usize::from(pen)] = color;
            self.record(Change::Ink(pen, color));
        }
    }

    pub(crate) fn set_h_scroll(&mut self, scroll: u8) {
        if self.state.h_scroll != scroll {
            self.state.h_scroll = scroll;
            self.record(Change::HScroll(scroll));
        }
    }

    fn record(&mut self, change: Change) {
        self.current
            .changes
            .push((self.current.cells.len() as u32, change));
    }

    fn finish_frame(&mut self) {
        let next = Frame::new(self.state);
        let finished = std::mem::replace(&mut self.current, next);
        if !finished.line_starts.is_empty() {
            self.last = Some(finished);
            self.frames_completed += 1;
        }
    }

    /// Feeds one character clock. `data` is the two video-RAM bytes at that
    /// character's address; it is only kept when the CRTC has display on.
    pub(crate) fn on_char(&mut self, clock: &CharClock, data: [u8; 2]) {
        // VSYNC begin closes the previous frame and opens the next.
        if clock.vsync && !self.previous_vsync {
            self.finish_frame();
            self.synced = true;
        }
        self.previous_vsync = clock.vsync;

        // The Gate Array latches a requested mode change on the HSYNC
        // falling edge.
        if self.previous_hsync && !clock.hsync && self.state.mode != self.state.requested_mode {
            self.state.mode = self.state.requested_mode;
            let mode = self.state.mode;
            self.record(Change::Mode(mode));
        }
        self.previous_hsync = clock.hsync;

        let first_cell_of_line = clock.c0 == 0 || self.current.line_starts.is_empty();
        if first_cell_of_line {
            // A monitor with no VSYNC still scans a field; close free-running
            // frames at PAL length so the picture keeps updating. One that
            // *is* seeing VSYNC pulses locks to them instead, however long the
            // CRTC's field is, and only gives up after twice the PAL length.
            let limit = if self.synced {
                SYNCED_MAX_FRAME_LINES
            } else {
                FREE_RUN_FRAME_LINES
            };
            if self.current.line_starts.len() >= limit
                || self.current.cells.len() >= MAX_FRAME_CELLS
            {
                if self.synced {
                    self.synced = false; // lost sync: free-run until VSYNC returns
                }
                self.finish_frame();
            }
            self.current
                .line_starts
                .push(self.current.cells.len() as u32);
        }

        let mut flags = 0;
        if clock.display {
            flags |= FLAG_DISPLAY;
        }
        if clock.hsync {
            flags |= FLAG_HSYNC;
        }
        if clock.vsync {
            flags |= FLAG_VSYNC;
        }
        self.current.cells.push(Cell {
            data: if clock.display { data } else { [0, 0] },
            flags,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crtc::{Crtc, ram_address};

    const BLACK: Rgb = [0, 0, 0];

    fn inks(pen_colors: &[(usize, Rgb)], border: Rgb) -> [Rgb; 17] {
        let mut inks = [BLACK; 17];
        inks[16] = border;
        for &(pen, color) in pen_colors {
            inks[pen] = color;
        }
        inks
    }

    fn firmware_crtc() -> Crtc {
        let mut crtc = Crtc::new();
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
            crtc.select_register(register);
            crtc.write_register(value);
        }
        crtc.reload_start_address();
        crtc
    }

    /// Runs `characters` character clocks through a builder, fetching video
    /// bytes from `ram` like the machine does.
    fn run(builder: &mut FrameBuilder, crtc: &mut Crtc, ram: &[u8], characters: u32) {
        crtc.tick_with(characters * 4, |clock| {
            let address = usize::from(ram_address(clock.ma, clock.ra));
            builder.on_char(clock, [ram[address], ram[address | 1]]);
        });
    }

    /// Pen `0xB6` decodes to for Mode 1's second pixel, times the test's
    /// 16-per-pen red ramp.
    fn pens_for_mode1_second() -> u8 {
        decode_mode1(0xB6, 1) * 16
    }

    /// Two full firmware-layout frames (the first is a partial one).
    fn two_firmware_frames() -> u32 {
        64 * 312 * 2 + 64
    }

    fn pixel(rendered: &Rendered, x: u32, y: u32) -> Rgb {
        let at = ((y * rendered.width + x) * 3) as usize;
        [rendered.rgb[at], rendered.rgb[at + 1], rendered.rgb[at + 2]]
    }

    #[test]
    fn frames_run_from_one_vsync_to_the_next() {
        let mut crtc = firmware_crtc();
        let mut builder = FrameBuilder::new(VideoState::new(1, 0, inks(&[], BLACK)));
        run(
            &mut builder,
            &mut crtc,
            &[0; 0x10000],
            two_firmware_frames(),
        );
        assert!(builder.frames_completed() >= 2);
        // 39 rows of 8 lines with no adjust.
        assert_eq!(builder.last_frame().unwrap().line_count(), 312);
    }

    #[test]
    fn standard_layout_centres_the_display_in_the_window() {
        let border = [10, 20, 30];
        let paper = [200, 0, 0];
        let mut ram = vec![0u8; 0x10000];
        // Mode 2, pen 1 = paper: first display byte all ink.
        ram[0xC000] = 0xFF;
        // Display line 1 (raster 1) starts at 0xC800.
        ram[0xC800] = 0x80;
        let mut crtc = firmware_crtc();
        let mut builder = FrameBuilder::new(VideoState::new(2, 0, inks(&[(1, paper)], border)));
        run(&mut builder, &mut crtc, &ram, two_firmware_frames());
        let rendered = builder.last_frame().unwrap().render();

        assert_eq!((rendered.width, rendered.height), (768, 288));
        // 4 characters of border on the left, 44 lines on top.
        assert_eq!(rendered.display_origin, Some((64, 44)));
        assert_eq!(pixel(&rendered, 0, 0), border);
        assert_eq!(pixel(&rendered, 63, 44), border);
        // The first byte is 8 ink pixels, then the second byte's paper (0).
        for x in 64..72 {
            assert_eq!(pixel(&rendered, x, 44), paper, "x={x}");
        }
        assert_eq!(pixel(&rendered, 72, 44), BLACK);
        // Raster 1 of the row is 0x800 further on.
        assert_eq!(pixel(&rendered, 64, 45), paper);
        assert_eq!(pixel(&rendered, 65, 45), BLACK);
        // Right border and bottom border.
        assert_eq!(pixel(&rendered, 64 + 640, 44), border);
        assert_eq!(pixel(&rendered, 100, 44 + 200), border);
        // The 312-line field ends 4 lines before the window's last row; those
        // rows show the border colour rather than black.
        assert_eq!(pixel(&rendered, 100, 283), border);
        assert_eq!(pixel(&rendered, 100, 284), border);
        assert_eq!(pixel(&rendered, 767, 287), border);
    }

    #[test]
    fn doubling_scanlines_repeats_every_row_and_keeps_the_picture_4_by_3() {
        let rgb: Vec<u8> = (0..2 * 3 * 3).map(|v| v as u8).collect(); // 2 wide, 3 rows
        let doubled = double_scanlines(2, &rgb);
        assert_eq!(doubled.len(), rgb.len() * 2);
        for row in 0..3 {
            let src = &rgb[row * 6..row * 6 + 6];
            assert_eq!(&doubled[row * 12..row * 12 + 6], src);
            assert_eq!(&doubled[row * 12 + 6..row * 12 + 12], src);
        }
        // The shipped window is exactly 4:3 once doubled.
        assert_eq!(WINDOW_WIDTH * 3, WINDOW_HEIGHT * 2 * 4);
    }

    fn moving_the_hsync_position_moves_the_picture() {
        let mut ram = vec![0u8; 0x10000];
        ram[0xC000] = 0xFF;
        let paper = [1, 2, 3];
        let mut crtc = firmware_crtc();
        crtc.select_register(2);
        crtc.write_register(47); // HSYNC a character later -> picture one character left
        let mut builder = FrameBuilder::new(VideoState::new(2, 0, inks(&[(1, paper)], BLACK)));
        run(&mut builder, &mut crtc, &ram, two_firmware_frames());
        let rendered = builder.last_frame().unwrap().render();
        assert_eq!(rendered.display_origin.unwrap().0, 64 - 16);

        // ...and an earlier HSYNC moves it right.
        let mut crtc = firmware_crtc();
        crtc.select_register(2);
        crtc.write_register(44);
        let mut builder = FrameBuilder::new(VideoState::new(2, 0, inks(&[(1, paper)], BLACK)));
        run(&mut builder, &mut crtc, &ram, two_firmware_frames());
        let rendered = builder.last_frame().unwrap().render();
        assert_eq!(rendered.display_origin.unwrap().0, 64 + 32);
    }

    fn ink_changes_apply_from_the_character_they_were_made_on() {
        let mut ram = vec![0u8; 0x10000];
        // Every displayed byte is pen 1 in mode 2.
        for byte in ram.iter_mut() {
            *byte = 0xFF;
        }
        let first = [255, 0, 0];
        let second = [0, 255, 0];
        let mut crtc = firmware_crtc();
        let mut builder = FrameBuilder::new(VideoState::new(2, 0, inks(&[(1, first)], BLACK)));
        // Run to just before the second frame's display starts (a few
        // characters into a frame), change the ink, keep running.
        run(&mut builder, &mut crtc, &ram, 64 * 312 + 64 * 100 + 20);
        builder.set_ink(1, second);
        run(&mut builder, &mut crtc, &ram, 64 * 312);
        let rendered = builder.last_frame().unwrap().render();
        // The frame now being completed straddles the change: before it the
        // display is `first`, after it `second`.
        let mut seen_first = false;
        let mut seen_second = false;
        for y in 44..244 {
            match pixel(&rendered, 70, y) {
                c if c == first => {
                    assert!(!seen_second, "first colour reappeared after the change");
                    seen_first = true;
                }
                c if c == second => seen_second = true,
                other => panic!("unexpected colour {other:?} at line {y}"),
            }
        }
        assert!(seen_first && seen_second);
    }

    fn a_mode_request_latches_at_the_next_hsync_end() {
        // Every video byte is 0xB6: mode 0 reads it as pens 13 then 6,
        // mode 1 as pens 1, 2, 3, 2 ... (`decode_mode1`).
        let ram = vec![0xB6u8; 0x10000];
        let mut crtc = firmware_crtc();
        let pens: Vec<(usize, Rgb)> = (0..16).map(|pen| (pen, [pen as u8 * 16, 0, 0])).collect();
        let mut builder = FrameBuilder::new(VideoState::new(1, 0, inks(&pens, BLACK)));
        run(&mut builder, &mut crtc, &ram, two_firmware_frames());
        builder.request_mode(0);
        // Nothing changes until an HSYNC ends.
        assert_eq!(builder.state().mode, 1);
        run(&mut builder, &mut crtc, &ram, 64 * 312 + 64);
        assert_eq!(builder.state().mode, 0);
        let rendered = builder.last_frame().unwrap().render();
        let (x, y) = rendered.display_origin.unwrap();
        // Display line 0 came before the request: still Mode 1 (2-dot pixels).
        assert_eq!(pixel(&rendered, x, y), [16, 0, 0]);
        assert_eq!(pixel(&rendered, x + 2, y), [pens_for_mode1_second(), 0, 0]);
        // Later lines are Mode 0: two 4-dot pixels per byte, pens 13 and 6.
        assert_eq!(pixel(&rendered, x, y + 10), [13 * 16, 0, 0]);
        assert_eq!(pixel(&rendered, x + 3, y + 10), [13 * 16, 0, 0]);
        assert_eq!(pixel(&rendered, x + 4, y + 10), [6 * 16, 0, 0]);
    }
    /// Merged (every check kept): `ink_changes_apply_from_the_character_they_were_made_on`, `a_mode_request_latches_at_the_next_hsync_end`.
    #[test]
    fn mid_line_changes() {
        ink_changes_apply_from_the_character_they_were_made_on();
        a_mode_request_latches_at_the_next_hsync_end();
    }

    fn hsync_and_vsync_blank_the_picture() {
        let mut crtc = firmware_crtc();
        // Push HSYNC into the window: R2=56 puts it at window characters
        // (56 + 14 - 56 ...) -- easier: widen it over the window's left edge.
        crtc.select_register(2);
        crtc.write_register(40);
        crtc.select_register(3);
        crtc.write_register(0x8F); // 15 characters: 40..54, window starts at 54
        let mut builder = FrameBuilder::new(VideoState::new(2, 0, inks(&[], [9, 9, 9])));
        run(
            &mut builder,
            &mut crtc,
            &[0; 0x10000],
            two_firmware_frames(),
        );
        let rendered = builder.last_frame().unwrap().render();
        // Window character 0 is line character 54 -- inside the 15-wide HSYNC
        // (characters 40..=54), so it is black rather than border coloured.
        assert_eq!(pixel(&rendered, 0, 100), BLACK);
        assert_eq!(pixel(&rendered, 16, 100), [9, 9, 9]);
    }
    /// Merged (every check kept): `hsync_and_vsync_blank_the_picture`, `moving_the_hsync_position_moves_the_picture`.
    #[test]
    fn hsync_effects() {
        hsync_and_vsync_blank_the_picture();
        moving_the_hsync_position_moves_the_picture();
    }

    /// The firmware layout but with `R5 = 4` (a 316-line field), as Arkanoid's
    /// title screen uses.
    fn long_field_crtc() -> Crtc {
        let mut crtc = firmware_crtc();
        crtc.select_register(5);
        crtc.write_register(4);
        crtc
    }

    fn a_field_longer_than_312_lines_with_vsync_is_not_split() {
        let mut ram = vec![0u8; 0x10000];
        ram[0xC000] = 0xFF;
        let mut crtc = long_field_crtc();
        let mut builder =
            FrameBuilder::new(VideoState::new(2, 0, inks(&[(1, [200, 0, 0])], BLACK)));
        run(&mut builder, &mut crtc, &ram, 64 * 316 * 6 + 64 * 40);
        let before = builder.frames_completed();
        // Every completed field is a whole 316-line one with its VSYNC and its
        // display -- never a short stub left over from a 312-line cut.
        for _ in 0..12 {
            run(&mut builder, &mut crtc, &ram, 64 * 27);
            let last = builder.last_frame().unwrap();
            assert_eq!(last.line_count(), 316, "the last completed field");
            assert!(last.display_cells() > 0 && last.vsync_cells() > 0);
        }
        // ... and each 316 lines completes exactly one field, not two.
        let after = builder.frames_completed();
        run(&mut builder, &mut crtc, &ram, 64 * 316 * 4);
        assert_eq!(builder.frames_completed() - after, 4);
        assert!(after > before);
    }

    fn a_long_field_renders_a_picture_every_time_it_is_sampled() {
        let mut ram = vec![0u8; 0x10000];
        ram[0xC000] = 0xFF;
        let paper = [200, 0, 0];
        let mut crtc = long_field_crtc();
        let mut builder = FrameBuilder::new(VideoState::new(2, 0, inks(&[(1, paper)], BLACK)));
        run(&mut builder, &mut crtc, &ram, 64 * 316 * 3);
        // Sample at many different phases: the picture must always be there.
        for phase in 0..40 {
            run(&mut builder, &mut crtc, &ram, 64 * 31 + phase);
            let rendered = builder.last_frame().unwrap().render();
            assert_eq!(
                pixel(
                    &rendered,
                    rendered.display_origin.unwrap().0,
                    rendered.display_origin.unwrap().1
                ),
                paper,
                "phase {phase}: the display's first pixel"
            );
        }
    }
    /// Merged (every check kept): `a_field_longer_than_312_lines_with_vsync_is_not_split`, `a_long_field_renders_a_picture_every_time_it_is_sampled`.
    #[test]
    fn long_fields() {
        a_field_longer_than_312_lines_with_vsync_is_not_split();
        a_long_field_renders_a_picture_every_time_it_is_sampled();
    }

    fn a_synced_screen_that_loses_vsync_falls_back_to_free_running() {
        let mut crtc = firmware_crtc();
        let mut builder = FrameBuilder::new(VideoState::new(1, 0, inks(&[], BLACK)));
        run(&mut builder, &mut crtc, &[0; 0x10000], 64 * 312 * 3);
        assert!(builder.synced);
        crtc.select_register(7);
        crtc.write_register(100); // VSYNC position beyond R4: no more pulses
        run(&mut builder, &mut crtc, &[0; 0x10000], 64 * 312 * 8);
        assert!(!builder.synced, "gave up waiting for VSYNC");
        let frames = builder.frames_completed();
        run(&mut builder, &mut crtc, &[0; 0x10000], 64 * 312 * 3);
        assert!(
            builder.frames_completed() >= frames + 2,
            "free-running again"
        );
        assert_eq!(
            builder.last_frame().unwrap().line_count(),
            FREE_RUN_FRAME_LINES
        );
    }

    fn a_frame_without_vsync_still_completes_at_pal_length() {
        let mut crtc = firmware_crtc();
        crtc.select_register(7);
        crtc.write_register(100); // VSYNC position beyond R4: never fires
        let mut builder = FrameBuilder::new(VideoState::new(1, 0, inks(&[], BLACK)));
        run(&mut builder, &mut crtc, &[0; 0x10000], 64 * 312 * 3);
        assert!(builder.frames_completed() >= 2);
        assert_eq!(
            builder.last_frame().unwrap().line_count(),
            FREE_RUN_FRAME_LINES
        );
    }
    /// Merged (every check kept): `a_synced_screen_that_loses_vsync_falls_back_to_free_running`, `a_frame_without_vsync_still_completes_at_pal_length`.
    #[test]
    fn missing_vsync() {
        a_synced_screen_that_loses_vsync_falls_back_to_free_running();
        a_frame_without_vsync_still_completes_at_pal_length();
    }

    #[test]
    fn horizontal_soft_scroll_slides_the_display_right() {
        let mut ram = vec![0u8; 0x10000];
        ram[0xC000] = 0xFF;
        let paper = [7, 7, 7];
        let border = [1, 1, 1];
        let mut crtc = firmware_crtc();
        let mut builder = FrameBuilder::new(VideoState::new(2, 0, inks(&[(1, paper)], border)));
        builder.set_h_scroll(3);
        run(&mut builder, &mut crtc, &ram, two_firmware_frames());
        let rendered = builder.last_frame().unwrap().render();
        let (x, y) = rendered.display_origin.unwrap();
        for dx in 0..3 {
            assert_eq!(
                pixel(&rendered, x + dx, y),
                border,
                "scrolled-in border {dx}"
            );
        }
        assert_eq!(pixel(&rendered, x + 3, y), paper);
        assert_eq!(pixel(&rendered, x + 10, y), paper);
        assert_eq!(pixel(&rendered, x + 11, y), BLACK);
    }
}
