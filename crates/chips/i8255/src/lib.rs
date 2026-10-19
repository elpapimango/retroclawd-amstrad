//! Intel 8255 PPI (Programmable Peripheral Interface) -- 3 general-purpose
//! 8-bit ports (A/B/C), each independently configurable as input or
//! output via the control register, with the handshake modes: group A in
//! mode 0 (basic I/O), mode 1 (strobed input or output) or mode 2 (strobed
//! bidirectional), group B in mode 0 or mode 1. Same "protocol-only, host
//! supplies the specifics" shape as `nec765`/`ay-3-8910` -- no dependency
//! on `machine`, any CPU crate, or a specific host machine at all.
//!
//! Register/bit layout (control-word format, per-port direction bits,
//! the bit-set/reset sub-mode for Port C, the handshake pins) is the
//! 8255's own well-documented, widely-published architecture; what a
//! host's own wiring does with the pins is the host's business (the
//! Amstrad CPC leaves the strobe/acknowledge pins unconnected).
//!
//! **Port C in the handshake modes** (Intel datasheet, checked against the
//! real-hardware results of Kevin Thacker's CPC `ppi` acid test):
//!
//! | pin | group A mode 1 input | group A mode 1 output | mode 2 | group B mode 1 input | group B mode 1 output |
//! |-----|----------------------|-----------------------|--------|----------------------|-----------------------|
//! | PC0 | | | | INTRB | INTRB |
//! | PC1 | | | | IBFB | /OBFB |
//! | PC2 | | | | /STBB (pin) | /ACKB (pin) |
//! | PC3 | INTRA | INTRA | INTRA | | |
//! | PC4 | /STBA (pin) | free | /STBA (pin) | | |
//! | PC5 | IBFA | free | IBFA | | |
//! | PC6 | free | /ACKA (pin) | /ACKA (pin) | | |
//! | PC7 | free | /OBFA | /OBFA | | |
//!
//! A write to the Port C *data* register only reaches the bits of a group
//! that is in mode 0 (the real chip, and the acid test, leave the others
//! alone: the free bits of a handshake group change only through bit
//! set/reset); bit set/reset on a handshake group's interrupt-enable bit
//! (PC4 input / PC6 output of group A, PC2 of group B) sets that INTE flag
//! instead of the pin latch. The flags: IBF sets when /STB goes low (the
//! input latch takes the port's pins) and clears when Port A/B is read;
//! /OBF goes low when Port A/B is written and returns high when /ACK goes
//! low; INTR is IBF with INTE and /STB high (input) or /OBF high with INTE
//! and /ACK high (output). Any mode-set write clears every latch and flag.

const CONTROL_MODE_SET: u8 = 0x80;
const CONTROL_PA_INPUT: u8 = 0x10;
const CONTROL_PCH_INPUT: u8 = 0x08;
const CONTROL_PB_MODE1: u8 = 0x04;
const CONTROL_PB_INPUT: u8 = 0x02;
const CONTROL_PCL_INPUT: u8 = 0x01;

/// Real 8255 power-up/reset default: mode 0, every port configured as
/// input (control byte `0x9B`) -- commonly documented reset behavior.
const DEFAULT_CONTROL: u8 = 0x9B;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Ppi {
    /// The last mode-set control word (a write with bit 7 set) -- what
    /// actually determines each port's direction and mode. A bit-set/reset
    /// write (bit 7 clear) manipulates `port_c` directly instead and never
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
    /// `keyboard_port_b` already use for the 6510/CIA1 ports. `input_c` is
    /// also the level of the /STB and /ACK pins in the handshake modes.
    input_a: u8,
    input_b: u8,
    input_c: u8,
    /// Handshake flags (group A, group B): input buffer full, /OBF (true =
    /// high = no byte waiting), and the interrupt-enable flip-flops (A input
    /// = PC4, A output = PC6, B = PC2).
    ibf_a: bool,
    obf_a_high: bool,
    ibf_b: bool,
    obf_b_high: bool,
    inte_a_in: bool,
    inte_a_out: bool,
    inte_b: bool,
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
            ibf_a: false,
            obf_a_high: true,
            ibf_b: false,
            obf_b_high: true,
            inte_a_in: false,
            inte_a_out: false,
            inte_b: false,
        }
    }
}

impl Ppi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Group A's mode: 0, 1 or 2 (control bits 6:5, `11` is also mode 2).
    fn mode_a(&self) -> u8 {
        match (self.control >> 5) & 0x03 {
            0 => 0,
            1 => 1,
            _ => 2,
        }
    }

    fn mode_b(&self) -> u8 {
        u8::from(self.control & CONTROL_PB_MODE1 != 0)
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

    fn pin(&self, bit: u8) -> bool {
        self.input_c & (1 << bit) != 0
    }

    /// Group A's INTR output.
    fn intr_a(&self) -> bool {
        match self.mode_a() {
            0 => false,
            1 if self.port_a_input() => self.inte_a_in && self.ibf_a && self.pin(4),
            1 => self.inte_a_out && self.obf_a_high && self.pin(6),
            _ => {
                (self.inte_a_out && self.obf_a_high && self.pin(6))
                    || (self.inte_a_in && self.ibf_a && self.pin(4))
            }
        }
    }

    fn intr_b(&self) -> bool {
        if self.mode_b() == 0 {
            return false;
        }
        if self.port_b_input() {
            self.inte_b && self.ibf_b && self.pin(2)
        } else {
            self.inte_b && self.obf_b_high && self.pin(2)
        }
    }

    /// Port C bits that are driven by a handshake flag (not by the latch, not a pin read back):
    /// returns `(mask, value)`.
    fn handshake_outputs(&self) -> (u8, u8) {
        let mut mask = 0u8;
        let mut value = 0u8;
        let mut put = |bit: u8, on: bool| {
            mask |= 1 << bit;
            if on {
                value |= 1 << bit;
            }
        };
        match self.mode_a() {
            0 => {}
            1 if self.port_a_input() => {
                put(3, self.intr_a());
                put(5, self.ibf_a);
            }
            1 => {
                put(3, self.intr_a());
                put(7, self.obf_a_high);
            }
            _ => {
                put(3, self.intr_a());
                put(5, self.ibf_a);
                put(7, self.obf_a_high);
            }
        }
        if self.mode_b() == 1 {
            put(0, self.intr_b());
            if self.port_b_input() {
                put(1, self.ibf_b);
            } else {
                put(1, self.obf_b_high);
            }
        }
        (mask, value)
    }

    /// Port C bits that are handshake *input* pins (/STB, /ACK): read back as the pin level.
    fn handshake_input_pins(&self) -> u8 {
        let mut mask = 0u8;
        match self.mode_a() {
            0 => {}
            1 if self.port_a_input() => mask |= 1 << 4,
            1 => mask |= 1 << 6,
            _ => mask |= (1 << 4) | (1 << 6),
        }
        if self.mode_b() == 1 {
            mask |= 1 << 2;
        }
        mask
    }

    /// Port C bits a data write can reach: those of a group in mode 0.
    fn port_c_writable_mask(&self) -> u8 {
        let a = if self.mode_a() == 0 { 0xF8 } else { 0 };
        let b = if self.mode_b() == 0 { 0x07 } else { 0 };
        a | b
    }

    /// Writes the control register. A byte with bit 7 set is a real
    /// mode-set (reconfigures every port's direction and mode, and -- since real
    /// hardware resets all outputs to `0` whenever the control register
    /// is reconfigured this way -- clears every port's own output
    /// latch and handshake flag too). A byte with bit 7 clear is the real "bit set/reset"
    /// sub-mode: bits 3:1 pick one of Port C's 8 bits, bit 0 sets (`1`)
    /// or clears (`0`) it, direction untouched; on a handshake group's
    /// interrupt-enable bit it sets that flag instead of the pin latch.
    pub fn write_control(&mut self, val: u8) {
        if val & CONTROL_MODE_SET != 0 {
            self.control = val;
            self.port_a = 0;
            self.port_b = 0;
            self.port_c = 0;
            self.ibf_a = false;
            self.obf_a_high = true;
            self.ibf_b = false;
            self.obf_b_high = true;
            self.inte_a_in = false;
            self.inte_a_out = false;
            self.inte_b = false;
        } else {
            let bit = (val >> 1) & 0x07;
            let on = val & 0x01 != 0;
            let (mode_a, mode_b) = (self.mode_a(), self.mode_b());
            let a_input = self.port_a_input();
            // The interrupt-enable flip-flops live on these Port C bits.
            let inte = match bit {
                4 if mode_a == 2 || (mode_a == 1 && a_input) => Some(&mut self.inte_a_in),
                6 if mode_a == 2 || (mode_a == 1 && !a_input) => Some(&mut self.inte_a_out),
                2 if mode_b == 1 => Some(&mut self.inte_b),
                _ => None,
            };
            if let Some(flag) = inte {
                *flag = on;
                return;
            }
            // Pins that are status outputs or strobe inputs have no latch to set.
            let (status_mask, _) = self.handshake_outputs();
            if status_mask & (1 << bit) != 0 || self.handshake_input_pins() & (1 << bit) != 0 {
                return;
            }
            if on {
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
        let mode = self.mode_a();
        if mode == 2 || (mode == 1 && !self.port_a_input()) {
            self.obf_a_high = false;
        }
    }

    pub fn write_port_b(&mut self, val: u8) {
        self.port_b = val;
        if self.mode_b() == 1 && !self.port_b_input() {
            self.obf_b_high = false;
        }
    }

    /// A Port C data write: reaches only the bits of a group in mode 0.
    pub fn write_port_c(&mut self, val: u8) {
        let mask = self.port_c_writable_mask();
        self.port_c = (self.port_c & !mask) | (val & mask);
    }

    /// Whether the 8255 drives the Port A pins (an output in mode 0/1; in mode 2 only while /ACK is low,
    /// which a host that leaves the pin unconnected never does). An input port leaves the bus to
    /// whatever else is on it.
    pub fn port_a_pins_driven(&self) -> bool {
        match self.mode_a() {
            0 | 1 => !self.port_a_input(),
            _ => false,
        }
    }

    /// What a read of Port A returns; the side effect (a strobed input's IBF is cleared) is
    /// `read_port_a_clearing`.
    pub fn read_port_a(&self) -> u8 {
        if self.port_a_input() || self.mode_a() == 2 {
            self.input_a
        } else {
            self.port_a
        }
    }

    pub fn read_port_a_clearing(&mut self) -> u8 {
        let value = self.read_port_a();
        let mode = self.mode_a();
        if mode == 2 || (mode == 1 && self.port_a_input()) {
            self.ibf_a = false;
        }
        value
    }

    pub fn read_port_b(&self) -> u8 {
        if self.port_b_input() {
            self.input_b
        } else {
            self.port_b
        }
    }

    pub fn read_port_b_clearing(&mut self) -> u8 {
        let value = self.read_port_b();
        if self.mode_b() == 1 && self.port_b_input() {
            self.ibf_b = false;
        }
        value
    }

    pub fn read_port_c(&self) -> u8 {
        let input_mask = self.port_c_input_mask();
        // Mode 0 bits: an input bit shows its pin, an output bit its latch.
        let mut value = (self.input_c & input_mask) | (self.port_c & !input_mask);
        // Handshake pins: the strobe/acknowledge inputs show their pin level, the status bits their flag.
        let pins = self.handshake_input_pins();
        value = (value & !pins) | (self.input_c & pins);
        let (mask, status) = self.handshake_outputs();
        (value & !mask) | status
    }

    pub fn set_input_a(&mut self, val: u8) {
        self.input_a = val;
    }

    pub fn set_input_b(&mut self, val: u8) {
        self.input_b = val;
    }

    /// The Port B pin levels the outside world drives, whatever the port's
    /// direction (`read_port_b` gives the output latch while it is an output).
    pub fn input_b(&self) -> u8 {
        self.input_b
    }

    /// Sets the level of the Port C pins that are inputs (and of /STB and /ACK in the handshake
    /// modes); a falling edge of /STB latches the data and sets IBF, a falling edge of /ACK sets /OBF
    /// high again.
    pub fn set_input_c(&mut self, val: u8) {
        let falling = self.input_c & !val;
        self.input_c = val;
        let (mode_a, mode_b) = (self.mode_a(), self.mode_b());
        if falling & (1 << 4) != 0 && (mode_a == 2 || (mode_a == 1 && self.port_a_input())) {
            self.ibf_a = true;
        }
        if falling & (1 << 6) != 0 && (mode_a == 2 || (mode_a == 1 && !self.port_a_input())) {
            self.obf_a_high = true;
        }
        if falling & (1 << 2) != 0 && mode_b == 1 {
            if self.port_b_input() {
                self.ibf_b = true;
            } else {
                self.obf_b_high = true;
            }
        }
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

    /// The CPC acid test's mode 1 / mode 2 expectations: a data write reaches only the mode 0 group B bits, the free bits
    /// of group A change through bit set/reset only.
    #[test]
    fn in_mode_1_a_port_c_data_write_reaches_only_the_group_b_bits() {
        let mut ppi = Ppi::new();
        ppi.write_control(0xB0); // A mode 1 input, PC upper output, B mode 0 output, PC lower output
        ppi.write_port_c(0xFF);
        assert_eq!(
            ppi.read_port_c() & 0xC7,
            0x07,
            "PC7/PC6 untouched, PC2-0 written"
        );
        ppi.write_control(0x0D); // set PC6 (bit set/reset)
        assert_eq!(ppi.read_port_c() & 0xC0, 0x40);
        ppi.write_port_c(0x00); // a data write cannot clear it
        assert_eq!(ppi.read_port_c() & 0xC0, 0x40);
        ppi.write_control(0x0C); // clear PC6
        assert_eq!(ppi.read_port_c() & 0xC0, 0x00);
    }

    #[test]
    fn mode_1_output_free_bits_and_status_flags() {
        let mut ppi = Ppi::new();
        ppi.write_control(0xA0); // A mode 1 output, PC upper output
        assert_eq!(
            ppi.read_port_c() & 0x80,
            0x80,
            "/OBF is high until a byte is written"
        );
        ppi.write_port_a(0x12);
        assert_eq!(
            ppi.read_port_c() & 0x80,
            0x00,
            "/OBF goes low after a write"
        );
        ppi.set_input_c(0x40);
        ppi.set_input_c(0x00); // /ACK falls
        assert_eq!(
            ppi.read_port_c() & 0x80,
            0x80,
            "an acknowledge sets /OBF high again"
        );
        // INTR: /OBF high with INTE (PC6) set and /ACK high
        ppi.set_input_c(0x40);
        ppi.write_control(0x0D);
        assert_eq!(ppi.read_port_c() & 0x08, 0x08);
    }

    #[test]
    fn mode_1_input_strobe_sets_ibf_and_reading_port_a_clears_it() {
        let mut ppi = Ppi::new();
        ppi.write_control(0xB0);
        ppi.set_input_c(0x10); // /STB high
        ppi.set_input_a(0x5A);
        ppi.set_input_c(0x00); // /STB falls: data latched, IBF set
        assert_eq!(ppi.read_port_c() & 0x20, 0x20);
        assert_eq!(ppi.read_port_a_clearing(), 0x5A);
        assert_eq!(ppi.read_port_c() & 0x20, 0x00);
    }

    #[test]
    fn mode_2_port_a_pins_are_not_driven_and_port_c_shows_the_handshake_bits() {
        let mut ppi = Ppi::new();
        ppi.write_control(0xC0);
        assert!(!ppi.port_a_pins_driven());
        assert_eq!(ppi.read_port_c() & 0xA0, 0x80, "/OBF high, IBF low");
    }
}
