//! Intel 8255 PPI (Programmable Peripheral Interface) -- 3 general-purpose
//! 8-bit ports (A/B/C), each independently configurable as input or
//! output via the control register. **Mode 0 (basic I/O) only** -- modes
//! 1/2 (strobed/bidirectional handshake) exist on real hardware but no
//! host this crate is aware of at this project's own scale (the Amstrad
//! CPC) uses them, so they aren't modeled; a mode-set write's group-mode
//! bits are stored (echoed back on read) but never change how `read_*`/
//! `write_*` behave. Same "protocol-only, host supplies the specifics"
//! shape as `nec765`/`ay-3-8910` -- no dependency on `machine`, any CPU
//! crate, or a specific host machine at all.
//!
//! Register/bit layout (control-word format, per-port direction bits,
//! the bit-set/reset sub-mode for Port C) is the 8255's own
//! well-documented, widely-published architecture -- same "how the chip
//! works" epistemic tier as `ay-3-8910`'s own `CREDITS.md` entry, not
//! attributed to one specific source.

const CONTROL_MODE_SET: u8 = 0x80;
const CONTROL_PA_INPUT: u8 = 0x10;
const CONTROL_PCH_INPUT: u8 = 0x08;
const CONTROL_PB_INPUT: u8 = 0x02;
const CONTROL_PCL_INPUT: u8 = 0x01;

/// Real 8255 power-up/reset default: mode 0, every port configured as
/// input (control byte `0x9B`) -- commonly documented reset behavior.
const DEFAULT_CONTROL: u8 = 0x9B;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Ppi {
    /// The last mode-set control word (a write with bit 7 set) -- what
    /// actually determines each port's direction. A bit-set/reset write
    /// (bit 7 clear) manipulates `port_c` directly instead and never
    /// touches this.
    control: u8,
    port_a: u8,
    port_b: u8,
    port_c: u8,
    /// External input latches -- the host sets these to reflect real-world
    /// pin state (e.g. a keyboard row byte, VSYNC/cassette status) for
    /// whichever bits are currently configured as input. Mirrors the same
    /// "driven output reflects what was written, undriven input reflects
    /// the outside world" split `c64::bus::C64Bus::port01`/
    /// `keyboard_port_b` already use for the 6510/CIA1 ports.
    input_a: u8,
    input_b: u8,
    input_c: u8,
}

impl Default for Ppi {
    fn default() -> Self {
        Self {
            control: DEFAULT_CONTROL,
            port_a: 0,
            port_b: 0,
            port_c: 0,
            input_a: 0,
            input_b: 0,
            input_c: 0,
        }
    }
}

impl Ppi {
    pub fn new() -> Self {
        Self::default()
    }

    fn port_a_input(&self) -> bool {
        self.control & CONTROL_PA_INPUT != 0
    }

    fn port_b_input(&self) -> bool {
        self.control & CONTROL_PB_INPUT != 0
    }

    /// Port C's two nibbles have independent direction bits on real
    /// hardware -- returns the effective per-bit input mask (`1` = that
    /// bit is input), upper nibble from `CONTROL_PCH_INPUT`, lower from
    /// `CONTROL_PCL_INPUT`.
    fn port_c_input_mask(&self) -> u8 {
        let upper = if self.control & CONTROL_PCH_INPUT != 0 {
            0xF0
        } else {
            0
        };
        let lower = if self.control & CONTROL_PCL_INPUT != 0 {
            0x0F
        } else {
            0
        };
        upper | lower
    }

    /// Writes the control register. A byte with bit 7 set is a real
    /// mode-set (reconfigures every port's direction, and -- since real
    /// hardware resets all outputs to `0` whenever the control register
    /// is reconfigured this way -- clears every port's own output
    /// latch too). A byte with bit 7 clear is the real "bit set/reset"
    /// sub-mode: bits 3:1 pick one of Port C's 8 bits, bit 0 sets (`1`)
    /// or clears (`0`) it, direction untouched.
    pub fn write_control(&mut self, val: u8) {
        if val & CONTROL_MODE_SET != 0 {
            self.control = val;
            self.port_a = 0;
            self.port_b = 0;
            self.port_c = 0;
        } else {
            let bit = (val >> 1) & 0x07;
            if val & 0x01 != 0 {
                self.port_c |= 1 << bit;
            } else {
                self.port_c &= !(1 << bit);
            }
        }
    }

    pub fn control(&self) -> u8 {
        self.control
    }

    pub fn write_port_a(&mut self, val: u8) {
        self.port_a = val;
    }

    pub fn write_port_b(&mut self, val: u8) {
        self.port_b = val;
    }

    pub fn write_port_c(&mut self, val: u8) {
        self.port_c = val;
    }

    pub fn read_port_a(&self) -> u8 {
        if self.port_a_input() {
            self.input_a
        } else {
            self.port_a
        }
    }

    pub fn read_port_b(&self) -> u8 {
        if self.port_b_input() {
            self.input_b
        } else {
            self.port_b
        }
    }

    pub fn read_port_c(&self) -> u8 {
        let input_mask = self.port_c_input_mask();
        (self.input_c & input_mask) | (self.port_c & !input_mask)
    }

    pub fn set_input_a(&mut self, val: u8) {
        self.input_a = val;
    }

    pub fn set_input_b(&mut self, val: u8) {
        self.input_b = val;
    }

    pub fn set_input_c(&mut self, val: u8) {
        self.input_c = val;
    }
    /// Restores a CPC snapshot's raw PPI output latches and mode word.
    /// External input latches are intentionally left to the host wiring.
    pub fn restore_snapshot_state(&mut self, port_a: u8, port_b: u8, port_c: u8, control: u8) {
        self.port_a = port_a;
        self.port_b = port_b;
        self.port_c = port_c;
        self.control = control;
    }

    pub fn port_a_latch(&self) -> u8 {
        self.port_a
    }

    pub fn port_b_latch(&self) -> u8 {
        self.port_b
    }

    pub fn port_c_latch(&self) -> u8 {
        self.port_c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_every_port_input() {
        let ppi = Ppi::new();
        assert_eq!(ppi.control(), 0x9B);
    }

    #[test]
    fn output_port_reads_back_what_was_written() {
        let mut ppi = Ppi::new();
        ppi.write_control(0x80); // mode 0, every port output
        ppi.write_port_a(0x42);
        assert_eq!(ppi.read_port_a(), 0x42);
    }

    #[test]
    fn input_port_reads_the_external_latch_not_what_was_written() {
        let mut ppi = Ppi::new(); // default: Port A is input
        ppi.write_port_a(0x11); // written value ignored for reads
        ppi.set_input_a(0x99);
        assert_eq!(ppi.read_port_a(), 0x99);
    }

    #[test]
    fn port_c_nibbles_have_independent_direction() {
        let mut ppi = Ppi::new();
        // Mode 0, Port A output, Port C upper input, Port C lower output.
        ppi.write_control(0x80 | CONTROL_PCH_INPUT);
        ppi.write_port_c(0x0A); // lower nibble output value
        ppi.set_input_c(0xF0); // upper nibble external input
        assert_eq!(ppi.read_port_c(), 0xFA);
    }

    #[test]
    fn bit_set_reset_only_touches_port_c_and_leaves_direction_alone() {
        let mut ppi = Ppi::new();
        ppi.write_control(0x80); // mode 0, everything output
        ppi.write_control(0b0000_0111); // bit-set/reset: set bit 3
        assert_eq!(ppi.read_port_c() & 0x08, 0x08);
        ppi.write_control(0b0000_0110); // bit-set/reset: clear bit 3
        assert_eq!(ppi.read_port_c() & 0x08, 0);
        assert_eq!(ppi.control(), 0x80, "direction word must be untouched");
    }

    #[test]
    fn reconfiguring_direction_clears_output_latches() {
        let mut ppi = Ppi::new();
        ppi.write_control(0x80);
        ppi.write_port_a(0xFF);
        ppi.write_control(0x80); // real hardware: any mode-set write resets outputs to 0
        assert_eq!(ppi.read_port_a(), 0x00);
    }
}
