//! Amstrad CPC 464/664/6128 machine implementation: model-specific ROM,
//! 64K/128K RAM and paging, optional uPD765A floppy controller, Gate Array,
//! register-driven 6845 CRTC timing, Intel 8255 PPI, AY-3-8910 audio, the
//! real keyboard matrix, and cassette input/motor wiring.

use machine::{Bus, Machine, RunStats};

use crate::{Model, TapeMotorMode};

pub const ROM_SIZE: usize = 0x4000;
pub const ROM_IMAGE_SIZE_464: usize = ROM_SIZE * 2;
pub const ROM_IMAGE_SIZE_664_6128: usize = ROM_SIZE * 3;
/// GX4000 cartridges are real, physical 16K-bank EPROM images, up to 8
/// banks (128K) -- the ASIC's own `rmr2 & 0x07` bank-select field is 3
/// bits wide, so no real cartridge can exceed this.
pub const MAX_CARTRIDGE_BANKS: usize = 8;

/// One of the 2 real analogue joystick ports on CPC Plus/GX4000 (see
/// `crate::asic`). No meaning on a classic model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogJoystick {
    First,
    Second,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogAxis {
    X,
    Y,
}

/// Real 6-bit ADC range for `Cpc::set_analog_axis`.
pub const ANALOG_MIN: u8 = 0x00;
pub const ANALOG_MAX: u8 = 0x3F;
/// Centre (released) value -- see `crate::asic::Asic`'s own doc comment
/// on why this, not CPCTech's measured "nothing attached" `0x3F`, is the
/// idle default.
pub const ANALOG_CENTER: u8 = 0x1F;

const PRIMARY_RAM_SIZE: usize = 0x10000;

/// Physical 16K RAM bank visible in each Z80 window for the eight CPC6128
/// paging configurations, verbatim from the hardware table cited in
/// `CREDITS.md`.
const RAM_CONFIGURATIONS: [[usize; 4]; 8] = [
    [0, 1, 2, 3],
    [0, 1, 2, 7],
    [4, 5, 6, 7],
    [0, 3, 2, 7],
    [0, 4, 2, 3],
    [0, 5, 2, 3],
    [0, 6, 2, 3],
    [0, 7, 2, 3],
];

/// Silence put in front of every cassette image by default: CPCEC opens each
/// TZX/CDT with a 2000 ms hold (`tape_safetydelay`), and the firmware itself
/// spends ~2.4 s after motor-on before it samples (`200 * 0x0682` loop
/// iterations at ROM `0x2A72`), so an image that starts straight on its first
/// pilot -- Arkanoid, Killapede, Dan Dare, Joe Blade 2 all do -- would have
/// its first block mostly played before the firmware listens. `0` disables it.
pub const DEFAULT_TAPE_LEADER_MS: u32 = 2000;

const CLOCK_HZ: u64 = 4_000_000;
const FRAME_HZ: u64 = 50;
/// `.cdt`/`.tzx` pulse timings are represented in the same cycle domain
/// `zx_tape` already uses when parsing blocks (`3_546_900` cycles/second).
/// Keep CPC->tape conversion in that exact domain to avoid long-run drift.
const ZX_TAPE_CLOCK_HZ: u64 = 3_546_900;

/// Overlay memory-activity granularity: one bucket per 4KB of CPU address
/// space (16 buckets total for 64KB).
const MEMORY_ACTIVITY_BLOCK_SIZE: usize = 0x1000;
const MEMORY_ACTIVITY_BLOCKS: usize = 0x10000 / MEMORY_ACTIVITY_BLOCK_SIZE;

/// Recent EAR-line history samples (`0` low / `1` high) for the tape
/// overlay's waveform bar -- sampled at a fixed `zx-tape`-domain cycle
/// interval (`TAPE_WAVEFORM_SAMPLE_INTERVAL_CYCLES`), not once per CPU
/// instruction, so consecutive characters represent equal real time and
/// a run of repeated characters' length is directly comparable to real
/// pulse widths -- a once-per-instruction sample rate would make the bar
/// meaningless for spotting pulse-width problems, since instructions take
/// a variable number of T-states each.
const TAPE_WAVEFORM_SAMPLES: usize = 128;
/// One tape-domain sample roughly every 1/9th of a real bit-0 pulse
/// (855 T-states) -- enough resolution that a genuine pulse shows up as
/// several repeated bar characters, not a single ambiguous one.
const TAPE_WAVEFORM_SAMPLE_INTERVAL_CYCLES: u32 = 96;
/// Recent completed EAR-line pulse widths, in tape-domain T-states, for
/// the tape overlay's numeric pulse list -- the actual diagnostic signal
/// for "is this tape decoding real bits" (compare against the standard
/// ROM protocol's own known-good widths: pilot ~2168T, sync ~667/735T,
/// bit0 ~855T, bit1 ~1710T, all cited in this crate's own `CLAUDE.md`).
const TAPE_PULSE_WIDTH_SAMPLES: usize = 16;
const CPC_PORT_B_FIXED_HIGH_MASK: u8 = 0x1E;
const CPC_PORT_B_FIXED_LOW_MASK: u8 = 0x60;

/// Debug-overlay telemetry snapshot exported through the CPC machine plugin.
#[derive(Debug, Clone)]
pub struct OverlaySnapshot {
    pub ram_bytes: usize,
    pub rom_bytes: usize,
    pub map_0000: String,
    pub map_4000: String,
    pub map_8000: String,
    pub map_c000: String,
    pub memory_reads_total: u64,
    pub memory_writes_total: u64,
    pub memory_read_blocks: Vec<u64>,
    pub memory_write_blocks: Vec<u64>,
    pub io_reads_total: u64,
    pub io_writes_total: u64,
    pub ppi_control: u8,
    pub ppi_port_a: u8,
    pub ppi_port_b: u8,
    pub ppi_port_b_raw: u8,
    pub ppi_port_c: u8,
    pub keyboard_row_select: u8,
    pub psg_selected_register: u8,
    pub upper_rom_select: u8,
    pub ram_configuration: u8,
    pub gate_mode: u8,
    pub tape_port_reads: u64,
    pub tape_control_writes: u64,
    pub tape_motor_toggles: u64,
    pub tape_waveform: Vec<u8>,
    pub tape_pulse_widths: Vec<u16>,
    pub disk_motor: bool,
    pub disk_msr_reads: u64,
    pub disk_data_reads: u64,
    pub disk_data_writes: u64,
    pub disk_motor_writes: u64,
    pub fdc_cylinder: Option<u8>,
    pub fdc_phase: Option<String>,
}

/// A write that changes the picture (or the CRTC's counters), stamped with
/// the T-state -- counted from the start of the instruction that made it --
/// at which its bus cycle completed. `Cpc::step` replays these in order
/// while ticking the CRTC so each takes effect on the character clock it
/// really landed on, not at the instruction boundary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimedVideoEvent {
    pub t: u32,
    pub kind: VideoEventKind,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum VideoEventKind {
    CrtcSelect(u8),
    CrtcWrite(u8),
    /// Gate Array mode request (latched on the next HSYNC end).
    RequestMode(u8),
    Ink {
        pen: u8,
        rgb: crate::raster::Rgb,
    },
    HScroll(u8),
}

#[derive(Clone)]
pub struct CpcBus {
    model: Model,
    ram: Vec<u8>,
    lower_rom: Vec<u8>,
    basic_rom: Vec<u8>,
    amsdos_rom: Option<Vec<u8>>,
    /// GX4000's mandatory cartridge image (and, for Cpc464Plus/
    /// Cpc6128Plus, an optional attached cartridge -- not wired to any
    /// attach path yet, so always empty for those two models this
    /// session), sliced into `ROM_SIZE`-byte banks. Empty for every
    /// classic model, which always uses `lower_rom`/`basic_rom`/
    /// `amsdos_rom` instead.
    cartridge_banks: Vec<Vec<u8>>,
    /// Standard Memory Expansion ROM slots 1-6/8-15 (slot 0 is always
    /// BASIC, slot 7 is always AMSDOS on FDC-equipped models -- neither
    /// is stored here). `None` = unpopulated, falls back to BASIC, same
    /// as real hardware with no expansion board fitted in that slot.
    expansion_roms: [Option<Vec<u8>>; 16],
    /// `Some` only for `Model::has_asic()` models -- see `crate::asic`.
    asic: Option<crate::asic::Asic>,
    upper_rom_select: u8,
    ram_configuration: u8,
    /// Register 3 bits 5:3 -- the 64K expansion bank currently selected
    /// (0-7). Real hardware fact, verified against ZEsarUX's own
    /// `cpc_set_memory_pages` comment (see `CREDITS.md`): only
    /// meaningful once `additional_ram_banks() > 0`.
    ram_bank_select: u8,
    /// How many 64K "Standard Memory Expansion" banks beyond the base
    /// 6128's own onboard second 64K are physically present (0-7, so
    /// total banks 1-8, matching Caprice32's documented 576K maximum:
    /// 64K base + 8*64K = 576K). `0` (the default) reproduces every
    /// existing 128K-paging model's original behavior exactly.
    additional_ram_banks: u8,

    pub(crate) gate_array: crate::gate_array::GateArray,
    pub(crate) crtc: crate::crtc::Crtc,
    ppi: i8255::Ppi,
    pub(crate) ay: ay_3_8910::Ay3_8910,
    fdc: Option<nec765::Fdc<dsk::Disk>>,
    /// The disk exactly as it was inserted, so a save state that predates
    /// any write can put the medium back (see `restore_disk_image`).
    pristine_disks: [Option<dsk::Disk>; nec765::DRIVES],
    disk_motor: bool,
    /// Loaded cassette tape (`.cdt`/`.tzx`) if any.
    tape: Option<zx_tape::TapePlayer>,
    /// Port C bit 4, the CPC cassette motor control line.
    cassette_motor: bool,
    /// Selects Caprice-strict or ZEsarUX-compatible cassette transport.
    tape_motor_mode: TapeMotorMode,
    /// Silence prepended to every tape attached with `load_tape` (see
    /// [`DEFAULT_TAPE_LEADER_MS`]).
    tape_leader_ms: u32,
    /// Fractional remainder while converting CPC 4MHz T-states to
    /// `zx-tape`'s own 3.5469MHz cycle domain.
    tape_cycle_debt: u64,
    /// Mirrors what the bus itself last told the AY to select, since
    /// `ay_3_8910::Ay3_8910` has no public "currently selected register"
    /// getter -- needed to know when a PSG "read" function targets
    /// register 14 (the real keyboard-row read-back register on CPC).
    psg_selected_register: u8,
    /// Port C bits 3:0 -- real keyboard row select (a 74LS145 decoder
    /// picks 1 of 10 real matrix lines, see `crate::keyboard`'s own doc
    /// comment; 4 bits, not 3 -- 10 lines need the full nibble).
    keyboard_row_select: u8,
    pub(crate) keyboard: crate::keyboard::KeyMatrix,
    /// Cumulative memory-bus activity (CPU-side only), bucketed by 4KB.
    memory_reads_total: u64,
    memory_writes_total: u64,
    memory_read_blocks: [u64; MEMORY_ACTIVITY_BLOCKS],
    memory_write_blocks: [u64; MEMORY_ACTIVITY_BLOCKS],
    /// Cumulative I/O-port activity (CPU-side `IN`/`OUT`).
    io_reads_total: u64,
    io_writes_total: u64,
    /// Tape I/O diagnostics.
    tape_port_reads: u64,
    tape_control_writes: u64,
    tape_motor_toggles: u64,
    tape_waveform: [u8; TAPE_WAVEFORM_SAMPLES],
    tape_waveform_cursor: usize,
    tape_waveform_len: usize,
    /// Cycles accumulated toward the next `TAPE_WAVEFORM_SAMPLE_INTERVAL_CYCLES`
    /// waveform-bar sample.
    tape_waveform_sample_debt: u32,
    /// Recent completed pulse widths (T-states) -- see
    /// `TAPE_PULSE_WIDTH_SAMPLES`'s own doc comment.
    tape_pulse_widths: [u16; TAPE_PULSE_WIDTH_SAMPLES],
    tape_pulse_widths_cursor: usize,
    tape_pulse_widths_len: usize,
    /// Tape-domain T-states elapsed since the EAR line last changed level
    /// -- accumulates across `tick_tape` calls; pushed onto
    /// `tape_pulse_widths` and reset to `0` the instant the level flips.
    tape_cycles_since_edge: u32,
    /// Disk I/O diagnostics.
    disk_msr_reads: u64,
    disk_data_reads: u64,
    disk_data_writes: u64,
    disk_motor_writes: u64,
    /// Current CPU T-state phase inside the Gate Array's four-T-state RAM
    /// arbitration slot. Every CPU memory access waits until phase zero.
    bus_tstate_phase: u8,
    /// T-states already reported through `Bus::advance_tstates` during the
    /// current instruction. Reconciled with the core's full instruction cost
    /// afterward so internal (non-bus) cycles advance the phase too.
    instruction_advanced_tstates: u32,
    /// Absolute T-states executed so far (whole instructions), the clock the
    /// tape recorder timestamps MIC edges against.
    cycles_total: u64,
    /// Debugger port breakpoints (see `machine::PortWatcher`); not saved in states.
    pub(crate) port_watch: machine::PortWatcher,
    /// Optional AMX-style mouse on the joystick-0 port (matrix line 9); see
    /// `crate::amx_mouse`. Transient host input, not saved in save states.
    amx_mouse: crate::amx_mouse::AmxMouse,
    /// The cassette output line (PPI Port C bit 5) recorded as pulses while
    /// the motor runs -- see `crate::tape_recorder`.
    tape_recorder: crate::tape_recorder::TapeRecorder,
    /// Picture-affecting writes made since `Cpc::step` last drained them.
    video_events: Vec<TimedVideoEvent>,
    /// The CRTC exactly as it was before the first register write of the
    /// current instruction. Writes take effect on `crtc` immediately (so
    /// reads within the same instruction see them); `Cpc::step` restores
    /// this copy and re-applies the writes at their real character clock
    /// while ticking.
    crtc_replay_base: Option<crate::crtc::Crtc>,
}

impl CpcBus {
    /// `image`: the classic-shaped combined firmware ROM for every model
    /// with `Model::has_internal_rom()`, or (GX4000 only) the mandatory
    /// cartridge image, already size-validated by `Cpc::new`.
    fn new(image: Vec<u8>, model: Model) -> Self {
        let (lower_rom, basic_rom, amsdos_rom, cartridge_banks) = if model.has_internal_rom() {
            let lower = image[..ROM_SIZE].to_vec();
            let basic = image[ROM_SIZE..ROM_SIZE * 2].to_vec();
            let amsdos = model
                .has_fdc()
                .then(|| image[ROM_SIZE * 2..ROM_SIZE * 3].to_vec());
            (lower, basic, amsdos, Vec::new())
        } else {
            // GX4000: no internal firmware at all -- the image itself is
            // the mandatory cartridge, sliced into ROM_SIZE-byte banks.
            let banks: Vec<Vec<u8>> = image.chunks(ROM_SIZE).map(<[u8]>::to_vec).collect();
            (Vec::new(), Vec::new(), None, banks)
        };
        let mut bus = Self {
            model,
            ram: vec![0; model.ram_size()],
            lower_rom,
            basic_rom,
            amsdos_rom,
            cartridge_banks,
            expansion_roms: std::array::from_fn(|_| None),
            asic: model.has_asic().then(crate::asic::Asic::new),
            upper_rom_select: 0,
            ram_configuration: 0,
            ram_bank_select: 0,
            additional_ram_banks: 0,
            gate_array: crate::gate_array::GateArray::new(),
            crtc: crate::crtc::Crtc::with_type(model.default_crtc_type()),
            ppi: i8255::Ppi::new(),
            ay: ay_3_8910::Ay3_8910::new(),
            fdc: model.has_fdc().then(|| {
                let mut fdc = nec765::Fdc::new();
                fdc.set_motor(false);
                // CPC (unlike this crate's other host, zx_spectrum's +3)
                // drives `tick_fdc` every step, so real bounded
                // Seek/Recalibrate timing is safe to enable here.
                fdc.enable_seek_timing();
                fdc
            }),
            pristine_disks: [None, None],
            disk_motor: false,
            tape: None,
            cassette_motor: false,
            tape_motor_mode: TapeMotorMode::default(),
            tape_leader_ms: DEFAULT_TAPE_LEADER_MS,
            tape_cycle_debt: 0,
            psg_selected_register: 0,
            keyboard_row_select: 0,
            keyboard: crate::keyboard::KeyMatrix::new(),
            memory_reads_total: 0,
            memory_writes_total: 0,
            memory_read_blocks: [0; MEMORY_ACTIVITY_BLOCKS],
            memory_write_blocks: [0; MEMORY_ACTIVITY_BLOCKS],
            io_reads_total: 0,
            io_writes_total: 0,
            tape_port_reads: 0,
            tape_control_writes: 0,
            tape_motor_toggles: 0,
            tape_waveform: [0; TAPE_WAVEFORM_SAMPLES],
            tape_waveform_cursor: 0,
            tape_waveform_len: 0,
            tape_waveform_sample_debt: 0,
            tape_pulse_widths: [0; TAPE_PULSE_WIDTH_SAMPLES],
            tape_pulse_widths_cursor: 0,
            tape_pulse_widths_len: 0,
            tape_cycles_since_edge: 0,
            disk_msr_reads: 0,
            disk_data_reads: 0,
            disk_data_writes: 0,
            disk_motor_writes: 0,
            bus_tstate_phase: 0,
            instruction_advanced_tstates: 0,
            cycles_total: 0,
            port_watch: machine::PortWatcher::default(),
            amx_mouse: crate::amx_mouse::AmxMouse::default(),
            tape_recorder: crate::tape_recorder::TapeRecorder::default(),
            video_events: Vec::new(),
            crtc_replay_base: None,
        };
        // Port C's pins when the 8255 leaves them as inputs: the keyboard-row lines and the cassette
        // write-data line read high, the motor and the AY's BC1/BDIR lines low (the level the CPC
        // acid test `ppi` expects of a CPC, `port_c_input = &2F`).
        bus.ppi.set_input_c(0x2F);
        bus.refresh_cassette_input();
        bus
    }

    /// Port C carries the AY's real BDIR/BC1 function-select bits
    /// (7:6) alongside the keyboard row select (3:0) -- real CPC
    /// hardware has no direct AY register/data ports at all (unlike ZX
    /// Spectrum 128K's `0xfffd`/`0xbffd`); every PSG access is this
    /// same real sequence real firmware uses: write a value to Port A,
    /// then write Port C with BDIR/BC1 set to the wanted function
    /// (`11`=select register, `10`=write data, `01`=read data), which is
    /// what actually triggers the AY operation using whatever's
    /// currently on Port A. A `01` (read) response is fed back into the
    /// PPI's own Port A *input* latch (`set_input_a`) rather than
    /// answered directly here, so a later Port A `IN` (with Port A
    /// correctly configured as PPI input by firmware at that point)
    /// reads it back through the PPI's own existing input/output
    /// mechanism with no special-casing needed on the read path at all.
    fn write_port_c(&mut self, val: u8) {
        self.ppi.write_port_c(val);
        self.keyboard_row_select = val & 0x0F;
        self.note_cassette_output();

        // The cassette relay hangs off Port C bit 4, so a data write to Port C
        // switches the motor exactly like a bit-set/reset command on that bit
        // (Caprice32's `z80_OUT_handler` sets `CPC.tape_motor = val & 0x10` on a
        // Port C data write, CPCEC defines `tape_enabled` as `pio_port_c & 16`).
        // Real firmware and loaders preserve bit 4 when they drive Port C for
        // the AY/keyboard handshake (they read Port C back and only change the
        // other bits); one that deliberately writes it high -- the ROM's own
        // cassette start-up does, and so do custom loaders such as Arkanoid's
        // SpeedLock stage -- must start the tape. This was previously ignored
        // (motor only from bit-set/reset), which left such a loader waiting
        // forever for pulses from a stopped tape.
        let motor_on = val & 0x10 != 0;
        if self.ppi.control() & 0x08 == 0 && motor_on != self.cassette_motor {
            self.set_cassette_motor(motor_on);
        }
        self.psg_function(self.ppi.port_c_latch());
    }

    /// What the AY sees on its data bus: Port A's output latch while the 8255 drives the pins, otherwise
    /// the undriven bus, which floats high (the CPC acid test `ppi`: "mode 0: ppi port a output").
    fn psg_bus_value(&self) -> u8 {
        if self.ppi.port_a_pins_driven() {
            self.ppi.port_a_latch()
        } else {
            0xFF
        }
    }

    /// The AY operation Port C's BDIR/BC1 lines (7:6) select, performed as the function is entered:
    /// `11` select register, `10` write data, `01` read data, `00` inactive.
    fn psg_function(&mut self, port_c: u8) {
        match (port_c >> 6) & 0x03 {
            0b01 => {
                // Register 14 is the real keyboard-row read-back
                // register on CPC -- any other register reads the AY
                // normally.
                let v = if self.ay.register_valid() && self.ay.selected_register() == 14 {
                    // Port A: the keyboard line pulls bits low; an output port shows its latch on the pins
                    // (wired-AND with the keys), an input port only the keys (the acid test `psg`).
                    let keys = self.keyboard_line(self.keyboard_row_select);
                    if self.ay.io_port_output(0) {
                        self.ay.io_port_latch(0) & keys
                    } else {
                        keys
                    }
                } else {
                    self.ay.read()
                };
                self.ppi.set_input_a(v);
            }
            0b10 => {
                let v = self.psg_bus_value();
                self.ay.write(v);
            }
            0b11 => {
                let v = self.psg_bus_value();
                self.ay.select_register(v);
                self.psg_selected_register = v & 0x0F;
            }
            _ => {} // 00 = inactive
        }
    }

    /// Pure read with no side effects -- for `Machine::read_memory`.
    pub(crate) fn peek(&self, addr: u16) -> u8 {
        if let Some(byte) = self.asic_rom_overlay(addr) {
            return byte;
        }
        match addr {
            0x0000..=0x3FFF if !self.gate_array.lower_rom_disabled() => {
                if self.cartridge_banks.is_empty() {
                    self.lower_rom[addr as usize]
                } else {
                    // Real hardware default: cartridge bank 0 at &0000,
                    // whether the ASIC is locked or unlocked-but-not-
                    // relocated-away-from-here (see `asic_rom_overlay`).
                    self.cartridge_banks[0][addr as usize]
                }
            }
            0xC000..=0xFFFF if !self.gate_array.upper_rom_disabled() => {
                self.selected_upper_rom_byte((addr - 0xC000) as usize)
            }
            _ => self.ram[self.ram_index(addr)],
        }
    }

    /// CPC Plus/GX4000 ASIC memory-map overlay for `0x4000-0xBFFF`: the
    /// ASIC register window (`0x4000-0x7FFF` once RMR2 selects it) and
    /// the RMR2 lower-ROM-relocation feature (repositioning a cartridge
    /// bank to `0x4000` or `0x8000` instead of its default `0x0000`).
    /// Returns `None` when neither applies (including every classic,
    /// non-ASIC model), so the caller falls through to plain RAM/ROM --
    /// this only ever affects `Model::has_asic()` models. Gated the same
    /// way real hardware gates it: behind the classic lower-ROM-enable
    /// bit (see `CREDITS.md`).
    fn asic_rom_overlay(&self, addr: u16) -> Option<u8> {
        let asic = self.asic.as_ref()?;
        if self.gate_array.lower_rom_disabled() || !asic.active() {
            return None;
        }
        let window = addr / ROM_SIZE as u16;
        let position = asic.rmr2() & 0x18;
        if window == 1 && position == 0x18 {
            return Some(asic.read_window((addr - 0x4000) as usize));
        }
        if self.cartridge_banks.is_empty() {
            return None;
        }
        let bank =
            &self.cartridge_banks[(asic.rmr2() & 0x07) as usize % self.cartridge_banks.len()];
        match (window, position) {
            (0, 0x00) | (0, 0x18) => Some(bank[(addr & 0x3FFF) as usize]),
            (1, 0x08) => Some(bank[(addr - 0x4000) as usize]),
            (2, 0x10) => Some(bank[(addr - 0x8000) as usize]),
            _ => None,
        }
    }

    /// Routes a memory write into the ASIC's own register storage when
    /// the ASIC register window is currently visible at `0x4000-0x7FFF`
    /// (`asic.active() && rmr2 bits 4:3 == 0x18`, gated the same way
    /// reads are -- see `asic_rom_overlay`'s own doc comment). Returns
    /// whether the write was consumed this way (the caller must not also
    /// write it into system RAM when `true`).
    fn asic_register_write(&mut self, addr: u16, val: u8) -> bool {
        let lower_rom_disabled = self.gate_array.lower_rom_disabled();
        let Some(asic) = self.asic.as_mut() else {
            return false;
        };
        if lower_rom_disabled || !asic.active() || (asic.rmr2() & 0x18) != 0x18 {
            return false;
        }
        match addr {
            0x4000..=0x5FFF => {
                asic.write_low((addr - 0x4000) as usize, val);
                true
            }
            0x6000..=0x7FFF => {
                let offset = (addr - 0x6000) as usize;
                asic.write_high(offset, val);
                let effect = asic.video_effect_of_high_write(offset);
                match effect {
                    Some(crate::asic::VideoWrite::Ink { pen, rgb }) => {
                        self.log_video_event(VideoEventKind::Ink {
                            pen,
                            rgb: [rgb.0, rgb.1, rgb.2],
                        });
                    }
                    Some(crate::asic::VideoWrite::HScroll(scroll)) => {
                        self.log_video_event(VideoEventKind::HScroll(scroll));
                    }
                    None => {}
                }
                true
            }
            _ => false,
        }
    }

    /// Records a picture-affecting write at the T-state its bus cycle ended
    /// on (see [`TimedVideoEvent`]).
    fn log_video_event(&mut self, kind: VideoEventKind) {
        self.video_events.push(TimedVideoEvent {
            t: self.instruction_advanced_tstates,
            kind,
        });
    }

    /// Absolute T-state of the bus cycle in progress.
    fn now(&self) -> u64 {
        self.cycles_total + u64::from(self.instruction_advanced_tstates)
    }

    /// Feeds the cassette recorder after any write that may have moved the
    /// MIC line (Port C bit 5).
    fn note_cassette_output(&mut self) {
        let mic = self.ppi.read_port_c() & 0x20 != 0;
        let now = self.now();
        self.tape_recorder.mic(now, mic);
    }

    /// A CRTC register-file write. Applied at once, and logged so the
    /// instruction's CRTC ticks can be replayed with it landing on the
    /// right character clock.
    fn crtc_write(&mut self, kind: VideoEventKind) {
        if self.crtc_replay_base.is_none() {
            self.crtc_replay_base = Some(self.crtc.clone());
        }
        match kind {
            VideoEventKind::CrtcSelect(value) => self.crtc.select_register(value),
            VideoEventKind::CrtcWrite(value) => self.crtc.write_register(value),
            _ => unreachable!("only CRTC register writes are routed here"),
        }
        self.log_video_event(kind);
    }

    /// Gate Array picture state as it stands right now: the trace's
    /// starting point, and its resynchronisation point after anything that
    /// replaces the Gate Array behind its back (state/snapshot loads).
    pub(crate) fn video_state(&self) -> crate::raster::VideoState {
        let inks = std::array::from_fn(|pen| {
            let (r, g, b) = match &self.asic {
                Some(asic) => asic.pen_color(pen as u8),
                None => self.gate_array.pen_color(pen as u8),
            };
            [r, g, b]
        });
        crate::raster::VideoState::new(
            self.gate_array.screen_mode(),
            self.asic.as_ref().map_or(0, |asic| asic.h_scroll()),
            inks,
        )
    }

    /// Whether the CPC Plus/GX4000 Programmable Raster Interrupt is
    /// currently active -- real hardware fact: "CPC style interrupts and
    /// Programmable Raster Interrupts cannot be active at the same time"
    /// (see `CREDITS.md`), so the caller must suppress the classic
    /// hsync-counter interrupt assertion (its own bookkeeping still runs
    /// unconditionally) while this is `true`.
    fn asic_pri_active(&self) -> bool {
        self.asic
            .as_ref()
            .is_some_and(|asic| asic.active() && asic.pri() != 0)
    }

    /// Compares the real CRTC line/raster counters against the
    /// programmed PRI line (see `crate::asic`'s own doc comment for the
    /// exact hardware-sourced formula) and fires it if they match.
    /// Returns whether it fired this call.
    fn asic_check_pri(&mut self) -> bool {
        let CpcBus {
            asic,
            crtc,
            gate_array,
            ..
        } = self;
        let Some(asic) = asic.as_mut() else {
            return false;
        };
        if !asic.active() || asic.pri() == 0 {
            return false;
        }
        let pri = asic.pri();
        if crtc.vertical_counter() == (pri >> 3) & 0x1F && crtc.raster_counter() == (pri & 0x07) {
            asic.trigger_raster();
            gate_array.reset_top_bit();
            true
        } else {
            false
        }
    }

    /// Runs one DMA instruction step per enabled channel (see
    /// `asic::Asic::step_dma`'s own doc comment), applying any resulting
    /// PSG register write directly (restoring whatever register the CPU
    /// itself had selected via Port C, matching real hardware -- DMA
    /// sound must not disturb the CPU's own AY addressing). Returns the
    /// raw per-channel results so the caller can request CPU interrupts
    /// for any that fired one, or `None` when there's no ASIC or it's
    /// locked.
    fn step_asic_dma(&mut self) -> Option<[crate::asic::DmaResult; 3]> {
        let CpcBus {
            asic,
            ram,
            ay,
            psg_selected_register,
            ..
        } = self;
        let asic = asic.as_mut()?;
        if !asic.active() {
            return None;
        }
        let results = asic.step_dma(&ram[..PRIMARY_RAM_SIZE]);
        for result in &results {
            if let Some((register, data)) = result.psg_load {
                let previous = *psg_selected_register;
                ay.select_register(register);
                ay.write(data);
                ay.select_register(previous);
            }
        }
        Some(results)
    }

    /// Whether a DMA-caused `/INT` is still held asserted, awaiting
    /// explicit DCSR acknowledgment -- see
    /// `asic::Asic::irq_unacknowledged`'s own doc comment.
    fn asic_irq_held(&self) -> bool {
        self.asic
            .as_ref()
            .is_some_and(crate::asic::Asic::irq_unacknowledged)
    }

    /// Upper ROM (`0xC000-0xFFFF`) source byte at `offset` (already
    /// relative to `0xC000`): the cartridge bank `upper_rom_select`
    /// picks (falling back to bank 0 for an out-of-range slot, same
    /// "unpopulated slot falls back to the default" convention as
    /// classic `selected_upper_rom`) for GX4000, or the classic BASIC/
    /// AMSDOS selection for every other model.
    fn selected_upper_rom_byte(&self, offset: usize) -> u8 {
        if self.cartridge_banks.is_empty() {
            return self.selected_upper_rom()[offset];
        }
        let idx = if (self.upper_rom_select as usize) < self.cartridge_banks.len() {
            self.upper_rom_select as usize
        } else {
            0
        };
        self.cartridge_banks[idx][offset]
    }

    fn selected_upper_rom(&self) -> &[u8] {
        if self.upper_rom_select == 7 {
            self.amsdos_rom.as_deref().unwrap_or(&self.basic_rom)
        } else if let Some(rom) = self
            .expansion_roms
            .get(self.upper_rom_select as usize)
            .and_then(Option::as_deref)
        {
            // A real expansion ROM board loaded into this slot (real
            // hardware fact: slots 1-6/8-15 are unused unless populated).
            rom
        } else {
            // Slot 0 is BASIC; unpopulated expansion slots fall back to
            // BASIC on classic CPC hardware.
            &self.basic_rom
        }
    }

    /// Loads a real expansion ROM board image into upper-ROM slot
    /// `slot` (1-6 or 8-15 -- slot 0 is always BASIC, slot 7 is always
    /// AMSDOS on FDC-equipped models, neither is a valid target here).
    /// `rom` must be exactly `ROM_SIZE` (16K) bytes, the real size of
    /// one CPC upper-ROM slot.
    pub(crate) fn load_expansion_rom(&mut self, slot: u8, rom: Vec<u8>) -> Result<(), String> {
        if slot == 0 || slot == 7 || slot as usize >= self.expansion_roms.len() {
            return Err(format!(
                "invalid expansion ROM slot {slot} (expected 1-6 or 8-15)"
            ));
        }
        if rom.len() != ROM_SIZE {
            return Err(format!(
                "expansion ROM slot {slot} image must be {ROM_SIZE} bytes, got {}",
                rom.len()
            ));
        }
        self.expansion_roms[slot as usize] = Some(rom);
        Ok(())
    }

    /// Resolves a paging-table page index into a real physical 16K page
    /// in `self.ram`. `RAM_CONFIGURATIONS` entries `0..=3` are the base
    /// 64K (pages 0-3, always fixed -- "always 0 on an unexpanded
    /// CPC6128"); entries `4..=7` are relative to whichever 64K bank
    /// register 3's bits 5:3 currently select (real hardware fact, see
    /// `CREDITS.md`) -- bank 0 is the base 6128's own onboard second
    /// 64K (physical pages 4-7, unchanged from every model without
    /// `additional_ram_banks`), banks 1+ are Standard Memory Expansion
    /// boards. Selecting a bank beyond how many are physically present
    /// wraps to a populated one (an engineering choice, not a verified
    /// "nothing installed" hardware behavior -- see `CREDITS.md`).
    fn window_ram_bank(&self, window: usize) -> usize {
        if self.model.has_128k_paging() {
            let page = RAM_CONFIGURATIONS[self.ram_configuration as usize][window];
            if page < 4 {
                page
            } else {
                let populated_banks = u16::from(self.additional_ram_banks) + 1;
                let bank = u16::from(self.ram_bank_select) % populated_banks;
                4 + bank as usize * 4 + (page - 4)
            }
        } else {
            window
        }
    }

    fn window_source_label(&self, window: usize) -> String {
        match window {
            0 if !self.gate_array.lower_rom_disabled() => "ROM lower".to_string(),
            3 if !self.gate_array.upper_rom_disabled() => {
                if self.upper_rom_select == 7 && self.amsdos_rom.is_some() {
                    "ROM upper[7]=AMSDOS".to_string()
                } else {
                    format!("ROM upper[{}]=BASIC", self.upper_rom_select)
                }
            }
            _ => format!("RAM{}", self.window_ram_bank(window)),
        }
    }

    fn ram_index(&self, addr: u16) -> usize {
        let window = addr as usize / ROM_SIZE;
        let bank = self.window_ram_bank(window);
        bank * ROM_SIZE + (addr as usize & (ROM_SIZE - 1))
    }

    fn record_memory_read(&mut self, addr: u16) {
        self.memory_reads_total = self.memory_reads_total.saturating_add(1);
        let block = usize::from(addr) / MEMORY_ACTIVITY_BLOCK_SIZE;
        if let Some(count) = self.memory_read_blocks.get_mut(block) {
            *count = count.saturating_add(1);
        }
    }

    fn record_memory_write(&mut self, addr: u16) {
        self.memory_writes_total = self.memory_writes_total.saturating_add(1);
        let block = usize::from(addr) / MEMORY_ACTIVITY_BLOCK_SIZE;
        if let Some(count) = self.memory_write_blocks.get_mut(block) {
            *count = count.saturating_add(1);
        }
    }

    fn record_tape_waveform_sample(&mut self) {
        let sample = self
            .tape
            .as_ref()
            .and_then(|tape| {
                let state = tape.playback_state();
                (state.playing && !state.finished).then(|| u8::from(tape.ear_bit()))
            })
            .unwrap_or(1);
        self.tape_waveform[self.tape_waveform_cursor] = sample;
        self.tape_waveform_cursor = (self.tape_waveform_cursor + 1) % TAPE_WAVEFORM_SAMPLES;
        if self.tape_waveform_len < TAPE_WAVEFORM_SAMPLES {
            self.tape_waveform_len += 1;
        }
    }

    fn tape_waveform_samples(&self) -> Vec<u8> {
        let len = self.tape_waveform_len;
        if len == 0 {
            return Vec::new();
        }
        let start =
            (self.tape_waveform_cursor + TAPE_WAVEFORM_SAMPLES - len) % TAPE_WAVEFORM_SAMPLES;
        let mut out = Vec::with_capacity(len);
        for offset in 0..len {
            let idx = (start + offset) % TAPE_WAVEFORM_SAMPLES;
            out.push(self.tape_waveform[idx]);
        }
        out
    }

    fn record_tape_pulse_width(&mut self, width_cycles: u32) {
        let width = u16::try_from(width_cycles).unwrap_or(u16::MAX);
        self.tape_pulse_widths[self.tape_pulse_widths_cursor] = width;
        self.tape_pulse_widths_cursor =
            (self.tape_pulse_widths_cursor + 1) % TAPE_PULSE_WIDTH_SAMPLES;
        if self.tape_pulse_widths_len < TAPE_PULSE_WIDTH_SAMPLES {
            self.tape_pulse_widths_len += 1;
        }
    }

    fn tape_pulse_widths_samples(&self) -> Vec<u16> {
        let len = self.tape_pulse_widths_len;
        if len == 0 {
            return Vec::new();
        }
        let start = (self.tape_pulse_widths_cursor + TAPE_PULSE_WIDTH_SAMPLES - len)
            % TAPE_PULSE_WIDTH_SAMPLES;
        let mut out = Vec::with_capacity(len);
        for offset in 0..len {
            let idx = (start + offset) % TAPE_PULSE_WIDTH_SAMPLES;
            out.push(self.tape_pulse_widths[idx]);
        }
        out
    }

    fn select_ram_configuration(&mut self, val: u8) {
        if self.model.has_128k_paging() {
            self.ram_configuration = val & 0x07;
            self.ram_bank_select = (val >> 3) & 0x07;
        }
    }

    pub(crate) fn screen_ram(&self) -> &[u8] {
        // The CRTC always sees the primary 64K, independent of CPU paging.
        &self.ram[..PRIMARY_RAM_SIZE]
    }

    pub(crate) fn ram(&self) -> &[u8] {
        &self.ram
    }

    pub(crate) fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }

    pub(crate) fn ram_configuration(&self) -> u8 {
        self.ram_configuration
    }

    pub(crate) fn set_ram_configuration(&mut self, configuration: u8) {
        self.ram_configuration = configuration & 0x07;
    }

    pub(crate) fn ram_bank_select(&self) -> u8 {
        self.ram_bank_select
    }

    pub(crate) fn set_ram_bank_select(&mut self, bank: u8) {
        self.ram_bank_select = bank & 0x07;
    }

    pub(crate) fn additional_ram_banks(&self) -> u8 {
        self.additional_ram_banks
    }

    /// Grows (or shrinks) physical RAM to `banks` additional 64K
    /// Standard Memory Expansion banks (0-7, clamped) beyond the base
    /// 6128's own onboard second 64K -- a no-op on any model without
    /// `has_128k_paging()`. Existing bytes below the new length are
    /// preserved; new bytes (when growing) start zeroed, matching
    /// freshly powered-on RAM.
    pub(crate) fn set_additional_ram_banks(&mut self, banks: u8) {
        if !self.model.has_128k_paging() {
            return;
        }
        self.additional_ram_banks = banks.min(7);
        let total_banks = u32::from(self.additional_ram_banks) + 1;
        let new_len = (4 + total_banks as usize * 4) * ROM_SIZE;
        self.ram.resize(new_len, 0);
    }

    pub(crate) fn upper_rom_select(&self) -> u8 {
        self.upper_rom_select
    }

    pub(crate) fn set_upper_rom_select(&mut self, selection: u8) {
        self.upper_rom_select = selection;
    }

    pub(crate) fn disk_motor(&self) -> bool {
        self.disk_motor
    }

    pub(crate) fn set_disk_motor(&mut self, active: bool) {
        self.disk_motor = active;
        if let Some(fdc) = &mut self.fdc {
            fdc.set_motor(active);
        }
    }

    pub(crate) fn fdc_state(&self) -> Option<nec765::FdcState> {
        self.fdc.as_ref().map(nec765::Fdc::dump_state)
    }

    pub(crate) fn restore_fdc_state(&mut self, state: Option<nec765::FdcState>) {
        if let (Some(fdc), Some(state)) = (&mut self.fdc, state) {
            fdc.restore_state(state);
        }
    }

    /// The 8255 and the two latches that depend on what the firmware wrote to it: port C's keyboard row and the PSG's selected
    /// register. A save state that left them out resumed with the PPI in its power-on mode.
    pub(crate) fn ppi_state(&self) -> (i8255::Ppi, u8, u8) {
        (
            self.ppi.clone(),
            self.keyboard_row_select,
            self.psg_selected_register,
        )
    }

    pub(crate) fn restore_ppi_state(
        &mut self,
        ppi: i8255::Ppi,
        keyboard_row_select: u8,
        psg_selected_register: u8,
    ) {
        self.ppi = ppi;
        self.keyboard_row_select = keyboard_row_select;
        self.psg_selected_register = psg_selected_register;
        self.ay.select_register(psg_selected_register);
    }

    /// CPC Plus/GX4000 ASIC state (all internal register storage, no
    /// externally supplied media -- unlike the cartridge image itself,
    /// which stays external, same `--rom` convention as every other
    /// machine here). `None` for every classic model.
    pub(crate) fn asic_state(&self) -> Option<crate::asic::Asic> {
        self.asic.clone()
    }

    pub(crate) fn restore_asic_state(&mut self, state: Option<crate::asic::Asic>) {
        if let (Some(asic), Some(state)) = (&mut self.asic, state) {
            *asic = state;
        }
    }

    pub(crate) fn cpc_snapshot(&self, cpu: z80::Cpu) -> cpc_snapshot::Snapshot {
        let (selected_register, registers) = self.crtc.snapshot_state();
        let overlay = self.overlay_snapshot();
        let mut ay_registers = [0u8; 16];
        for (index, register) in ay_registers.iter_mut().enumerate() {
            *register = self.ay.read_register(index as u8);
        }
        cpc_snapshot::Snapshot {
            version: 3,
            cpu,
            gate_array: cpc_snapshot::GateArrayState {
                selected_pen: self.gate_array.selected_pen(),
                pen_colors: self.gate_array.pen_colors(),
                rom_config: self.gate_array.rom_config(),
                ram_config: self.ram_configuration,
            },
            crtc: cpc_snapshot::CrtcState {
                selected_register,
                registers,
            },
            upper_rom: self.upper_rom_select,
            ppi: cpc_snapshot::PpiState {
                port_a: self.ppi.port_a_latch(),
                port_b: self.ppi.port_b_latch(),
                port_c: self.ppi.port_c_latch(),
                control: self.ppi.control(),
            },
            ay: cpc_snapshot::AyState {
                selected_register: self.psg_selected_register,
                registers: ay_registers,
            },
            ram: self.ram.clone(),
            cpc_model: Some(match self.model {
                Model::Cpc464 | Model::Cpc464Plus => 0,
                Model::Cpc664 => 1,
                Model::Cpc6128 | Model::Cpc6128Plus => 2,
                Model::Gx4000 => 3,
            }),
            last_interrupt: Some(0),
            screen_modes: [0; 6],
            version3: Some(cpc_snapshot::Version3State {
                drive_a_filename: [0; 13],
                drive_b_filename: [0; 13],
                cartridge_filename: [0; 13],
                fdc_motor: u8::from(self.disk_motor),
                drive_a_track: overlay.fdc_cylinder.unwrap_or(0),
                drive_b_track: 0,
                drive_c_track: 0,
                drive_d_track: 0,
                printer_data: 0,
                psg_envelope_step: 0,
                psg_envelope_direction: 0,
                crtc_type: self.crtc.crtc_type().index(),
                crtc_address: 0,
                crtc_scanline: 0,
                crtc_char_count: 0,
                crtc_line_count: 0,
                crtc_raster_count: self.crtc.raster_counter(),
                crtc_vt_adjust_count: 0,
                crtc_hsync_count: 0,
                crtc_vsync_count: 0,
                crtc_flags: 0,
                gate_array_interrupt_delay: 0,
                gate_array_scanline_count: 0,
                z80_interrupt_pending: 0,
            }),
        }
    }

    pub(crate) fn restore_cpc_snapshot(&mut self, snapshot: &cpc_snapshot::Snapshot) {
        self.ram.copy_from_slice(&snapshot.ram);
        self.gate_array.restore_snapshot_state(
            snapshot.gate_array.selected_pen,
            snapshot.gate_array.pen_colors,
            snapshot.gate_array.rom_config,
        );
        self.ram_configuration = snapshot.gate_array.ram_config & 0x07;
        let (selected_register, registers) =
            (snapshot.crtc.selected_register, snapshot.crtc.registers);
        self.crtc
            .restore_snapshot_state(selected_register, registers);
        self.upper_rom_select = snapshot.upper_rom;
        self.ppi.restore_snapshot_state(
            snapshot.ppi.port_a,
            snapshot.ppi.port_b,
            snapshot.ppi.port_c,
            snapshot.ppi.control,
        );
        self.keyboard_row_select = snapshot.ppi.port_c & 0x0F;
        self.psg_selected_register = snapshot.ay.selected_register & 0x0F;
        for (index, value) in snapshot.ay.registers.iter().copied().enumerate() {
            self.ay.select_register(index as u8);
            self.ay.write(value);
        }
        self.ay.select_register(self.psg_selected_register);
        if let Some(v3) = &snapshot.version3 {
            self.set_disk_motor(v3.fdc_motor != 0);
            // Honour the snapshot's CRTC type when it names one this model
            // can carry: types 3/4 are the ASIC cores (CPC+ family; 4 also
            // the cost-down classic 6128), 0-2 discrete 6845s.
            if let Some(crtc_type) = crate::CrtcType::from_index(v3.crtc_type) {
                let plausible = if self.model.has_asic() {
                    matches!(crtc_type, crate::CrtcType::Type3 | crate::CrtcType::Type4)
                } else {
                    crtc_type != crate::CrtcType::Type3
                };
                if plausible {
                    self.crtc.set_crtc_type(crtc_type);
                }
            }
        }
    }

    /// Puts `disk` in `drive` (0 = A, 1 = B), keeping a pristine copy of what went in.
    fn load_disk_in(&mut self, drive: usize, disk: dsk::Disk) {
        if drive < nec765::DRIVES
            && let Some(fdc) = &mut self.fdc
        {
            self.pristine_disks[drive] = Some(disk.clone());
            fdc.insert_disk_in(drive, disk);
        }
    }

    /// Takes the disk out of drive A (a no-op with none inserted).
    fn eject_disk(&mut self) {
        self.eject_disk_from(0);
    }

    /// Takes the disk out of `drive`. The pristine copy goes too: it belongs to the disk that left.
    fn eject_disk_from(&mut self, drive: usize) {
        if let Some(fdc) = &mut self.fdc {
            fdc.eject_disk_from(drive);
        }
        if let Some(pristine) = self.pristine_disks.get_mut(drive) {
            *pristine = None;
        }
    }

    /// The disk in drive A with every write applied.
    pub(crate) fn disk(&self) -> Option<&dsk::Disk> {
        self.disk_in(0)
    }

    /// The disk in `drive` with every write applied.
    pub(crate) fn disk_in(&self, drive: usize) -> Option<&dsk::Disk> {
        self.fdc.as_ref()?.disk_in(drive)
    }

    /// The modified disk of drive A serialised for a save state; `None` while the
    /// disk is untouched (then the externally supplied image is exact).
    pub(crate) fn modified_disk_image(&self) -> Option<Vec<u8>> {
        self.modified_disk_image_of(0)
    }

    pub(crate) fn modified_disk_image_of(&self, drive: usize) -> Option<Vec<u8>> {
        self.disk_in(drive)
            .filter(|d| d.is_modified())
            .map(dsk::Disk::to_bytes)
    }

    /// Puts drive A's medium back to what a save state recorded: the
    /// captured modified image, or (for a state taken before any write)
    /// the disk as originally inserted.
    pub(crate) fn restore_disk_image(&mut self, image: Option<Vec<u8>>) -> Result<(), String> {
        self.restore_disk_image_of(0, image)
    }

    pub(crate) fn restore_disk_image_of(
        &mut self,
        drive: usize,
        image: Option<Vec<u8>>,
    ) -> Result<(), String> {
        if drive >= nec765::DRIVES {
            return Ok(());
        }
        let Some(fdc) = &mut self.fdc else {
            return Ok(());
        };
        match image {
            Some(bytes) => {
                let mut disk = dsk::parse(&bytes)
                    .map_err(|e| format!("save state disk image is invalid: {e}"))?;
                disk.mark_modified();
                fdc.insert_disk_in(drive, disk);
            }
            None => {
                if fdc.disk_in(drive).is_some_and(dsk::Disk::is_modified)
                    && let Some(pristine) = self.pristine_disks[drive].clone()
                {
                    fdc.insert_disk_in(drive, pristine);
                }
            }
        }
        Ok(())
    }

    /// Records that drive A's current disk image has been persisted.
    pub(crate) fn mark_disk_saved(&mut self) {
        self.mark_disk_saved_in(0);
    }

    pub(crate) fn mark_disk_saved_in(&mut self, drive: usize) {
        if let Some(disk) = self.fdc.as_mut().and_then(|fdc| fdc.disk_in_mut(drive)) {
            disk.clear_modified();
        }
    }

    fn load_tape(&mut self, tape: zx_tape::Tape) {
        let tape = tape.with_leading_silence(self.tape_leader_ms);
        self.tape = Some(zx_tape::TapePlayer::new(tape));
        self.tape_cycle_debt = 0;
        if self.cassette_motor
            && let Some(tape) = &mut self.tape
        {
            tape.play();
        }
        self.refresh_cassette_input();
    }

    /// Takes the cassette out of the deck; the EAR input goes back to idle.
    /// What the program recorded on the output line stays (it is a separate
    /// recorder, written out on its own).
    fn eject_tape(&mut self) {
        self.tape = None;
        self.tape_cycle_debt = 0;
        self.refresh_cassette_input();
    }

    fn set_tape_speed(&mut self, multiplier: u32) {
        if let Some(tape) = &mut self.tape {
            tape.set_speed_multiplier(multiplier);
        }
    }

    fn pause_tape_after_motor_stop(&mut self) {
        if let Some(tape) = &mut self.tape {
            tape.pause();
        }
        self.tape_cycle_debt = 0;
        self.tape_cycles_since_edge = 0;
        self.tape_waveform_sample_debt = 0;
    }

    fn tick_tape(&mut self, _cpc_cycles: u32) {
        let mut tape_cycles_advanced = 0u32;
        let mut edge_fired = false;
        if (self.cassette_motor
            || matches!(self.tape_motor_mode, TapeMotorMode::ZesaruxCompatibility))
            && let Some(tape) = &mut self.tape
        {
            self.tape_cycle_debt += u64::from(_cpc_cycles) * ZX_TAPE_CLOCK_HZ;
            let tape_cycles = (self.tape_cycle_debt / CLOCK_HZ) as u32;
            self.tape_cycle_debt %= CLOCK_HZ;
            if tape_cycles != 0 {
                let level_before = tape.ear_bit();
                tape.tick(tape_cycles);
                edge_fired = tape.ear_bit() != level_before;
                tape_cycles_advanced = tape_cycles;
            }
        }
        if tape_cycles_advanced != 0 {
            self.tape_cycles_since_edge = self
                .tape_cycles_since_edge
                .saturating_add(tape_cycles_advanced);
            if edge_fired {
                self.record_tape_pulse_width(self.tape_cycles_since_edge);
                self.tape_cycles_since_edge = 0;
            }
            self.tape_waveform_sample_debt = self
                .tape_waveform_sample_debt
                .saturating_add(tape_cycles_advanced);
            while self.tape_waveform_sample_debt >= TAPE_WAVEFORM_SAMPLE_INTERVAL_CYCLES {
                self.tape_waveform_sample_debt -= TAPE_WAVEFORM_SAMPLE_INTERVAL_CYCLES;
                self.record_tape_waveform_sample();
            }
        }
        self.refresh_cassette_input();
    }

    /// Advances real Seek/Recalibrate timing on the FDC (see
    /// `nec765::Fdc::tick`'s own doc comment) -- a no-op on models
    /// without an FDC.
    fn tick_fdc(&mut self, cycles: u32) {
        if let Some(fdc) = &mut self.fdc {
            fdc.tick(cycles);
        }
    }

    pub(crate) fn tape_position(&self) -> Option<zx_tape::TapePosition> {
        self.tape.as_ref().map(zx_tape::TapePlayer::dump_state)
    }

    pub(crate) fn tape_playback_state(&self) -> Option<zx_tape::TapePlaybackState> {
        self.tape.as_ref().map(zx_tape::TapePlayer::playback_state)
    }

    pub(crate) fn tape_ear_bit(&self) -> Option<bool> {
        self.tape.as_ref().map(zx_tape::TapePlayer::ear_bit)
    }

    pub(crate) fn tape_cycles_until_next_edge(&self) -> Option<u32> {
        self.tape
            .as_ref()
            .and_then(zx_tape::TapePlayer::cycles_until_next_edge)
    }

    pub(crate) fn restore_tape_position(&mut self, state: Option<zx_tape::TapePosition>) {
        if let (Some(tape), Some(state)) = (&mut self.tape, state) {
            tape.restore_state(state);
        }
        self.refresh_cassette_input();
    }

    pub(crate) fn cassette_motor(&self) -> bool {
        self.cassette_motor
    }

    pub(crate) fn has_tape(&self) -> bool {
        self.tape.is_some()
    }

    pub(crate) fn tape_motor_mode(&self) -> TapeMotorMode {
        self.tape_motor_mode
    }

    pub(crate) fn set_tape_motor_mode(&mut self, mode: TapeMotorMode) {
        self.tape_motor_mode = mode;
        if matches!(mode, TapeMotorMode::CapriceStrict) && !self.cassette_motor {
            self.pause_tape_after_motor_stop();
        } else if matches!(mode, TapeMotorMode::ZesaruxCompatibility)
            && self.tape.is_some()
            && let Some(tape) = &mut self.tape
        {
            tape.play();
        }
        self.refresh_cassette_input();
    }

    pub(crate) fn set_cassette_motor(&mut self, active: bool) {
        let was_active = self.cassette_motor;
        self.cassette_motor = active;
        let now = self.now();
        self.tape_recorder.motor(now, active);
        self.tape_cycle_debt = 0;
        if active {
            self.tape_cycles_since_edge = 0;
            self.tape_waveform_sample_debt = 0;
            if !was_active && let Some(tape) = &mut self.tape {
                tape.play();
            }
        } else if was_active && matches!(self.tape_motor_mode, TapeMotorMode::CapriceStrict) {
            self.pause_tape_after_motor_stop();
        }
        if active != was_active {
            self.tape_motor_toggles = self.tape_motor_toggles.saturating_add(1);
            let playback = self.tape.as_ref().map(zx_tape::TapePlayer::playback_state);
            let edge_cycles = self
                .tape
                .as_ref()
                .and_then(zx_tape::TapePlayer::cycles_until_next_edge);
            let ear = self.tape.as_ref().map(zx_tape::TapePlayer::ear_bit);
            tracing::info!(
                "cpc cassette motor transition: {}->{} attached={} pos={}/{} playing={} finished={} speed={}x edge_cycles={} ear={}",
                if was_active { "on" } else { "off" },
                if active { "on" } else { "off" },
                self.tape.is_some(),
                playback.map_or(0, |s| s.position),
                playback.map_or(0, |s| s.events),
                playback.is_some_and(|s| s.playing),
                playback.is_some_and(|s| s.finished),
                playback.map_or(1, |s| s.speed_multiplier),
                edge_cycles
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "n/a".to_string()),
                match ear {
                    Some(true) => "high",
                    Some(false) => "low",
                    None => "n/a",
                }
            );
        }
        self.refresh_cassette_input();
    }
    fn write_ppi_control(&mut self, val: u8) {
        // Compare against ZEsarUX's CPC PPI handling: cassette motor control
        // must only come from BSR writes (bit7=0) targeting Port C bit 4.
        // Mode-set writes (bit7=1) may reset Port-C output latches, but
        // they must not directly toggle the cassette motor state.
        let old_port_c = self.ppi.read_port_c();
        let old_motor_latch = (old_port_c & 0x10) != 0;

        self.ppi.write_control(val);

        let port_c = self.ppi.read_port_c();
        let new_motor_latch = (port_c & 0x10) != 0;
        self.keyboard_row_select = port_c & 0x0F;
        self.note_cassette_output();

        // Only bit-set/reset (bit7=0) commands that target Port C bit 4
        // control the cassette motor. BSR writes to other Port C bits are
        // unrelated (keyboard row/PSG handshakes) and must not affect it.
        // Mode-set writes (bit7=1) also never touch the motor latch.
        if val & 0x80 == 0 {
            let bit = (val >> 1) & 0x07;
            let value = val & 0x01;
            if self.tape.is_some() {
                tracing::info!(
                    "cpc ppi bsr write: control=0x{val:02X} bit={} value={} motor_latch_before={} motor_latch_after={} port_c_before=0x{old_port_c:02X} port_c_after=0x{port_c:02X}",
                    bit,
                    value,
                    if old_motor_latch { "on" } else { "off" },
                    if new_motor_latch { "on" } else { "off" }
                );
            }
            if bit != 4 && old_motor_latch != new_motor_latch {
                tracing::warn!(
                    "cpc ppi non-motor bsr changed motor latch: control=0x{val:02X} bit={} value={} before={} after={} port_c_before=0x{old_port_c:02X} port_c_after=0x{port_c:02X}",
                    bit,
                    value,
                    if old_motor_latch { "on" } else { "off" },
                    if new_motor_latch { "on" } else { "off" }
                );
            }
            if bit >= 6 {
                // BDIR/BC1 moved by bit set/reset: the AY sees the new function like after a Port C write.
                self.psg_function(self.ppi.port_c_latch());
            }
            if bit == 4 {
                self.tape_control_writes = self.tape_control_writes.saturating_add(1);
                let motor_on = value != 0;
                tracing::info!(
                    "cpc ppi motor command: control=0x{val:02X} bit=4 value={} motor={}",
                    value,
                    if motor_on { "on" } else { "off" }
                );
                self.set_cassette_motor(motor_on);
            }
        } else if self.tape.is_some() {
            tracing::info!(
                "cpc ppi mode-set write: control=0x{val:02X} motor_latch_before={} motor_latch_after={} port_c_before=0x{old_port_c:02X} port_c_after=0x{port_c:02X}",
                if old_motor_latch { "on" } else { "off" },
                if new_motor_latch { "on" } else { "off" }
            );
        }
    }

    fn ppi_port_b_read_value(&self) -> u8 {
        // Match classic CPC PPI Port B status lines (as ZEsarUX documents):
        // bits 1..4 are fixed high (manufacturer/refresh), bits 5..6 fixed
        // low (parallel/expansion), while bit 0 (VSYNC) and bit 7 (cassette
        // input) come from the live input latch state.
        let mut value = self.ppi.read_port_b();
        if self.ppi.control() & 0x02 == 0 {
            // Port B configured as output: the 8255 drives the pins, so a read gives the latch back (the
            // CPC acid test "port b output latch r/w").
            return value;
        }
        value |= CPC_PORT_B_FIXED_HIGH_MASK;
        value &= !CPC_PORT_B_FIXED_LOW_MASK;
        value
    }

    fn set_port_b_status_bit(&mut self, bit: u8, active: bool) {
        let mut input = self.ppi.read_port_b();
        let mask = 1 << bit;
        if active {
            input |= mask;
        } else {
            input &= !mask;
        }
        self.ppi.set_input_b(input);
    }

    fn refresh_cassette_input(&mut self) {
        let transport_runs = self.cassette_motor
            || matches!(self.tape_motor_mode, TapeMotorMode::ZesaruxCompatibility);
        let high = if transport_runs {
            self.tape.as_ref().is_none_or(|tape| tape.ear_bit())
        } else {
            true
        };
        self.set_port_b_status_bit(7, high);
    }
    fn set_vsync_input(&mut self, active: bool) {
        // PPI Port B bit 0 is the CRTC VSYNC state on a CPC. Preserve the
        // other externally driven status bits when updating it.
        self.set_port_b_status_bit(0, active);
    }

    /// What the keyboard matrix presents on `line`: the keys/joysticks, ANDed
    /// (all lines are active-low) with whatever an attached AMX mouse is
    /// pulling low on line 9.
    fn keyboard_line(&self, line: u8) -> u8 {
        let keys = self.keyboard.read(line);
        if line == 9 {
            keys & self.amx_mouse.line9_mask()
        } else {
            keys
        }
    }

    fn begin_instruction(&mut self) {
        self.instruction_advanced_tstates = 0;
    }

    fn finish_instruction(&mut self, total_tstates: u32) {
        debug_assert!(self.instruction_advanced_tstates <= total_tstates);
        self.cycles_total += u64::from(total_tstates);
        self.amx_mouse.tick(total_tstates);
        let internal = total_tstates.saturating_sub(self.instruction_advanced_tstates);
        self.bus_tstate_phase = ((u32::from(self.bus_tstate_phase) + internal) & 3) as u8;
    }

    pub(crate) fn bus_tstate_phase(&self) -> u8 {
        self.bus_tstate_phase
    }

    pub(crate) fn set_bus_tstate_phase(&mut self, phase: u8) {
        self.bus_tstate_phase = phase & 3;
        self.instruction_advanced_tstates = 0;
    }

    pub(crate) fn overlay_snapshot(&self) -> OverlaySnapshot {
        let fdc_state = self.fdc.as_ref().map(nec765::Fdc::dump_state);
        OverlaySnapshot {
            ram_bytes: self.ram.len(),
            rom_bytes: self.lower_rom.len()
                + self.basic_rom.len()
                + self.amsdos_rom.as_ref().map_or(0, Vec::len),
            map_0000: self.window_source_label(0),
            map_4000: self.window_source_label(1),
            map_8000: self.window_source_label(2),
            map_c000: self.window_source_label(3),
            memory_reads_total: self.memory_reads_total,
            memory_writes_total: self.memory_writes_total,
            memory_read_blocks: self.memory_read_blocks.to_vec(),
            memory_write_blocks: self.memory_write_blocks.to_vec(),
            io_reads_total: self.io_reads_total,
            io_writes_total: self.io_writes_total,
            ppi_control: self.ppi.control(),
            ppi_port_a: self.ppi.read_port_a(),
            ppi_port_b: self.ppi_port_b_read_value(),
            ppi_port_b_raw: self.ppi.read_port_b(),
            ppi_port_c: self.ppi.read_port_c(),
            keyboard_row_select: self.keyboard_row_select,
            psg_selected_register: self.psg_selected_register,
            upper_rom_select: self.upper_rom_select,
            ram_configuration: self.ram_configuration,
            gate_mode: self.gate_array.screen_mode(),
            tape_port_reads: self.tape_port_reads,
            tape_control_writes: self.tape_control_writes,
            tape_motor_toggles: self.tape_motor_toggles,
            tape_waveform: self.tape_waveform_samples(),
            tape_pulse_widths: self.tape_pulse_widths_samples(),
            disk_motor: self.disk_motor,
            disk_msr_reads: self.disk_msr_reads,
            disk_data_reads: self.disk_data_reads,
            disk_data_writes: self.disk_data_writes,
            disk_motor_writes: self.disk_motor_writes,
            fdc_cylinder: fdc_state.as_ref().map(nec765::FdcState::cylinder),
            fdc_phase: fdc_state
                .as_ref()
                .map(|state| state.phase_label().to_string()),
        }
    }
}

impl Bus for CpcBus {
    fn io_accessed(&mut self, port: u16, value: u8, write: bool) {
        self.port_watch.note(port, value, write);
    }

    fn contend_memory(&mut self, _addr: u16) -> u8 {
        (4 - self.bus_tstate_phase) & 3
    }

    fn advance_tstates(&mut self, tstates: u8) {
        self.bus_tstate_phase = (self.bus_tstate_phase + tstates) & 3;
        self.instruction_advanced_tstates += u32::from(tstates);
    }

    /// The Gate Array aligns an `IN`/`OUT` cycle just as it aligns a memory
    /// access, but the I/O cycle ends 3 T-states into a slot after a 4-T-state
    /// cycle (a memory read/write is 3). The core's own 4-T-state base cost
    /// then makes an I/O cycle last `4 + ((3 - phase) & 3)` T-states: 7, 6, 5 or
    /// 4 from phases 0..3, always finishing at phase 3. That reproduces ep128emu's
    /// `ioPortWait` and, through the instructions' other accesses, Caprice32's
    /// effective totals (`OUT (n),A` 12 T, `OUT (C),r`/`IN r,(C)` 16 T, `OUTI`/`INI`
    /// 20 T); see `tests/instruction_timing.rs`.
    fn contend_io(&mut self, _port: u16) -> u8 {
        (7 - self.bus_tstate_phase) & 3
    }

    /// An internal cycle spent before a later access of the same instruction
    /// moves the Gate Array slot phase that access is aligned against (a
    /// `PUSH`'s extra T-state makes its first write wait three more, not
    /// none). Counted like any other elapsed time, so the end-of-instruction
    /// reconciliation in `finish_instruction` sees one fewer deferred cycle.
    fn internal_cycles(&mut self, n: u8) {
        self.advance_tstates(n);
    }
    fn read(&mut self, addr: u16) -> u8 {
        self.record_memory_read(addr);
        self.peek(addr)
    }

    /// RAM is always written underneath ROM overlays. On the 6128 the live
    /// paging configuration chooses which physical 16K bank receives it.
    /// CPC Plus/GX4000: a write into `0x4000-0x7FFF` while the ASIC
    /// register window is visible there lands in the ASIC's own register
    /// storage instead (real hardware fact -- see `asic_register_write`'s
    /// own doc comment), not in system RAM.
    fn write(&mut self, addr: u16, val: u8) {
        self.record_memory_write(addr);
        if self.asic_register_write(addr, val) {
            return;
        }
        let index = self.ram_index(addr);
        self.ram[index] = val;
    }

    /// CPC devices decode only selected address bits. These checks are
    /// deliberately independent: one OUT can reach several devices.
    fn write_io(&mut self, port: u16, val: u8) {
        self.io_writes_total = self.io_writes_total.saturating_add(1);

        // Gate Array: A15=0, A14=1. On a CPC Plus/GX4000, a write with
        // bit 5 set targets RMR2 instead once unlocked -- see
        // `asic::Asic::maybe_write_rmr2`'s own doc comment.
        if port & 0xC000 == 0x4000 {
            let consumed_by_rmr2 = self
                .asic
                .as_mut()
                .is_some_and(|asic| asic.maybe_write_rmr2(val));
            if !consumed_by_rmr2 {
                self.gate_array.write(val);
                match val >> 6 {
                    // Ink write. A CPC+ takes its colours from the ASIC
                    // palette (see `video_state`) but still honours the
                    // classic write, which sets that palette entry.
                    0b01 => {
                        let pen = self.gate_array.selected_pen().min(16);
                        let (r, g, b) = match self.asic.as_mut() {
                            Some(asic) => asic.set_legacy_ink(pen, val),
                            None => self.gate_array.pen_color(pen),
                        };
                        self.log_video_event(VideoEventKind::Ink {
                            pen,
                            rgb: [r, g, b],
                        });
                    }
                    0b10 => self.log_video_event(VideoEventKind::RequestMode(val & 3)),
                    _ => {}
                }
            }
        }
        // CPC6128 paging PAL: A15=0 and Gate Array function 11.
        if port & 0x8000 == 0 && val >> 6 == 0b11 {
            self.select_ram_configuration(val);
        }
        // CRTC: A14=0; A9:A8 select the function. CPC Plus/GX4000: every
        // byte written to the select-register port also feeds the
        // ASIC's own unlock-sequence detector (real hardware fact, see
        // `asic::Asic::check_unlock_sequence`'s own doc comment).
        if port & 0x4000 == 0 {
            match port & 0x0300 {
                0x0000 => {
                    self.crtc_write(VideoEventKind::CrtcSelect(val));
                    if let Some(asic) = &mut self.asic {
                        asic.check_unlock_sequence(val);
                    }
                }
                0x0100 => self.crtc_write(VideoEventKind::CrtcWrite(val)),
                _ => {}
            }
        }
        // Upper ROM latch: A13=0. Slot 0 is BASIC, slot 7 is AMSDOS.
        if port & 0x2000 == 0 {
            self.upper_rom_select = val;
        }
        // PPI: A11=0; A9:A8 select A/B/C/control.
        if port & 0x0800 == 0 {
            match port & 0x0300 {
                0x0000 => self.ppi.write_port_a(val),
                0x0100 => self.ppi.write_port_b(val),
                0x0200 => self.write_port_c(val),
                0x0300 => self.write_ppi_control(val),
                _ => unreachable!(),
            }
        }
        // Expansion/FDC: A10=0 and A7=0; A8:A0 select motor/MSR/data.
        if port & 0x0400 == 0 && port & 0x0080 == 0 {
            let function = (((port >> 8) & 1) << 1) | (port & 1);
            match function {
                0 | 1 if self.fdc.is_some() => {
                    self.disk_motor_writes = self.disk_motor_writes.saturating_add(1);
                    self.disk_motor = val & 1 != 0;
                    if let Some(fdc) = &mut self.fdc {
                        fdc.set_motor(self.disk_motor);
                    }
                }
                3 => {
                    self.disk_data_writes = self.disk_data_writes.saturating_add(1);
                    if let Some(fdc) = &mut self.fdc {
                        fdc.write_data(val);
                    }
                }
                _ => {}
            }
        }
    }

    fn read_io(&mut self, port: u16) -> u8 {
        self.io_reads_total = self.io_reads_total.saturating_add(1);
        let mut data = 0xFF;
        // CRTC reads: A14=0; A9:A8 = 10 is the status register (`&BExx`),
        // 11 the register read (`&BFxx`). Which registers answer, and what
        // the status port returns, depends on the CRTC type.
        if port & 0x4000 == 0 {
            match port & 0x0300 {
                0x0200 => data = self.crtc.read_status(),
                0x0300 => data = self.crtc.read_register_clearing(),
                _ => {}
            }
        }
        if port & 0x0800 == 0 {
            data = match port & 0x0300 {
                0x0000 => self.ppi.read_port_a_clearing(),
                0x0100 => {
                    self.tape_port_reads = self.tape_port_reads.saturating_add(1);
                    self.ppi_port_b_read_value()
                }
                0x0200 => self.ppi.read_port_c(),
                _ => data,
            };
        }
        if port & 0x0400 == 0 && port & 0x0080 == 0 {
            let function = (((port >> 8) & 1) << 1) | (port & 1);
            if let Some(fdc) = &mut self.fdc {
                data = match function {
                    2 => {
                        self.disk_msr_reads = self.disk_msr_reads.saturating_add(1);
                        fdc.read_msr()
                    }
                    3 => {
                        self.disk_data_reads = self.disk_data_reads.saturating_add(1);
                        fdc.read_data()
                    }
                    _ => data,
                };
            }
        }
        data
    }

    /// Classic CPC has no vectoring controller (default floating-bus
    /// `0xFF` applies unchanged). CPC Plus/GX4000: routed through the
    /// ASIC -- see `asic::Asic::interrupt_vector_byte`'s own doc comment
    /// for the real, source-dependent byte (and the acknowledge-time
    /// bookkeeping this call also performs).
    fn interrupt_vector_byte(&mut self) -> u8 {
        match &mut self.asic {
            Some(asic) => asic.interrupt_vector_byte(),
            None => 0xFF,
        }
    }

    /// The Gate Array drops `/INT` and clears bit 5 of its counter when the
    /// Z80 acknowledges the interrupt (see `GateArray::acknowledge_interrupt`).
    fn interrupt_acknowledged(&mut self) {
        self.gate_array.acknowledge_interrupt();
    }
}

#[derive(Clone)]
pub struct Cpc {
    pub(crate) cpu: z80::Cpu,
    pub(crate) bus: CpcBus,

    /// In real CPU T-states, `f64` so its fractional remainder survives
    /// across instructions rather than drifting the effective sample
    /// rate flat -- same shape `zx_spectrum::ZxSpectrum`'s own
    /// `audio_cycle_accumulator` uses, for the same reason.
    pub(crate) audio_cycle_accumulator: f64,
    /// AY clock divider remainder -- real CPC AY-3-8910 runs at
    /// CPU-clock/4 (1MHz), a real, *different* divider from ZX Spectrum
    /// 128K's own CPU-clock/2 (the `ay-3-8910` crate itself has no
    /// opinion on this, by design -- see `CLAUDE.md`'s own note on
    /// exactly this). `u32` remainder, not a per-step division, same
    /// shape as `zx_spectrum`'s own `ay_cycle_debt`.
    pub(crate) ay_cycle_debt: u32,
    audio_buffer: Vec<f32>,
    /// The picture trace the CRTC's character clocks feed (see
    /// `crate::raster`). Not part of save states: a state load restarts it
    /// and the next completed frame is what renders.
    raster: crate::raster::FrameBuilder,
}

impl Cpc {
    /// Constructs one CPC from its "rom" blob: the standard classic
    /// combined ROM image (464/664/6128 = lower OS + BASIC [+ AMSDOS];
    /// 464Plus/6128Plus = the same shape, their own real Plus-ASIC-aware
    /// firmware) for every model with `Model::has_internal_rom()`, or
    /// (GX4000 only) the mandatory cartridge image: any multiple of
    /// `ROM_SIZE` bytes from 1 to `MAX_CARTRIDGE_BANKS` banks -- real
    /// GX4000 cartridges range from 16K to 128K.
    pub fn new(rom: Vec<u8>, model: Model) -> Result<Self, String> {
        if model.has_internal_rom() {
            let expected = model.rom_image_size();
            if rom.len() != expected {
                return Err(format!(
                    "{} ROM image must be {expected} bytes, got {}",
                    model.name(),
                    rom.len()
                ));
            }
        } else {
            let max = ROM_SIZE * MAX_CARTRIDGE_BANKS;
            if rom.is_empty() || !rom.len().is_multiple_of(ROM_SIZE) || rom.len() > max {
                return Err(format!(
                    "{} cartridge image must be a multiple of {ROM_SIZE} bytes, 1-{MAX_CARTRIDGE_BANKS} banks (up to {max} bytes), got {}",
                    model.name(),
                    rom.len()
                ));
            }
        }
        let bus = CpcBus::new(rom, model);
        let raster = crate::raster::FrameBuilder::new(bus.video_state());
        Ok(Self {
            cpu: z80::Cpu::default(),
            bus,
            audio_cycle_accumulator: 0.0,
            ay_cycle_debt: 0,
            audio_buffer: Vec::new(),
            raster,
        })
    }

    /// Restarts the picture trace from the live Gate Array/ASIC state --
    /// for anything that replaced that state behind the trace's back
    /// (save-state and snapshot loads).
    pub(crate) fn resync_video(&mut self) {
        self.bus.tape_recorder = crate::tape_recorder::TapeRecorder::default();
        self.bus.video_events.clear();
        self.bus.crtc_replay_base = None;
        self.raster.reset(self.bus.video_state());
    }

    pub fn model(&self) -> Model {
        self.bus.model
    }

    /// The CRTC silicon variant in use (see [`crate::CrtcType`]).
    pub fn crtc_type(&self) -> crate::CrtcType {
        self.bus.crtc.crtc_type()
    }

    /// Overrides the CRTC variant. Affects register read-back, the status
    /// port, sync-width and start-address rules, and re-applies the new
    /// type's register write masks to what is already programmed.
    pub fn set_crtc_type(&mut self, crtc_type: crate::CrtcType) {
        self.bus.crtc.set_crtc_type(crtc_type);
    }

    /// Sets one CPU register by name (`PC`, `AF`, `BC`, `DE`, `HL`, `IX`,
    /// `IY`, `SP`, `I`, `R`, `IM`, `IFF1`, `IFF2`, shadow pairs, and
    /// 8-bit aliases). Names are case-insensitive.
    pub fn set_register(&mut self, name: &str, value: u64) -> Result<(), String> {
        self.cpu.set_register(name, value)
    }

    pub fn load_disk(&mut self, disk: dsk::Disk) -> Result<(), String> {
        self.load_disk_in(0, disk)
    }

    /// Puts `disk` in drive `drive` (0 = A, 1 = B, the external drive of a CPC664/6128).
    pub fn load_disk_in(&mut self, drive: usize, disk: dsk::Disk) -> Result<(), String> {
        if !self.model().has_fdc() {
            return Err("floppy disks require CPC664 or CPC6128".to_string());
        }
        if drive >= nec765::DRIVES {
            return Err(format!("drive {drive} does not exist (drives 0 and 1)"));
        }
        self.bus.load_disk_in(drive, disk);
        Ok(())
    }

    /// Takes the disk out of drive A. Unsaved writes are lost with it --
    /// export them first (`take_modified_disk_image`) if they matter.
    pub fn eject_disk(&mut self) {
        self.bus.eject_disk();
    }

    /// Takes the disk out of `drive` (0 or 1).
    pub fn eject_disk_from(&mut self, drive: usize) {
        self.bus.eject_disk_from(drive);
    }

    /// Takes the cassette out of the deck.
    pub fn eject_tape(&mut self) {
        self.bus.eject_tape();
    }

    /// The disk currently in the drive, with every write the FDC has made
    /// applied (`dsk::Disk::is_modified` says whether any happened).
    pub fn disk(&self) -> Option<&dsk::Disk> {
        self.bus.disk()
    }

    /// The disk currently in `drive` (0 or 1).
    pub fn disk_in(&self, drive: usize) -> Option<&dsk::Disk> {
        self.bus.disk_in(drive)
    }

    /// The disk image as a `.dsk` file if it has been modified since it
    /// was inserted or last marked saved; `None` when there's nothing to
    /// persist.
    pub fn take_modified_disk_image(&self) -> Option<Vec<u8>> {
        self.bus.modified_disk_image()
    }

    /// `take_modified_disk_image` for `drive` (0 or 1).
    pub fn take_modified_disk_image_of(&self, drive: usize) -> Option<Vec<u8>> {
        self.bus.modified_disk_image_of(drive)
    }

    /// Call after writing `take_modified_disk_image`'s bytes out, so the
    /// same changes aren't reported as unsaved again.
    pub fn mark_disk_saved(&mut self) {
        self.bus.mark_disk_saved();
    }

    /// `mark_disk_saved` for `drive` (0 or 1).
    pub fn mark_disk_saved_in(&mut self, drive: usize) {
        self.bus.mark_disk_saved_in(drive);
    }

    /// Everything the program wrote to the cassette output line while the
    /// motor ran, as a tape (`None` when it wrote nothing). Writing it out
    /// with `zx_tape::write_tzx` or `write_voc` gives the file a real tape
    /// deck would have recorded.
    pub fn recorded_tape(&self) -> Option<zx_tape::Tape> {
        self.bus.tape_recorder.to_tape()
    }

    /// How many cassette-output pulses have been recorded.
    pub fn recorded_pulse_count(&self) -> usize {
        self.bus.tape_recorder.pulse_count()
    }

    /// Forgets the recording (after it has been saved).
    pub fn clear_recorded_tape(&mut self) {
        self.bus.tape_recorder.clear();
    }

    /// Installs a cassette tape (`.cdt`/`.tzx`) for this CPC. Every model
    /// supports cassette input; 664/6128 keep their FDC support alongside it.
    /// The tape gets [`DEFAULT_TAPE_LEADER_MS`] of leading silence unless
    /// [`Cpc::set_tape_leader_ms`] changed it before this call.
    pub fn load_tape(&mut self, tape: zx_tape::Tape) {
        self.bus.load_tape(tape);
    }

    /// Sets how much silence is prepended to a tape installed by a later
    /// [`Cpc::load_tape`] (`0` plays the image exactly as stored).
    pub fn set_tape_leader_ms(&mut self, ms: u32) {
        self.bus.tape_leader_ms = ms;
    }

    /// The leading silence applied by [`Cpc::load_tape`], in milliseconds.
    pub fn tape_leader_ms(&self) -> u32 {
        self.bus.tape_leader_ms
    }

    /// Sets cassette playback speed multiplier (1 = real time). A no-op
    /// until a tape has been installed.
    pub fn set_tape_speed(&mut self, multiplier: u32) {
        self.bus.set_tape_speed(multiplier);
    }

    /// Current cassette playback status for debugger/log diagnostics.
    pub fn tape_playback_state(&self) -> Option<zx_tape::TapePlaybackState> {
        self.bus.tape_playback_state()
    }

    /// Whether a cassette image is currently attached.
    pub fn has_tape(&self) -> bool {
        self.bus.has_tape()
    }

    /// Current cassette motor output line state.
    pub fn cassette_motor(&self) -> bool {
        self.bus.cassette_motor()
    }

    /// Current cassette motor transport policy.
    pub fn tape_motor_mode(&self) -> crate::TapeMotorMode {
        self.bus.tape_motor_mode()
    }

    /// Selects Caprice-strict or ZEsarUX-compatible cassette transport.
    pub fn set_tape_motor_mode(&mut self, mode: crate::TapeMotorMode) {
        self.bus.set_tape_motor_mode(mode);
    }

    /// Current EAR line level (`true` = high) when tape is attached.
    pub fn tape_ear_bit(&self) -> Option<bool> {
        self.bus.tape_ear_bit()
    }

    /// Tape-domain cycles remaining before the next EAR edge.
    pub fn tape_cycles_until_next_edge(&self) -> Option<u32> {
        self.bus.tape_cycles_until_next_edge()
    }

    /// Active-low key byte (`0` bit = pressed) for one CPC keyboard matrix
    /// line, exposed for host/plugin diagnostics.
    pub fn keyboard_row_state(&self, line: u8) -> u8 {
        self.bus.keyboard_line(line)
    }

    /// Current keyboard row-select nibble on PPI Port C (0-15).
    pub fn keyboard_row_select(&self) -> u8 {
        self.bus.keyboard_row_select
    }

    /// Last AY register selected through the Port-C BDIR/BC1 handshake.
    pub fn psg_selected_register(&self) -> u8 {
        self.bus.psg_selected_register
    }

    /// Rich machine telemetry for debugger overlays and deep diagnostics.
    pub fn overlay_snapshot(&self) -> OverlaySnapshot {
        self.bus.overlay_snapshot()
    }

    /// Performs an `OUT (port),value` on the I/O bus between instructions
    /// (the debugger's `write-port`). Decoded exactly like a CPU `OUT`, so a
    /// CPC's loose port decode can reach several chips from one write.
    pub fn write_port(&mut self, port: u16, value: u8) {
        self.bus.begin_instruction();
        self.bus.write_io(port, value);
    }

    /// Number of physical 16K RAM blocks: 4 on the 64K models, 8 on a 6128,
    /// plus 4 per Standard Memory Expansion bank (up to 36 = 576K).
    pub fn ram_block_count(&self) -> usize {
        self.bus.ram.len() / ROM_SIZE
    }

    /// Reads up to `len` bytes from `offset` within physical 16K RAM block
    /// `block`, independent of what the CPU currently has paged in. Blocks
    /// 0-3 are the base 64K, 4-7 the 6128's second 64K, 8+ the expansion
    /// banks. `None` for a block or offset out of range; the result is cut
    /// short at the end of the block.
    pub fn read_ram_block(&self, block: u8, offset: u16, len: u16) -> Option<Vec<u8>> {
        let start = usize::from(block) * ROM_SIZE + usize::from(offset);
        let block_end = (usize::from(block) + 1) * ROM_SIZE;
        if usize::from(offset) >= ROM_SIZE || block_end > self.bus.ram.len() {
            return None;
        }
        let end = (start + usize::from(len)).min(block_end);
        Some(self.bus.ram[start..end].to_vec())
    }

    /// Writes `data` at `offset` within physical RAM block `block` (see
    /// [`Cpc::read_ram_block`]), cut short at the end of the block. Returns
    /// the bytes written -- zero for a block or offset out of range.
    pub fn write_ram_block(&mut self, block: u8, offset: u16, data: &[u8]) -> usize {
        let start = usize::from(block) * ROM_SIZE + usize::from(offset);
        let block_end = (usize::from(block) + 1) * ROM_SIZE;
        if usize::from(offset) >= ROM_SIZE || block_end > self.bus.ram.len() {
            return 0;
        }
        let len = data.len().min(block_end - start);
        self.bus.ram[start..start + len].copy_from_slice(&data[..len]);
        len
    }

    /// Plugs an AMX-style mouse into the joystick-0 port (matrix line 9), or
    /// removes it. Off by default because it shares that port with digital
    /// joystick 1; see `crate::amx_mouse` for what is documented hardware
    /// behaviour and what is this project's own engineering choice.
    pub fn set_amx_mouse(&mut self, enabled: bool) {
        self.bus.amx_mouse.set_enabled(enabled);
    }

    pub fn amx_mouse_enabled(&self) -> bool {
        self.bus.amx_mouse.enabled()
    }

    /// Relative host mouse motion in mickeys (`dy < 0` = up). Ignored unless
    /// an AMX mouse is plugged in.
    pub fn mouse_motion(&mut self, dx: i8, dy: i8) {
        self.bus.amx_mouse.motion(dx, dy);
    }

    pub fn mouse_button(&mut self, button: crate::amx_mouse::MouseButton, pressed: bool) {
        self.bus.amx_mouse.button(button, pressed);
    }

    pub fn press_key(&mut self, key: crate::keyboard::CpcKey) {
        self.bus.keyboard.press(key);
    }

    pub fn release_key(&mut self, key: crate::keyboard::CpcKey) {
        self.bus.keyboard.release(key);
    }

    /// Sets one CPC Plus/GX4000 analogue-joystick axis (a no-op on
    /// every classic model -- no ASIC, no analogue port). `value`:
    /// `ANALOG_MIN..=ANALOG_MAX` (real 6-bit ADC range); typically
    /// driven from a digital direction (`ANALOG_MIN`/`ANALOG_MAX` at the
    /// pressed extremes, `ANALOG_CENTER` released -- see
    /// `crate::asic::Asic`'s own doc comment on this default).
    pub fn set_analog_axis(&mut self, joystick: AnalogJoystick, axis: AnalogAxis, value: u8) {
        let index = match (joystick, axis) {
            (AnalogJoystick::First, AnalogAxis::X) => 0,
            (AnalogJoystick::First, AnalogAxis::Y) => 1,
            (AnalogJoystick::Second, AnalogAxis::X) => 2,
            (AnalogJoystick::Second, AnalogAxis::Y) => 3,
        };
        if let Some(asic) = &mut self.bus.asic {
            asic.set_analog_axis(index, value);
        }
    }

    /// Grows physical RAM to `banks` additional 64K Standard Memory
    /// Expansion banks (0-7) beyond the base 6128's own onboard second
    /// 64K -- real hardware fact (see `CREDITS.md`): only CPC6128/
    /// 6128Plus have register 3's bank-select bits wired to anything.
    /// Call once, before booting real firmware that probes RAM size --
    /// resizing after firmware has already sized itself around the
    /// original RAM amount produces the same result a real user fitting
    /// an expansion board before power-on would see, not a hot-plug.
    pub fn set_ram_expansion_banks(&mut self, banks: u8) -> Result<(), String> {
        if !self.model().has_128k_paging() {
            return Err(format!(
                "RAM expansion requires a 128K-paging CPC model (6128/6128Plus), got {}",
                self.model().name()
            ));
        }
        self.bus.set_additional_ram_banks(banks);
        Ok(())
    }

    /// How many 64K expansion banks beyond the onboard second 64K are
    /// currently populated (always `0` on a model without
    /// `has_128k_paging()`).
    pub fn ram_expansion_banks(&self) -> u8 {
        self.bus.additional_ram_banks()
    }

    /// Loads a real expansion ROM board image into upper-ROM slot `slot`
    /// (1-6 or 8-15). A no-op-rejecting call on `Gx4000` (cartridge-only,
    /// no upper-ROM-slot concept at all -- see `Model::has_internal_rom`).
    pub fn load_expansion_rom(&mut self, slot: u8, rom: Vec<u8>) -> Result<(), String> {
        if !self.model().has_internal_rom() {
            return Err(format!(
                "{} has no upper-ROM expansion slots (cartridge-only)",
                self.model().name()
            ));
        }
        self.bus.load_expansion_rom(slot, rom)
    }

    /// Loads a Caprice-compatible CPC SNA snapshot. The snapshot's RAM size
    /// must match this machine; ROM and external media remain attached to the
    /// current instance. ASIC-only state is not represented by public SNA
    /// fields, so Plus/GX4000 ASIC registers remain unchanged.
    pub fn load_snapshot(&mut self, snapshot: cpc_snapshot::Snapshot) -> Result<(), String> {
        if snapshot.ram.len() != self.bus.ram().len() {
            return Err(format!(
                "CPC SNA RAM size mismatch: machine has {} bytes, snapshot has {}",
                self.bus.ram().len(),
                snapshot.ram.len()
            ));
        }
        if let Some(model) = snapshot.cpc_model {
            let compatible = match model {
                0 => matches!(self.model(), Model::Cpc464 | Model::Cpc464Plus),
                1 => self.model() == Model::Cpc664,
                2 => matches!(self.model(), Model::Cpc6128 | Model::Cpc6128Plus),
                3 => self.model().has_asic(),
                _ => false,
            };
            if !compatible {
                return Err(format!(
                    "CPC SNA model {model} is incompatible with {}",
                    self.model().name()
                ));
            }
        }
        self.cpu = snapshot.cpu;
        self.bus.restore_cpc_snapshot(&snapshot);
        self.resync_video();
        Ok(())
    }

    /// Builds a version-3 CPC SNA snapshot from current CPU, RAM, and
    /// register state. Public SNA has no Plus ASIC extension, so ASIC state
    /// remains available through this project's internal `.state` format.
    pub fn save_snapshot(&self) -> cpc_snapshot::Snapshot {
        self.bus.cpc_snapshot(self.cpu)
    }

    /// Picture-affecting writes made through the bus *outside* an
    /// instruction (a debugger poke, `load`, tests driving the bus directly)
    /// have no instruction to be timed within: the CRTC/Gate Array already
    /// took them, so only the trace's shadow state needs catching up.
    fn flush_out_of_step_video(&mut self) {
        if self.bus.video_events.is_empty() {
            return;
        }
        for event in std::mem::take(&mut self.bus.video_events) {
            match event.kind {
                VideoEventKind::RequestMode(mode) => self.raster.request_mode(mode),
                VideoEventKind::Ink { pen, rgb } => self.raster.set_ink(pen, rgb),
                VideoEventKind::HScroll(scroll) => self.raster.set_h_scroll(scroll),
                VideoEventKind::CrtcSelect(_) | VideoEventKind::CrtcWrite(_) => {}
            }
        }
        self.bus.crtc_replay_base = None;
    }

    /// Advances the CRTC -- and the picture trace its character clocks feed
    /// -- across one instruction of `cycles` T-states. The instruction's
    /// picture-affecting writes (CRTC registers, Gate Array modes/inks, ASIC
    /// palette and scroll) are re-applied in the order and at the T-state
    /// they happened, by splitting the tick there, so a write lands on the
    /// character clock it really hit.
    fn advance_video(&mut self, cycles: u32) -> crate::crtc::CrtcEvents {
        let pending = std::mem::take(&mut self.bus.video_events);
        if pending.is_empty() {
            self.bus.crtc_replay_base = None;
            return self.tick_video(cycles);
        }
        if let Some(base) = self.bus.crtc_replay_base.take() {
            // The writes already ran against the CRTC (so in-instruction
            // reads saw them); rewind and replay them against its ticks.
            self.bus.crtc = base;
        }
        let mut events = crate::crtc::CrtcEvents::default();
        let mut elapsed = 0;
        for event in pending {
            let at = event.t.min(cycles).max(elapsed);
            events.merge(self.tick_video(at - elapsed));
            elapsed = at;
            match event.kind {
                VideoEventKind::CrtcSelect(value) => self.bus.crtc.select_register(value),
                VideoEventKind::CrtcWrite(value) => self.bus.crtc.write_register(value),
                VideoEventKind::RequestMode(mode) => self.raster.request_mode(mode),
                VideoEventKind::Ink { pen, rgb } => self.raster.set_ink(pen, rgb),
                VideoEventKind::HScroll(scroll) => self.raster.set_h_scroll(scroll),
            }
        }
        events.merge(self.tick_video(cycles - elapsed));
        events
    }

    /// Ticks the CRTC by `tstates`, feeding every character clock (plus the
    /// two video-RAM bytes it addresses) to the picture trace.
    fn tick_video(&mut self, tstates: u32) -> crate::crtc::CrtcEvents {
        if tstates == 0 {
            return crate::crtc::CrtcEvents::default();
        }
        let Cpc { bus, raster, .. } = self;
        let CpcBus {
            crtc, ram, asic, ..
        } = bus;
        let ram = &ram[..PRIMARY_RAM_SIZE];
        let scroll = match asic {
            Some(asic) if asic.active() => {
                crtc.set_split(asic.split_line(), asic.split_address());
                asic.v_scroll()
            }
            _ => 0,
        };
        crtc.tick_with(tstates, |clock| {
            raster.on_char(clock, crate::raster::fetch_video_bytes(ram, clock, scroll));
        })
    }

    /// CPC has no separate beeper the way ZX Spectrum does -- the
    /// AY-3-8910 is the *only* sound source, so this is simpler than
    /// `zx_spectrum::ZxSpectrum::mix_audio_sample`'s beeper+AY mix.
    fn mix_audio_sample(&self) -> f32 {
        self.bus.ay.output().clamp(-1.0, 1.0)
    }
}

impl Machine for Cpc {
    fn clock_hz(&self) -> u64 {
        CLOCK_HZ
    }

    fn frame_hz(&self) -> u64 {
        FRAME_HZ
    }

    fn run(&mut self, running: &mut dyn FnMut(&dyn Machine) -> bool) -> RunStats {
        machine::Clock::new(self.frame_hz()).run(self, running)
    }

    fn step(&mut self) -> u32 {
        self.flush_out_of_step_video();
        // `/INT` is a held level, not a pulse: sampled at every instruction
        // boundary, so an interrupt that arrived while interrupts were
        // disabled (or during the instruction after `EI`) is taken as soon as
        // the CPU allows it, and one cleared meanwhile (acknowledge, or an
        // `RMR` write with bit 4) is not delivered from a stale request.
        if self.bus.gate_array.interrupt_line() {
            self.cpu.request_interrupt();
        }
        self.bus.begin_instruction();
        let cycles = u32::from(self.cpu.step(&mut self.bus));
        self.bus.finish_instruction(cycles);
        let events = self.advance_video(cycles);
        for _ in 0..events.hsync_falls {
            let classic_interrupt = self.bus.gate_array.hsync_falling_edge();
            // Real hardware: classic CPC-style interrupts and the CPC
            // Plus/GX4000 Programmable Raster Interrupt cannot be active
            // at the same time -- the classic counter's own bookkeeping
            // above still ran either way, only the assertion is
            // suppressed (see `CpcBus::asic_pri_active`'s own doc
            // comment). On a classic CPC (no ASIC) this is always false.
            if !self.bus.asic_pri_active() && classic_interrupt {
                self.bus.gate_array.assert_interrupt();
            }
            if self.bus.asic_check_pri() {
                self.bus.gate_array.assert_interrupt();
            }
            if let Some(results) = self.bus.step_asic_dma() {
                for result in results {
                    if result.interrupt {
                        self.cpu.request_interrupt();
                    }
                }
            }
        }
        for _ in 0..events.vsync_rises {
            self.bus.gate_array.vsync_started();
        }
        self.bus.set_vsync_input(self.bus.crtc.vsync());
        self.bus.tick_tape(cycles);
        self.bus.tick_fdc(cycles);

        // CPC Plus/GX4000: a DMA-caused interrupt left unacknowledged
        // stays asserted -- real hardware samples `/INT` every
        // instruction, not just at HSYNC (see
        // `asic::Asic::irq_unacknowledged`'s own doc comment).
        if self.bus.asic_irq_held() {
            self.cpu.request_interrupt();
        }

        self.ay_cycle_debt += cycles;
        let ay_cycles = self.ay_cycle_debt / 4;
        self.ay_cycle_debt %= 4;
        self.bus.ay.tick(ay_cycles);

        self.audio_cycle_accumulator += cycles as f64;
        let cycles_per_sample = self.clock_hz() as f64 / machine::AUDIO_SAMPLE_RATE as f64;
        while self.audio_cycle_accumulator >= cycles_per_sample {
            self.audio_cycle_accumulator -= cycles_per_sample;
            let sample = self.mix_audio_sample();
            self.audio_buffer.push(sample);
        }

        cycles
    }

    fn halted(&self) -> bool {
        self.cpu.halted
    }

    fn pc(&self) -> u16 {
        self.cpu.pc
    }

    fn registers(&self) -> Vec<(&'static str, u64)> {
        self.cpu.registers()
    }

    fn render_frame(&self) -> Option<(u32, u32, Vec<u8>)> {
        let rendered = match self.raster.last_frame() {
            Some(frame) => frame.render(),
            // No field completed yet (fresh or freshly loaded machine):
            // draw the current registers as a still picture.
            None => {
                let (split, scroll) = match &self.bus.asic {
                    Some(asic) if asic.active() => (
                        Some((asic.split_line(), asic.split_address())),
                        asic.v_scroll(),
                    ),
                    _ => (None, 0),
                };
                crate::raster::synthetic_frame(
                    &self.bus.crtc,
                    self.bus.video_state(),
                    self.bus.screen_ram(),
                    split,
                    scroll,
                )
                .render()
            }
        };
        let crate::raster::Rendered {
            width,
            height,
            mut rgb,
            display_origin,
        } = rendered;
        if let Some(asic) = &self.bus.asic
            && asic.active()
        {
            // Sprite coordinates are relative to the display area; with
            // nothing displayed they sit at the standard layout's origin.
            let origin = display_origin
                .unwrap_or(((4 * 16) as u32, crate::raster::VSYNC_TO_WINDOW_LINES as u32));
            crate::plus_video::composite_sprites(&mut rgb, width, height, origin, asic);
        }
        Some((
            width,
            height * 2,
            crate::raster::double_scanlines(width, &rgb),
        ))
    }

    fn read_memory(&self, addr: u16) -> u8 {
        self.bus.peek(addr)
    }

    fn load(&mut self, addr: u16, bytes: &[u8]) -> usize {
        // Same "real hardware never truly drops a write" reasoning as
        // `c64::C64::load` -- RAM is always underneath.
        let mut a = addr;
        for &b in bytes {
            self.bus.write(a, b);
            a = a.wrapping_add(1);
        }
        bytes.len()
    }

    fn set_pc(&mut self, addr: u16) {
        self.cpu.pc = addr;
    }

    fn reset(&mut self) {
        self.cpu = z80::Cpu::new();
    }

    fn drain_audio(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.audio_buffer)
    }

    fn set_port_watches(&mut self, watches: Vec<machine::PortWatch>) -> bool {
        self.bus.port_watch.set_watches(watches);
        true
    }

    fn take_port_hit(&mut self) -> Option<machine::PortHit> {
        self.bus.port_watch.take_hit()
    }

    fn save_state(&self) -> Vec<u8> {
        crate::save_state::encode(self)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        crate::save_state::decode(self, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tape from the (uncommitted) `software/cpc` folder, or `None` -- with a
    /// note on stderr -- so a test that needs it skips instead of failing.
    fn software_tape(name: &str) -> Option<zx_tape::Tape> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../software/cpc/");
        match std::fs::read(format!("{path}{name}")) {
            Ok(bytes) => Some(zx_tape::parse_tzx(&bytes).unwrap()),
            Err(_) => {
                eprintln!("skipping: software/cpc/{name} not present");
                None
            }
        }
    }

    fn test_cpc_model(model: Model) -> Cpc {
        let mut rom = vec![0; model.rom_image_size()];
        rom[..ROM_SIZE].fill(0xAA);
        rom[ROM_SIZE..ROM_SIZE * 2].fill(0xBB);
        if model.has_fdc() {
            rom[ROM_SIZE * 2..].fill(0xCC);
        }
        Cpc::new(rom, model).unwrap()
    }

    /// GX4000 (or an optional Plus-family cartridge, not exercised by
    /// any other test here): a flat `banks * ROM_SIZE`-byte cartridge
    /// image, each bank filled with its own index so tests can identify
    /// which bank is currently visible.
    fn test_cpc_cartridge(model: Model, banks: u8) -> Cpc {
        let mut image = vec![0u8; ROM_SIZE * usize::from(banks)];
        for (bank, chunk) in image.chunks_mut(ROM_SIZE).enumerate() {
            chunk.fill(bank as u8);
        }
        Cpc::new(image, model).unwrap()
    }

    fn test_cpc() -> Cpc {
        test_cpc_model(Model::Cpc464)
    }

    /// Runs `cpc` until the Gate Array raises `/INT`, returning how many
    /// instructions that took.
    fn run_until_interrupt_line(cpc: &mut Cpc) -> u32 {
        for steps in 0..200_000 {
            if cpc.bus.gate_array.interrupt_line() {
                return steps;
            }
            cpc.step();
        }
        panic!("the Gate Array never raised /INT");
    }

    /// A CPC with both ROMs paged out and zeroed RAM (a field of NOPs) from
    /// `0x4000`, IM 1, interrupts disabled.
    fn cpc_running_nops_with_interrupts_disabled() -> Cpc {
        let mut cpc = test_cpc();
        cpc.bus.write_io(0x7F00, 0x8D); // Mode 1, both ROMs off
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
        ] {
            cpc.bus.write_io(0xBC00, register);
            cpc.bus.write_io(0xBD00, value);
        }
        cpc.cpu.pc = 0x4000;
        cpc.cpu.sp = 0xC000;
        cpc.cpu.im = 1;
        cpc.cpu.iff1 = false;
        cpc.cpu.iff2 = false;
        cpc
    }

    /// The Gate Array holds `/INT` until the Z80 answers it: an interrupt that
    /// came while the CPU had interrupts disabled is still there when they are
    /// enabled, and is taken once (not lost, not repeated).
    #[test]
    fn an_interrupt_raised_while_disabled_is_taken_when_interrupts_are_enabled() {
        let mut cpc = cpc_running_nops_with_interrupts_disabled();
        run_until_interrupt_line(&mut cpc);
        // Interrupts are off, so the CPU kept running the NOP field; the line
        // is still asserted a few instructions later.
        for _ in 0..50 {
            cpc.step();
        }
        assert!(cpc.bus.gate_array.interrupt_line());
        assert!(
            cpc.cpu.pc >= 0x4000 && cpc.cpu.pc < 0x8000,
            "{:04X}",
            cpc.cpu.pc
        );

        cpc.cpu.iff1 = true; // as if `EI` had run (and its delay is over)
        cpc.step();
        assert_eq!(cpc.cpu.pc, 0x0038, "taken as soon as the CPU allows it");
        assert!(
            !cpc.bus.gate_array.interrupt_line(),
            "acknowledge drops /INT"
        );
        cpc.cpu.iff1 = true;
        let after = cpc.cpu.pc;
        cpc.step();
        assert_ne!(cpc.cpu.pc, 0x0038, "not delivered twice (was {after:04X})");
    }

    /// An `RMR` write with bit 4 set discards the waiting interrupt.
    #[test]
    fn the_mode_register_delay_bit_discards_a_waiting_interrupt() {
        let mut cpc = cpc_running_nops_with_interrupts_disabled();
        run_until_interrupt_line(&mut cpc);
        cpc.bus.write_io(0x7F00, 0x9D); // bit 4 set
        cpc.cpu.iff1 = true;
        let pc = cpc.cpu.pc;
        cpc.step();
        assert_ne!(cpc.cpu.pc, 0x0038);
        assert!(cpc.cpu.pc >= pc, "kept running NOPs");
    }

    /// Acknowledging the interrupt reaches the Gate Array: bit 5 of its
    /// scanline counter is cleared.
    #[test]
    fn the_cpu_acknowledge_clears_bit_5_of_the_gate_array_counter() {
        let mut cpc = cpc_running_nops_with_interrupts_disabled();
        for _ in 0..40 {
            cpc.bus.gate_array.hsync_falling_edge();
        }
        cpc.bus.gate_array.assert_interrupt();
        cpc.cpu.iff1 = true;
        cpc.step(); // accepted
        for _ in 0..43 {
            assert!(!cpc.bus.gate_array.hsync_falling_edge());
        }
        assert!(
            cpc.bus.gate_array.hsync_falling_edge(),
            "counter was cut from 40 to 8"
        );
    }

    fn each_model_requires_its_standard_combined_rom_size() {
        for model in [
            Model::Cpc464,
            Model::Cpc664,
            Model::Cpc6128,
            Model::Cpc464Plus,
            Model::Cpc6128Plus,
        ] {
            assert!(Cpc::new(vec![0; model.rom_image_size()], model).is_ok());
            assert!(Cpc::new(vec![0; model.rom_image_size() - 1], model).is_err());
        }
    }

    fn gx4000_accepts_any_bank_count_from_one_to_eight_and_rejects_others() {
        assert!(Cpc::new(vec![0; ROM_SIZE], Model::Gx4000).is_ok());
        assert!(Cpc::new(vec![0; ROM_SIZE * 8], Model::Gx4000).is_ok());
        assert!(Cpc::new(Vec::new(), Model::Gx4000).is_err());
        assert!(Cpc::new(vec![0; ROM_SIZE * 9], Model::Gx4000).is_err());
        assert!(Cpc::new(vec![0; ROM_SIZE / 2], Model::Gx4000).is_err());
    }
    /// Merged (every check kept): `each_model_requires_its_standard_combined_rom_size`, `gx4000_accepts_any_bank_count_from_one_to_eight_and_rejects_others`.
    #[test]
    fn rom_sizes() {
        each_model_requires_its_standard_combined_rom_size();
        gx4000_accepts_any_bank_count_from_one_to_eight_and_rejects_others();
    }

    #[test]
    fn disk_models_select_amsdos_in_upper_rom_slot_7() {
        for model in [Model::Cpc664, Model::Cpc6128] {
            let mut cpc = test_cpc_model(model);
            assert_eq!(cpc.bus.peek(0xC000), 0xBB);
            cpc.bus.write_io(0xDF00, 7);
            assert_eq!(cpc.bus.peek(0xC000), 0xCC);
            cpc.bus.write_io(0xDF00, 3);
            assert_eq!(
                cpc.bus.peek(0xC000),
                0xBB,
                "unpopulated upper ROM slots fall back to BASIC"
            );
        }
    }

    #[test]
    fn cpc664_ignores_6128_ram_configuration_writes() {
        let mut cpc = test_cpc_model(Model::Cpc664);
        cpc.bus.gate_array.write(0x8D);
        cpc.bus.ram[0] = 0x42;
        cpc.bus.write_io(0x7F00, 0xC2);
        assert_eq!(cpc.bus.peek(0), 0x42);
        assert_eq!(cpc.bus.ram_configuration(), 0);
    }

    fn disk_models_decode_motor_status_and_data_ports() {
        for model in [Model::Cpc664, Model::Cpc6128] {
            let mut cpc = test_cpc_model(model);
            assert_eq!(cpc.bus.read_io(0xFB7E), 0x80); // RQM, host-to-FDC
            cpc.bus.write_io(0xFA7E, 1);
            assert!(cpc.bus.disk_motor());
            cpc.bus.write_io(0xFB7F, 0x08); // Sense Interrupt Status, none pending
            assert_eq!(cpc.bus.read_io(0xFB7E), 0xD0); // RQM + result direction + busy
            assert_eq!(cpc.bus.read_io(0xFB7F), 0x80); // invalid: nothing pending
            assert_eq!(cpc.bus.read_io(0xFB7E), 0x80);
        }

        assert_eq!(test_cpc().bus.read_io(0xFB7E), 0xFF);
    }

    fn minimal_tap_block_tape() -> zx_tape::Tape {
        // One tiny standard block (flag + checksum). The loader-level
        // validity of these bytes is irrelevant here; we only need a long
        // pulse train to observe position movement over many steps.
        let data = [0x00u8, 0x00u8];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(data.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&data);
        zx_tape::parse_tap(&bytes).unwrap()
    }

    fn one_sector_dsk(fill: u8) -> dsk::Disk {
        const DISC: &[u8] = b"MV - CPCEMU Disk-File\r\nDisk-Info\r\n";
        const TRACK: &[u8] = b"Track-Info\r\n";
        let mut bytes = vec![0; 0x400];
        bytes[..DISC.len()].copy_from_slice(DISC);
        bytes[0x30] = 1; // one track
        bytes[0x31] = 1; // one side
        bytes[0x32..0x34].copy_from_slice(&0x0300u16.to_le_bytes());
        bytes[0x100..0x100 + TRACK.len()].copy_from_slice(TRACK);
        bytes[0x115] = 1; // one sector
        bytes[0x118] = 0; // C
        bytes[0x119] = 0; // H
        bytes[0x11A] = 1; // R
        bytes[0x11B] = 2; // N = 512 bytes
        bytes[0x200..0x400].fill(fill);
        dsk::parse(&bytes).unwrap()
    }

    fn parsed_dsk_sector_reads_through_cpc_fdc_ports() {
        assert!(test_cpc().load_disk(one_sector_dsk(0x5A)).is_err());
        let mut cpc = test_cpc_model(Model::Cpc6128);
        cpc.load_disk(one_sector_dsk(0x5A)).unwrap();
        cpc.bus.write_io(0xFA7E, 1);
        // Read Data + MFM, exactly as AMSDOS sends it.
        for byte in [0x46, 0x00, 0x00, 0x00, 0x01, 0x02, 0x01, 0x2A, 0xFF] {
            cpc.bus.write_io(0xFB7F, byte);
        }

        assert_eq!(cpc.bus.read_io(0xFB7E), 0x10, "first poll: not ready yet");
        assert_eq!(
            cpc.bus.read_io(0xFB7E),
            0xF0,
            "execution phase, controller -> CPU"
        );
        for _ in 0..512 {
            assert_eq!(cpc.bus.read_io(0xFB7F), 0x5A);
        }
        assert_eq!(cpc.bus.read_io(0xFB7E), 0xD0, "result phase");
        let result: Vec<u8> = (0..7).map(|_| cpc.bus.read_io(0xFB7F)).collect();
        // No terminal-count line on a CPC: end-of-cylinder is the normal end.
        assert_eq!(result, [0x40, 0x80, 0, 0, 0, 1, 2]);
        assert_eq!(cpc.bus.read_io(0xFB7E), 0x80);
    }
    /// Merged (every check kept): `disk_models_decode_motor_status_and_data_ports`, `parsed_dsk_sector_reads_through_cpc_fdc_ports`.
    #[test]
    fn fdc_ports() {
        disk_models_decode_motor_status_and_data_ports();
        parsed_dsk_sector_reads_through_cpc_fdc_ports();
    }

    /// A one-sector disk whose Extended DSK "actual data length"
    /// override stores 2 back-to-back real copies (256 bytes each,
    /// filled with `fill_a`/`fill_b`) -- exercises weak/copy-protected
    /// sector cycling end to end through the real CPC FDC ports.
    fn two_copy_weak_sector_dsk(fill_a: u8, fill_b: u8) -> dsk::Disk {
        const EXTENDED_DISC: &[u8] = b"EXTENDED CPC DSK File\r\nDisk-Info\r\n";
        const TRACK: &[u8] = b"Track-Info\r\n";
        let mut bytes = vec![0u8; 0x100];
        bytes[..EXTENDED_DISC.len()].copy_from_slice(EXTENDED_DISC);
        bytes[0x30] = 1; // one track
        bytes[0x31] = 1; // one side
        let track_size = 0x100 + 512; // header + 2x256-byte copies
        bytes[0x34] = (track_size / 256) as u8;

        let mut track = vec![0u8; 0x100];
        track[..TRACK.len()].copy_from_slice(TRACK);
        track[0x14] = 1; // N=1 (256 bytes nominal)
        track[0x15] = 1; // one sector
        track[0x18] = 0; // C
        track[0x19] = 0; // H
        track[0x1A] = 1; // R
        track[0x1B] = 1; // N
        track[0x1E..0x20].copy_from_slice(&512u16.to_le_bytes()); // actual length override
        bytes.extend(track);
        bytes.extend(vec![fill_a; 256]);
        bytes.extend(vec![fill_b; 256]);
        dsk::parse(&bytes).unwrap()
    }

    #[test]
    fn weak_sector_copies_cycle_through_real_cpc_fdc_reads() {
        let mut cpc = test_cpc_model(Model::Cpc6128);
        cpc.load_disk(two_copy_weak_sector_dsk(0x11, 0x22)).unwrap();
        cpc.bus.write_io(0xFA7E, 1);

        let read_one_sector = |cpc: &mut Cpc| -> u8 {
            for byte in [0x06, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x2A, 0xFF] {
                cpc.bus.write_io(0xFB7F, byte);
            }
            let first_byte = cpc.bus.read_io(0xFB7F);
            for _ in 1..256 {
                cpc.bus.read_io(0xFB7F);
            }
            for _ in 0..7 {
                cpc.bus.read_io(0xFB7F);
            }
            first_byte
        };

        assert_eq!(read_one_sector(&mut cpc), 0x11, "first real read: copy 0");
        assert_eq!(read_one_sector(&mut cpc), 0x22, "second real read: copy 1");
        assert_eq!(
            read_one_sector(&mut cpc),
            0x11,
            "third real read wraps back to copy 0"
        );
    }

    #[test]
    fn the_drives_are_modified_and_saved_independently() {
        let mut cpc = test_cpc_model(Model::Cpc6128);
        cpc.load_disk(one_sector_dsk(0x5A)).unwrap();
        cpc.load_disk_in(1, one_sector_dsk(0xC3)).unwrap();
        assert!(cpc.take_modified_disk_image_of(1).is_none());
        // Write a sector to drive B only (Write Data, unit 1).
        cpc.bus.write_io(0xFA7E, 1);
        for byte in [0x45, 0x01, 0x00, 0x00, 0x01, 0x02, 0x01, 0x2A, 0xFF] {
            cpc.bus.write_io(0xFB7F, byte);
        }
        for _ in 0..512 {
            cpc.bus.write_io(0xFB7F, 0x99);
        }
        for _ in 0..7 {
            cpc.bus.read_io(0xFB7F);
        }
        assert!(
            cpc.take_modified_disk_image_of(1).is_some(),
            "drive B was written"
        );
        assert!(cpc.take_modified_disk_image().is_none(), "drive A was not");
        cpc.mark_disk_saved_in(1);
        assert!(cpc.take_modified_disk_image_of(1).is_none());
    }

    /// Real regression: `retroclawd`'s GUI startup path calls
    /// `render_frame()` on the machine exactly as `Cpc::new` leaves it --
    /// no CPU cycles run yet -- to decide the window size before opening
    /// it, and fails cleanly with "no video output implemented" if that's
    /// `None`. With `GateArray::screen_mode` wrongly defaulting to `0`
    /// this returned `None` at construction even though CPC video
    /// *is* implemented (Mode 1), only ever showing up once the ROM
    /// itself had run long enough to correct the mode -- exactly why
    /// `--headless --dump-screen` (renders after cycles run) worked while
    /// the live GUI window did not.
    #[test]
    fn render_frame_works_immediately_at_construction_before_any_cycles_run() {
        let cpc = test_cpc();
        assert!(
            cpc.render_frame().is_some(),
            "a freshly constructed Cpc must already report Mode 1 video, \
             matching real hardware's power-on default"
        );
    }

    fn render_frame_is_the_fixed_monitor_window_in_every_gate_array_mode() {
        // The picture is a fixed 768x576 frame (a 768x288-line window of
        // Mode-2 pixels, each line shown twice -- the 4:3 PAL square-pixel
        // raster) whatever the mode (a split-mode screen can't have a
        // per-mode size).
        for mode in 0..=3 {
            let mut cpc = test_cpc();
            cpc.bus.write_io(0x7F00, 0x80 | mode);
            cpc.bus.gate_array.hsync_falling_edge();
            let (width, height, rgb) = cpc.render_frame().unwrap();
            assert_eq!((width, height), (768, 576), "mode {mode}");
            assert_eq!(rgb.len(), 768 * 576 * 3);
        }
    }

    fn render_frame_doubles_every_scanline_into_a_4_by_3_picture() {
        let mut cpc = test_cpc();
        program_firmware_layout(&mut cpc);
        cpc.bus.write_io(0x7F00, 0x80 | 1);
        cpc.bus.write_io(0x7F00, 0x03); // select pen 3 (what Mode 1 reads 0xFF as)
        cpc.bus.write_io(0x7F00, 0x40 | 0x0B); // pen 3: hardware colour 11
        cpc.flush_out_of_step_video();
        // Give consecutive lines different content: display line d is
        // all-ink only when d is even.
        cpc.bus.ram_mut()[0xC000..0xC000 + 0x800 * 8].fill(0);
        for raster in (0..8).step_by(2) {
            cpc.bus.ram_mut()[0xC000 + raster * 0x800..0xC000 + raster * 0x800 + 80].fill(0xFF);
        }
        two_fields(&mut cpc);
        let (width, height, rgb) = cpc.render_frame().unwrap();
        assert_eq!(width * 3, height * 4, "the presented picture is 4:3");
        let row = |y: u32| &rgb[(y * width * 3) as usize..((y + 1) * width * 3) as usize];
        for line in 0..height / 2 {
            assert_eq!(
                row(line * 2),
                row(line * 2 + 1),
                "line {line} is shown twice"
            );
        }
        // Lines really differ from one another (the doubling isn't hiding a
        // blank picture): display lines 0 and 1 sit on window lines 44 and 45, rows 88 and 90.
        assert_ne!(row(88), row(90));
    }

    /// Reads the Mode 1 text screen back as text by matching every 8x8 cell
    /// against the firmware's own character set (the lower ROM's `&3800`
    /// font): pen 1 on pen 0, so a glyph row `g` is stored as
    /// `byte0 = g & 0xF0`, `byte1 = (g << 4)`.
    fn read_mode1_text(ram: &[u8], rom: &[u8]) -> Vec<String> {
        let glyph =
            |ch: usize| -> [u8; 8] { std::array::from_fn(|row| rom[0x3800 + ch * 8 + row]) };
        (0..25)
            .map(|row| {
                (0..40)
                    .map(|column| {
                        let cell: [u8; 8] = std::array::from_fn(|line| {
                            let at = 0xC000 + row * 80 + column * 2 + line * 0x800;
                            (ram[at] & 0xF0) | (ram[at + 1] >> 4)
                        });
                        match (32..256usize).find(|&ch| glyph(ch) == cell) {
                            Some(0xA4) => '\u{a9}',
                            Some(ch) => char::from(ch as u8),
                            None if cell == [0; 8] => ' ',
                            None => '?',
                        }
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// Boots each real firmware ROM (skipped when a ROM isn't present) and
    /// reads its banner back off the screen as text through the ROM's own
    /// character set -- each model's own version string, copyright year and
    /// BASIC version, then the `Ready` prompt.
    #[test]
    fn real_firmware_boots_to_each_models_own_banner() {
        let roms = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../roms/cpc/");
        let mut checked = 0;
        for (model, file, version, year, basic) in [
            (
                Model::Cpc464,
                "cpc464.rom",
                "64K Microcomputer  (v1)",
                "1984",
                "1.0",
            ),
            (
                Model::Cpc664,
                "cpc664.rom",
                "64K Microcomputer  (v2)",
                "1984",
                "1.1",
            ),
            (
                Model::Cpc6128,
                "cpc6128.rom",
                "128K Microcomputer  (v3)",
                "1985",
                "1.1",
            ),
        ] {
            let Ok(rom) = std::fs::read(format!("{roms}{file}")) else {
                eprintln!("skipping {file}: ROM not present");
                continue;
            };
            let mut cpc = Cpc::new(rom.clone(), model).unwrap();
            let mut spent = 0u64;
            while spent < 5_000_000 {
                spent += u64::from(cpc.step());
            }
            let text = read_mode1_text(cpc.bus.screen_ram(), &rom);
            let expected = [
                (1, format!(" Amstrad {version}")),
                (3, format!(" \u{a9}{year} Amstrad Consumer Electronics plc")),
                (4, "           and Locomotive Software Ltd.".to_string()),
                (6, format!(" BASIC {basic}")),
                (8, "Ready".to_string()),
            ];
            for (row, line) in expected {
                assert_eq!(text[row], line, "{file} text row {row}");
            }
            checked += 1;
        }
        eprintln!("boot banner verified on {checked} real ROM(s)");
    }

    /// The CPC+ and GX4000 need their own firmware/cartridge, which isn't
    /// shipped in `roms/cpc` -- so these only run when someone drops
    /// `cpc464plus.rom`/`cpc6128plus.rom`/`gx4000.rom` (a cartridge image
    /// for the GX4000) there, and assert nothing a correctly booting machine
    /// wouldn't do: it runs for a second of emulated time without trapping,
    /// completes video fields, and has display on. They have never run
    /// against a real ROM here, so treat a pass as a smoke test, not a proof.
    #[test]
    fn plus_and_gx4000_firmware_smoke_when_roms_are_present() {
        let roms = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../roms/cpc/");
        for (model, file) in [
            (Model::Cpc464Plus, "cpc464plus.rom"),
            (Model::Cpc6128Plus, "cpc6128plus.rom"),
            (Model::Gx4000, "gx4000.rom"),
        ] {
            let Ok(rom) = std::fs::read(format!("{roms}{file}")) else {
                eprintln!("skipping {file}: ROM not present");
                continue;
            };
            let mut cpc = Cpc::new(rom, model).unwrap();
            let mut spent = 0u64;
            while spent < 4_000_000 {
                spent += u64::from(cpc.step());
            }
            assert!(
                cpc.raster.frames_completed() > 10,
                "{file}: no video fields"
            );
            let (width, height, _) = cpc.render_frame().unwrap();
            assert_eq!((width, height), (768, 576), "{file}");
        }
    }

    /// Boots each real firmware ROM (skipped when a ROM isn't present) and
    /// checks the rendered monitor window against an independent decode of
    /// screen RAM: the classic 80-byte row / 2K raster-interleave layout,
    /// the Gate Array's per-mode bit layout, the pens the Gate Array holds,
    /// and the fixed window geometry (4 characters of border either side,
    /// 44 lines above and below the 640x200 display).
    #[test]
    fn real_firmware_boot_screen_matches_an_independent_decode_of_screen_ram() {
        use crate::video::{decode_mode0, decode_mode1, decode_mode2};
        let roms = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../roms/cpc/");
        let mut checked = 0;
        for (model, file) in [
            (Model::Cpc464, "cpc464.rom"),
            (Model::Cpc664, "cpc664.rom"),
            (Model::Cpc6128, "cpc6128.rom"),
        ] {
            let Ok(rom) = std::fs::read(format!("{roms}{file}")) else {
                eprintln!("skipping {file}: ROM not present");
                continue;
            };
            let mut cpc = Cpc::new(rom, model).unwrap();
            // Boot to the BASIC banner, then look for a field during which
            // the screen didn't change (the cursor blink is the only thing
            // still moving) and check that field's render.
            let mut spent = 0u64;
            while spent < 5_000_000 {
                spent += u64::from(cpc.step());
            }
            let screen = |cpc: &Cpc| cpc.bus.screen_ram()[0xC000..].to_vec();
            let mut before = screen(&cpc);
            let mut verified = false;
            for _ in 0..40 {
                let frames = cpc.raster.frames_completed();
                while cpc.raster.frames_completed() == frames {
                    cpc.step();
                }
                let after = screen(&cpc);
                let stable = before == after;
                before = after;
                if !stable {
                    continue;
                }
                let (width, height, rgb) = cpc.render_frame().unwrap();
                assert_eq!((width, height), (768, 576), "{file}");
                let mode = cpc.bus.gate_array.screen_mode();
                assert_eq!(mode, 1, "{file}: firmware boots into Mode 1");
                let pen = |p: u8| {
                    let (r, g, b) = cpc.bus.gate_array.pen_color(p);
                    [r, g, b]
                };
                // Addressed by window line; each line is presented twice.
                let at = |x: u32, line: u32| {
                    let i = (((line * 2) * width + x) * 3) as usize;
                    let j = i + (width * 3) as usize;
                    assert_eq!(rgb[i..i + 3], rgb[j..j + 3], "{file}: line {line} doubled");
                    [rgb[i], rgb[i + 1], rgb[i + 2]]
                };
                // Border: window corners and the strips around the display.
                let border = pen(16);
                assert_eq!(at(0, 0), border, "{file} top-left");
                assert_eq!(at(width - 1, height / 2 - 1), border, "{file} bottom-right");
                assert_eq!(at(63, 100), border, "{file} left border");
                assert_eq!(at(64 + 640, 100), border, "{file} right border");
                assert_eq!(at(300, 43), border, "{file} top border");
                assert_eq!(at(300, 44 + 200), border, "{file} bottom border");
                // Display: every pixel of all 200 lines.
                let ram = cpc.bus.screen_ram();
                for row in 0..25usize {
                    for raster in 0..8usize {
                        for column in 0..80usize {
                            let byte = ram[0xC000 + row * 80 + raster * 0x800 + column];
                            let x0 = 64 + column as u32 * 8;
                            let y = 44 + (row * 8 + raster) as u32;
                            let expected: Vec<[u8; 3]> = match mode {
                                0 => {
                                    let [l, r] = decode_mode0(byte);
                                    [l, l, l, l, r, r, r, r].iter().map(|&p| pen(p)).collect()
                                }
                                1 => (0..4u8)
                                    .flat_map(|p| {
                                        let c = pen(decode_mode1(byte, p));
                                        [c, c]
                                    })
                                    .collect(),
                                _ => decode_mode2(byte).iter().map(|&p| pen(p)).collect(),
                            };
                            for (dx, want) in expected.into_iter().enumerate() {
                                assert_eq!(
                                    at(x0 + dx as u32, y),
                                    want,
                                    "{file}: row {row} raster {raster} column {column} dot {dx}"
                                );
                            }
                        }
                    }
                }
                verified = true;
                break;
            }
            assert!(verified, "{file}: never saw a stable field to check");
            checked += 1;
        }
        eprintln!("boot-screen decode verified on {checked} real ROM(s)");
    }

    /// Programs the standard firmware CRTC layout through the ports and
    /// leaves the machine ready for `advance_video` to be driven directly.
    fn program_firmware_layout(cpc: &mut Cpc) {
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
            cpc.bus.write_io(0xBC00, register);
            cpc.bus.write_io(0xBD00, value);
        }
        cpc.bus.crtc.reload_start_address();
        cpc.flush_out_of_step_video();
    }

    /// Advances video by `characters` (a multiple of 4) in 16-T-state calls,
    /// so the CRTC's character phase stays at zero between calls.
    fn advance_characters(cpc: &mut Cpc, characters: u32) {
        assert_eq!(characters % 4, 0);
        for _ in 0..characters / 4 {
            cpc.advance_video(16);
        }
    }

    /// A pixel of `render_frame`'s output addressed by *window line* (the
    /// undoubled row space the trace and CPC+ sprites use): every line is
    /// presented twice, so line `y` is output row `2 * y` (and `2 * y + 1`,
    /// which `render_frame_doubles_every_scanline` pins separately).
    fn pixel_of(rendered: &(u32, u32, Vec<u8>), x: u32, y: u32) -> (u8, u8, u8) {
        let i = (((y * 2) * rendered.0 + x) * 3) as usize;
        (rendered.2[i], rendered.2[i + 1], rendered.2[i + 2])
    }

    fn an_ink_write_lands_on_the_character_clock_it_happened_on() {
        let mut cpc = test_cpc();
        program_firmware_layout(&mut cpc);
        cpc.bus.write_io(0x7F00, 0x80 | 2); // Mode 2
        cpc.bus.write_io(0x7F00, 0x10); // select the border
        cpc.bus.write_io(0x7F00, 0x40 | 0x04); // border ink A (hardware colour 4)
        cpc.flush_out_of_step_video();
        let ink_a = cpc.bus.gate_array.pen_color(16);

        // Frame N runs from the first VSYNC (line 240) to the next (552).
        // Stop at character 8 of line 281 -- 41 lines into that frame, i.e.
        // window line 13, in the top border (the window opens at line 28).
        advance_characters(&mut cpc, 64 * 281 + 8);
        // A write whose bus cycle ended 9 T-states into a 16-T-state span:
        // two whole characters (8 T) have already passed.
        cpc.bus.instruction_advanced_tstates = 9;
        cpc.bus.write_io(0x7F00, 0x40 | 0x0B); // border ink B (hardware colour 11)
        let ink_b = cpc.bus.gate_array.pen_color(16);
        assert_ne!(ink_a, ink_b);
        cpc.advance_video(16);
        // Finish that frame and the one after (so it becomes `last`).
        advance_characters(&mut cpc, 64 * (553 - 281) - 8);

        let rendered = cpc.render_frame().unwrap();
        // The write landed on line character 8 + 2 = 10, which the window
        // (starting at line character 60) shows at window character 14.
        let first_b = 14 * 16;
        assert_eq!(pixel_of(&rendered, first_b - 1, 13), ink_a);
        assert_eq!(pixel_of(&rendered, first_b, 13), ink_b);
        assert_eq!(pixel_of(&rendered, 767, 13), ink_b);
        // Earlier rows kept A; later rows are B throughout.
        assert_eq!(pixel_of(&rendered, 767, 12), ink_a);
        assert_eq!(pixel_of(&rendered, 0, 14), ink_b);
    }

    fn a_crtc_register_write_takes_effect_on_the_character_it_landed_on() {
        let mut cpc = test_cpc();
        program_firmware_layout(&mut cpc);
        cpc.bus.write_io(0x7F00, 0x80 | 2); // Mode 2
        cpc.bus.write_io(0x7F00, 0x01); // select pen 1
        cpc.bus.write_io(0x7F00, 0x40 | 0x0B); // pen 1: hardware colour 11
        cpc.bus.write_io(0x7F00, 0x10);
        cpc.bus.write_io(0x7F00, 0x40 | 0x04); // border: hardware colour 4
        cpc.flush_out_of_step_video();
        let paper = cpc.bus.gate_array.pen_color(1);
        let border = cpc.bus.gate_array.pen_color(16);
        assert_ne!(paper, border);
        // All-ink screen so displayed characters are pen 1.
        for byte in cpc.bus.ram_mut().iter_mut() {
            *byte = 0xFF;
        }

        // Character 8 of CRTC line 332 (frame line 92 of the field that
        // starts at line 240: window line 64, a display line).
        advance_characters(&mut cpc, 64 * 332 + 8);
        // R1 (characters displayed) 40 -> 9, written 9 T-states in: the two
        // characters already clocked (8 and 9) saw the old value, so line
        // 332 shows characters 0..=9; the following lines only 0..=8.
        cpc.bus.write_io(0xBC00, 1);
        cpc.bus.instruction_advanced_tstates = 9;
        cpc.bus.write_io(0xBD00, 9);
        cpc.advance_video(16);
        advance_characters(&mut cpc, 64 * (553 - 332) - 8);

        let rendered = cpc.render_frame().unwrap();
        // Display character c is window character c + 4.
        let x = |character: u32| (character + 4) * 16;
        assert_eq!(
            pixel_of(&rendered, x(9), 64),
            paper,
            "character 9 still displayed"
        );
        assert_eq!(
            pixel_of(&rendered, x(10), 64),
            border,
            "character 10 is border"
        );
        assert_eq!(
            pixel_of(&rendered, x(30), 63),
            paper,
            "line before: full width"
        );
        assert_eq!(pixel_of(&rendered, x(8), 65), paper);
        assert_eq!(
            pixel_of(&rendered, x(9), 65),
            border,
            "next line: only 0..=8"
        );
    }
    /// Merged (every check kept): `an_ink_write_lands_on_the_character_clock_it_happened_on`, `a_crtc_register_write_takes_effect_on_the_character_it_landed_on`.
    #[test]
    fn writes_land_on_their_character_clock() {
        an_ink_write_lands_on_the_character_clock_it_happened_on();
        a_crtc_register_write_takes_effect_on_the_character_it_landed_on();
    }

    #[test]
    fn gate_array_interrupt_reaches_the_cpu_from_crtc_hsync_edges() {
        let mut cpc = test_cpc();
        cpc.bus.gate_array.write(0x85); // Mode 1, lower ROM disabled
        cpc.bus.write(0x0300, 0x00); // NOP
        cpc.set_pc(0x0300);
        cpc.cpu.iff1 = true;
        cpc.cpu.im = 1;

        // Four CRTC characters per line, one-character HSYNC.
        for (register, value) in [(0, 3), (2, 1), (3, 1), (4, 127), (7, 127), (9, 31)] {
            cpc.bus.crtc.select_register(register);
            cpc.bus.crtc.write_register(value);
        }

        let mut fired = false;
        for _ in 0..300 {
            cpc.step();
            if cpc.pc() == 0x0038 {
                fired = true;
                break;
            }
        }
        assert!(
            fired,
            "Gate Array's 52-HSYNC interrupt should have reached the CPU"
        );
    }

    #[test]
    fn memory_accesses_wait_for_one_in_four_gate_array_bus_slots() {
        let mut cpc = test_cpc();
        cpc.bus.gate_array.write(0x85); // expose RAM below lower ROM
        cpc.bus.write(0x0200, 0x3E); // LD A,n: fetch + immediate read
        cpc.bus.write(0x0201, 0x42);
        cpc.bus.write(0x0202, 0x00); // NOP fetch follows at phase 3
        cpc.set_pc(0x0200);

        assert_eq!(cpc.step(), 7);
        assert_eq!(cpc.cpu.a(), 0x42);
        assert_eq!(cpc.bus.bus_tstate_phase(), 3);
        assert_eq!(cpc.step(), 5, "next fetch must wait one T-state");
        assert_eq!(cpc.bus.bus_tstate_phase(), 0);
    }

    fn crtc_vsync_is_visible_on_ppi_port_b_bit_zero() {
        let mut cpc = test_cpc();
        for (register, value) in [(0, 0), (3, 0x20), (4, 3), (7, 1), (9, 0)] {
            cpc.bus.crtc.select_register(register);
            cpc.bus.crtc.write_register(value);
        }

        cpc.step();
        assert_eq!(cpc.bus.read_io(0xF500) & 1, 1);
        cpc.step();
        cpc.step();
        assert_eq!(cpc.bus.read_io(0xF500) & 1, 0);
    }
    fn ppi_port_b_exposes_fixed_status_lines() {
        let mut cpc = test_cpc();

        cpc.bus.set_vsync_input(false);
        let low_vsync = cpc.bus.read_io(0xF500);
        assert_eq!(low_vsync & 0x1E, 0x1E, "bits 1..4 are fixed high");
        assert_eq!(low_vsync & 0x60, 0x00, "bits 5..6 are fixed low");
        assert_eq!(low_vsync & 0x80, 0x80, "idle cassette input reads high");
        assert_eq!(low_vsync & 0x01, 0x00, "vsync low should clear bit 0");

        cpc.bus.set_vsync_input(true);
        let high_vsync = cpc.bus.read_io(0xF500);
        assert_eq!(high_vsync & 0x01, 0x01, "vsync high should set bit 0");
    }
    /// Merged (every check kept): `crtc_vsync_is_visible_on_ppi_port_b_bit_zero`, `ppi_port_b_exposes_fixed_status_lines`.
    #[test]
    fn ppi_port_b_lines() {
        crtc_vsync_is_visible_on_ppi_port_b_bit_zero();
        ppi_port_b_exposes_fixed_status_lines();
    }

    #[test]
    fn ejecting_the_tape_stops_playback_and_idles_the_ear_input() {
        let mut cpc = test_cpc();
        cpc.load_tape(minimal_tap_block_tape());
        assert!(cpc.has_tape());
        cpc.eject_tape();
        assert!(!cpc.has_tape());
        assert!(cpc.bus.tape_playback_state().is_none());
        // A new tape can go straight in.
        cpc.load_tape(minimal_tap_block_tape());
        assert!(cpc.has_tape());
    }

    #[test]
    fn load_tape_prepends_the_default_leader_and_zero_disables_it() {
        let bare = minimal_tap_block_tape();
        let mut cpc = test_cpc();
        assert_eq!(cpc.tape_leader_ms(), DEFAULT_TAPE_LEADER_MS);
        cpc.load_tape(bare.clone());
        // Playing: the first thing on the tape is the silent leader, so the
        // position stays at event 0 until the leader has played out.
        cpc.bus.write_io(0xF700, 0x82);
        cpc.bus.write_io(0xF700, 0x09);
        cpc.bus.tick_tape(4_000_000); // 1 s at 4 MHz
        assert_eq!(cpc.bus.tape_playback_state().unwrap().position, 0);
        assert_eq!(cpc.bus.tape_ear_bit(), Some(false), "leader is low");
        cpc.bus.tick_tape(4_400_000); // past 2 s in total
        assert!(cpc.bus.tape_playback_state().unwrap().position > 0);

        let mut cpc = test_cpc();
        cpc.set_tape_leader_ms(0);
        cpc.load_tape(bare);
        cpc.bus.write_io(0xF700, 0x82);
        cpc.bus.write_io(0xF700, 0x09);
        cpc.bus.tick_tape(4_000_000);
        assert!(
            cpc.bus.tape_playback_state().unwrap().position > 0,
            "no leader: the first block's pilot plays straight away"
        );
    }

    fn default_tape_motor_mode_is_caprice_strict() {
        let cpc = test_cpc();
        assert_eq!(cpc.bus.tape_motor_mode(), TapeMotorMode::CapriceStrict);
    }

    fn cassette_playback_continues_during_brief_motor_glitch_in_zesarux_compat_mode() {
        let mut cpc = test_cpc();
        cpc.bus
            .set_tape_motor_mode(TapeMotorMode::ZesaruxCompatibility);
        cpc.set_tape_leader_ms(0); // these tests drive the tape from its first pulse
        cpc.load_tape(minimal_tap_block_tape());

        assert_eq!(cpc.bus.read_io(0xF500) & 0x80, 0x80);

        cpc.bus.write_io(0xF700, 0x82); // PPI mode set: PA/PC output
        cpc.bus.write_io(0xF700, 0x09); // bit-set Port C bit 4 (cassette motor on)
        assert!(cpc.bus.cassette_motor());

        for _ in 0..800 {
            cpc.step();
        }
        let pos_before_off = cpc.bus.tape_playback_state().unwrap().position;
        assert!(pos_before_off > 0);

        cpc.bus.write_io(0xF700, 0x08); // cassette motor off
        assert!(!cpc.bus.cassette_motor());
        cpc.bus.tick_tape(100_000);
        let state_after_off = cpc.bus.tape_playback_state().unwrap();
        assert!(
            state_after_off.playing && !state_after_off.finished,
            "explicit ZesaruxCompatibility opt-in keeps playback running through motor-off"
        );
        cpc.bus.write_io(0xF700, 0x09); // motor back on
        for _ in 0..200 {
            cpc.step();
        }
        assert!(cpc.bus.tape_playback_state().unwrap().playing);
    }

    fn cassette_playback_stops_immediately_after_motor_off() {
        let mut cpc = test_cpc();
        cpc.bus.set_tape_motor_mode(TapeMotorMode::CapriceStrict);
        cpc.set_tape_leader_ms(0); // these tests drive the tape from its first pulse
        cpc.load_tape(minimal_tap_block_tape());

        cpc.bus.write_io(0xF700, 0x82); // PPI mode set: PA/PC output
        cpc.bus.write_io(0xF700, 0x09); // cassette motor on
        for _ in 0..800 {
            cpc.step();
        }
        let pos_before_off = cpc.bus.tape_playback_state().unwrap().position;

        cpc.bus.write_io(0xF700, 0x08); // cassette motor off
        let stopped = cpc.bus.tape_playback_state().unwrap();
        assert_eq!(stopped.position, pos_before_off);
        assert!(!stopped.playing);
        assert_eq!(cpc.bus.read_io(0xF500) & 0x80, 0x80, "EAR idles high");

        cpc.bus.tick_tape(CLOCK_HZ as u32);
        assert_eq!(
            cpc.bus.tape_playback_state().unwrap().position,
            pos_before_off
        );
    }

    #[test]
    fn jet_set_willy_motor_off_does_not_consume_tape_forever() {
        let mut cpc = test_cpc();
        cpc.bus.set_tape_motor_mode(TapeMotorMode::CapriceStrict);
        let Some(tape) = software_tape("Jet Set Willy.cdt") else {
            return;
        };
        cpc.set_tape_leader_ms(0); // these tests drive the tape from its first pulse
        cpc.load_tape(tape);

        cpc.bus.write_io(0xF700, 0x82); // PPI mode set: PA/PC output
        cpc.bus.write_io(0xF700, 0x09); // cassette motor on
        while cpc.bus.tape_playback_state().unwrap().position < 66_386 {
            cpc.bus.tick_tape(50_000);
        }
        let before_off = cpc.bus.tape_playback_state().unwrap().position;

        cpc.bus.write_io(0xF700, 0x08); // cassette motor off
        let stopped = cpc.bus.tape_playback_state().unwrap();

        assert_eq!(stopped.position, before_off);
        assert!(!stopped.playing);
        assert_eq!(cpc.bus.read_io(0xF500) & 0x80, 0x80, "EAR idles high");

        cpc.bus.tick_tape(CLOCK_HZ as u32);
        assert_eq!(cpc.bus.tape_playback_state().unwrap().position, before_off);
    }

    #[test]
    fn tape_pulse_widths_report_real_pilot_tone_timing() {
        let mut cpc = test_cpc();
        cpc.set_tape_leader_ms(0); // these tests drive the tape from its first pulse
        cpc.load_tape(minimal_tap_block_tape());

        cpc.bus.write_io(0xF700, 0x82); // PPI mode set: PA/PC output
        cpc.bus.write_io(0xF700, 0x09); // bit-set Port C bit 4 (cassette motor on)
        assert!(cpc.bus.cassette_motor());

        for _ in 0..20_000 {
            cpc.step();
        }

        let widths = cpc.bus.tape_pulse_widths_samples();
        assert!(
            !widths.is_empty(),
            "expected at least one completed pulse width by now"
        );
        // Standard ROM pilot tone is 2168 T-states/pulse (see CREDITS.md);
        // a 2-byte block's pilot run (8063 or 3223 pulses depending on
        // header/data classification) is far longer than 20,000 CPU steps
        // can traverse, so every captured pulse here should still be pilot
        // tone. Small tolerance for the CPC<->tape clock-domain scaling's
        // own rounding.
        for &width in &widths {
            assert!(
                (2100..=2250).contains(&width),
                "expected pilot-tone-width pulses (~2168T), got {width} in {widths:?}"
            );
        }
    }

    #[test]
    fn cassette_motor_drop_pauses_arkanoid_turbo_tail() {
        let mut cpc = test_cpc();
        cpc.bus.set_tape_motor_mode(TapeMotorMode::CapriceStrict);
        let Some(tape) = software_tape("Arkanoid.cdt") else {
            return;
        };
        cpc.set_tape_leader_ms(0); // these tests drive the tape from its first pulse
        cpc.load_tape(tape);

        cpc.bus.write_io(0xF700, 0x82); // PPI mode set: PA/PC output
        cpc.bus.write_io(0xF700, 0x09); // cassette motor on

        while cpc.bus.tape_playback_state().unwrap().position < 45_570 {
            cpc.bus.tick_tape(50_000);
        }
        let before_off = cpc.bus.tape_playback_state().unwrap().position;
        assert!(before_off < 45_604);

        cpc.bus.write_io(0xF700, 0x08); // cassette motor off
        for _ in 0..900 {
            cpc.bus.tick_tape(50_000);
        }

        let after_off = cpc.bus.tape_playback_state().unwrap();
        assert_eq!(after_off.position, before_off);
        assert!(!after_off.playing);
    }

    fn ppi_mode_set_does_not_drop_cassette_motor_latch() {
        let mut cpc = test_cpc();
        cpc.bus.write_io(0xF700, 0x82);
        cpc.bus.write_io(0xF700, 0x09);
        assert!(cpc.bus.cassette_motor());

        cpc.bus.write_io(0xF700, 0x82);
        assert!(cpc.bus.cassette_motor());
    }

    fn ppi_bsr_non_motor_bits_do_not_drop_cassette_motor() {
        let mut cpc = test_cpc();
        cpc.bus.write_io(0xF700, 0x82);
        cpc.bus.write_io(0xF700, 0x09); // set Port C bit 4 (motor on)
        assert!(cpc.bus.cassette_motor());

        // BSR writes for bits other than bit 4 must not affect cassette motor.
        for cmd in [
            0x00_u8, 0x01, // bit 0 clear/set
            0x02, 0x03, // bit 1 clear/set
            0x04, 0x05, // bit 2 clear/set
            0x06, 0x07, // bit 3 clear/set
            0x0A, 0x0B, // bit 5 clear/set
            0x0C, 0x0D, // bit 6 clear/set
            0x0E, 0x0F, // bit 7 clear/set
        ] {
            cpc.bus.write_io(0xF700, cmd);
            assert!(
                cpc.bus.cassette_motor(),
                "BSR control write 0x{cmd:02X} must not drop cassette motor"
            );
        }
    }

    fn ppi_port_c_data_writes_drive_the_cassette_motor_from_bit_4() {
        let mut cpc = test_cpc();
        cpc.bus.write_io(0xF700, 0x82); // PA/PC output
        assert!(!cpc.bus.cassette_motor());

        // A Port C data write with bit 4 set starts the motor (the ROM's own
        // cassette start-up and custom loaders do exactly this) ...
        cpc.bus.write_io(0xF600, 0x10);
        assert!(cpc.bus.cassette_motor());

        // ... AY/keyboard handshake values that keep bit 4 high leave it on ...
        for v in [0x10_u8, 0x50, 0x90, 0xD0, 0x9F, 0xDF] {
            cpc.bus.write_io(0xF600, v);
            assert!(
                cpc.bus.cassette_motor(),
                "Port-C data write 0x{v:02X} keeps bit 4 high so the motor stays on"
            );
        }

        // ... and one that clears it stops the motor.
        cpc.bus.write_io(0xF600, 0x40);
        assert!(!cpc.bus.cassette_motor());
        cpc.bus.write_io(0xF600, 0xC0);
        assert!(!cpc.bus.cassette_motor());
    }
    /// Merged (every check kept): `ppi_mode_set_does_not_drop_cassette_motor_latch`, `ppi_bsr_non_motor_bits_do_not_drop_cassette_motor`, `ppi_port_c_data_writes_drive_the_cassette_motor_from_bit_4`.
    #[test]
    fn ppi_cassette_motor_latch() {
        ppi_mode_set_does_not_drop_cassette_motor_latch();
        ppi_bsr_non_motor_bits_do_not_drop_cassette_motor();
        ppi_port_c_data_writes_drive_the_cassette_motor_from_bit_4();
    }

    fn a_port_c_data_write_resumes_a_tape_the_motor_off_had_paused() {
        let mut cpc = test_cpc();
        cpc.bus.set_tape_motor_mode(TapeMotorMode::CapriceStrict);
        let Some(tape) = software_tape("Arkanoid.cdt") else {
            return;
        };
        cpc.set_tape_leader_ms(0); // these tests drive the tape from its first pulse
        cpc.load_tape(tape);
        cpc.bus.write_io(0xF700, 0x82);
        cpc.bus.write_io(0xF700, 0x09); // motor on (bit-set/reset)
        for _ in 0..40 {
            cpc.bus.tick_tape(50_000);
        }
        cpc.bus.write_io(0xF700, 0x08); // motor off
        let paused_at = cpc.bus.tape_playback_state().unwrap().position;
        assert!(!cpc.bus.tape_playback_state().unwrap().playing);
        for _ in 0..40 {
            cpc.bus.tick_tape(50_000);
        }
        assert_eq!(cpc.bus.tape_playback_state().unwrap().position, paused_at);

        // A custom loader (Arkanoid's second stage) restarts it with a plain
        // Port C data write, no bit-set/reset command.
        cpc.bus.write_io(0xF600, 0x10);
        for _ in 0..40 {
            cpc.bus.tick_tape(50_000);
        }
        assert!(cpc.bus.tape_playback_state().unwrap().position > paused_at);
    }
    /// Merged (every check kept): `default_tape_motor_mode_is_caprice_strict`, `cassette_playback_continues_during_brief_motor_glitch_in_zesarux_compat_mode`, `cassette_playback_stops_immediately_after_motor_off`, `a_port_c_data_write_resumes_a_tape_the_motor_off_had_paused`.
    #[test]
    fn tape_motor_modes() {
        default_tape_motor_mode_is_caprice_strict();
        cassette_playback_continues_during_brief_motor_glitch_in_zesarux_compat_mode();
        cassette_playback_stops_immediately_after_motor_off();
        a_port_c_data_write_resumes_a_tape_the_motor_off_had_paused();
    }

    #[test]
    fn audio_samples_are_generated_once_a_tone_is_configured() {
        let mut cpc = test_cpc();

        // Configure the AY directly through the real Port A + Port C
        // protocol (already proven end-to-end by the PSG round-trip test
        // below) -- channel A: tone period, tone enabled in the mixer,
        // full volume, no envelope.
        cpc.bus.write_io(0xF700, 0x82); // PA output, PC output
        let write_register = |cpc: &mut Cpc, reg: u8, val: u8| {
            cpc.bus.write_io(0xF400, reg);
            cpc.bus.write_io(0xF600, 0xC0); // select
            cpc.bus.write_io(0xF400, val);
            cpc.bus.write_io(0xF600, 0x80); // write
        };
        write_register(&mut cpc, 0, 0x10); // tone period A, low byte
        write_register(&mut cpc, 1, 0x00); // tone period A, high byte
        write_register(&mut cpc, 7, 0x3E); // mixer: tone A enabled, everything else disabled
        write_register(&mut cpc, 8, 0x0F); // channel A volume: max, no envelope

        // Real time must actually pass for a sample period to elapse --
        // NOPs in RAM at the default PC (0x0000 is lower ROM, banked out
        // by nothing here, so use RAM at $0300 instead, same pattern the
        // other tests here use).
        cpc.set_pc(0x0300);
        for _ in 0..1000 {
            cpc.step();
        }

        let samples = cpc.drain_audio();
        assert!(
            !samples.is_empty(),
            "stepping past a sample period should produce audio"
        );
        assert!(
            samples.iter().any(|&s| s != 0.0),
            "a configured, audible tone should not be silence"
        );
    }

    fn keyboard_scan_reaches_the_cpu_through_ppi_and_ay_register_14() {
        let mut cpc = test_cpc();
        cpc.press_key(crate::keyboard::CpcKey::A); // line 8, bit 5

        cpc.bus.write_io(0xF700, 0x82); // PA output, PC output
        cpc.bus.write_io(0xF400, 0x0E); // PA = register index 14 (keyboard)
        cpc.bus.write_io(0xF600, 0xC0); // Port C: select-register function

        cpc.bus.write_io(0xF700, 0x92); // reconfigure: PA now input
        cpc.bus.write_io(0xF600, 0x48); // Port C: read function, row 8 selected (bits 3:0)
        assert_eq!(
            cpc.bus.read_io(0xF400) & (1 << 5),
            0,
            "A's bit should read low on line 8 once scanned via register 14"
        );

        cpc.bus.write_io(0xF600, 0x47); // read function, row 7 instead
        assert_eq!(
            cpc.bus.read_io(0xF400),
            0xFF,
            "A shouldn't show up on a line it isn't wired to"
        );
    }

    fn joystick_2_keys_reach_the_cpu_on_matrix_line_6() {
        let mut cpc = test_cpc();
        cpc.press_key(crate::keyboard::CpcKey::Joy2Fire1); // line 6, bit 4
        cpc.press_key(crate::keyboard::CpcKey::Joy2Left); // line 6, bit 2

        cpc.bus.write_io(0xF700, 0x82); // PA output, PC output
        cpc.bus.write_io(0xF400, 0x0E); // PA = register index 14 (keyboard)
        cpc.bus.write_io(0xF600, 0xC0); // Port C: select-register function

        cpc.bus.write_io(0xF700, 0x92); // reconfigure: PA now input
        cpc.bus.write_io(0xF600, 0x46); // Port C: read function, row 6 selected
        let line6 = cpc.bus.read_io(0xF400);
        assert_eq!(line6 & (1 << 4), 0, "Joy2Fire1 should read low on line 6");
        assert_eq!(line6 & (1 << 2), 0, "Joy2Left should read low on line 6");
        assert_ne!(line6 & (1 << 3), 0, "Joy2Right wasn't pressed");

        // The AY read is latched when the Port C read function is issued, so
        // a released stick shows up on the next scan.
        cpc.release_key(crate::keyboard::CpcKey::Joy2Fire1);
        cpc.bus.write_io(0xF600, 0x46);
        assert_ne!(cpc.bus.read_io(0xF400) & (1 << 4), 0);
    }

    fn joystick_1_keys_reach_the_cpu_on_matrix_line_9() {
        let mut cpc = test_cpc();
        cpc.press_key(crate::keyboard::CpcKey::Joy1Fire1); // line 9, bit 4
        cpc.press_key(crate::keyboard::CpcKey::Joy1Up); // line 9, bit 0

        cpc.bus.write_io(0xF700, 0x82); // PA output, PC output
        cpc.bus.write_io(0xF400, 0x0E); // PA = register index 14 (keyboard)
        cpc.bus.write_io(0xF600, 0xC0); // Port C: select-register function

        cpc.bus.write_io(0xF700, 0x92); // reconfigure: PA now input
        cpc.bus.write_io(0xF600, 0x49); // Port C: read function, row 9 selected
        let line9 = cpc.bus.read_io(0xF400);
        assert_eq!(line9 & (1 << 4), 0, "Joy1Fire1 should read low on line 9");
        assert_eq!(line9 & 1, 0, "Joy1Up should read low on line 9");
        assert_ne!(
            line9 & (1 << 6),
            0,
            "Joy1Fire3 wasn't pressed, should read high"
        );
        assert_ne!(
            line9 & (1 << 7),
            0,
            "Delete wasn't pressed, should read high"
        );
    }
    /// Merged (every check kept): `keyboard_scan_reaches_the_cpu_through_ppi_and_ay_register_14`, `joystick_2_keys_reach_the_cpu_on_matrix_line_6`, `joystick_1_keys_reach_the_cpu_on_matrix_line_9`.
    #[test]
    fn keyboard_and_joystick_scans() {
        keyboard_scan_reaches_the_cpu_through_ppi_and_ay_register_14();
        joystick_2_keys_reach_the_cpu_on_matrix_line_6();
        joystick_1_keys_reach_the_cpu_on_matrix_line_9();
    }

    /// Reads keyboard matrix line `line` the way firmware does: PPI Port C
    /// read function through AY register 14, then Port A.
    fn read_matrix_line(cpc: &mut Cpc, line: u8) -> u8 {
        cpc.bus.write_io(0xF700, 0x82); // PA output, PC output
        cpc.bus.write_io(0xF400, 0x0E); // PA = register index 14 (keyboard)
        cpc.bus.write_io(0xF600, 0xC0); // Port C: select-register function
        cpc.bus.write_io(0xF700, 0x92); // reconfigure: PA now input
        cpc.bus.write_io(0xF600, 0x40 | line); // read function, `line` selected
        cpc.bus.read_io(0xF400)
    }

    fn port_watches_follow_the_cpc_loose_decode_through_a_mask() {
        use machine::{PortHit, PortWatch};
        let mut cpc = test_cpc();
        // LD BC,0x7F8D ; OUT (C),C ; LD BC,0xF400 ; IN A,(C) ; HALT
        cpc.load(
            0x4000,
            &[
                0x01, 0x8D, 0x7F, 0xED, 0x49, 0x01, 0x00, 0xF4, 0xED, 0x78, 0x76,
            ],
        );
        cpc.set_pc(0x4000);
        // Every Gate Array address (A15 = 0, A14 = 1) on writes, whatever the low byte.
        assert!(cpc.set_port_watches(vec![PortWatch {
            port: 0x4000,
            mask: 0xC000,
            on_read: false,
            on_write: true,
        }]));
        cpc.step(); // LD BC
        assert_eq!(cpc.take_port_hit(), None);
        cpc.step(); // OUT (C),C
        assert_eq!(
            cpc.take_port_hit(),
            Some(PortHit {
                port: 0x7F8D,
                value: 0x8D,
                write: true
            })
        );
        cpc.step(); // LD BC,0xF400
        cpc.step(); // IN A,(C): a read, and the PPI is not a Gate Array address anyway
        assert_eq!(cpc.take_port_hit(), None);
    }

    fn write_port_is_decoded_like_a_cpu_out() {
        let mut cpc = test_cpc_model(Model::Cpc6128);
        assert_eq!(cpc.bus.peek(0x0000), 0xAA, "lower ROM is visible at reset");

        // Gate Array function 2 with bit 2 set: lower ROM disabled.
        cpc.write_port(0x7F00, 0x80 | 0x04);
        assert_eq!(cpc.bus.peek(0x0000), 0x00, "RAM shows through");

        // The same write reaches the PPI's loosely decoded ports.
        cpc.write_port(0xF700, 0x82);
        cpc.write_port(0xF400, 0x5A);
        assert_eq!(cpc.bus.ppi.read_port_a(), 0x5A);

        // And a banked-RAM configuration change takes effect for the CPU.
        cpc.write_ram_block(5, 0, &[0xC5]);
        cpc.write_port(0x7F00, 0xC2);
        assert_eq!(cpc.bus.peek(0x4000), 0xC5);
    }
    /// Merged (every check kept): `port_watches_follow_the_cpc_loose_decode_through_a_mask`, `write_port_is_decoded_like_a_cpu_out`.
    #[test]
    fn port_watch_cases() {
        port_watches_follow_the_cpc_loose_decode_through_a_mask();
        write_port_is_decoded_like_a_cpu_out();
    }

    #[test]
    fn ram_blocks_are_physical_and_independent_of_paging() {
        let mut cpc = test_cpc_model(Model::Cpc6128);
        assert_eq!(cpc.ram_block_count(), 8);

        // Block 5 is the second 64K's second page: the CPU only sees it at
        // &4000 once RAM configuration 2 maps the whole second 64K in.
        assert_eq!(cpc.write_ram_block(5, 0x0010, &[1, 2, 3]), 3);
        assert_eq!(cpc.read_ram_block(5, 0x0010, 3), Some(vec![1, 2, 3]));
        assert_eq!(cpc.bus.peek(0x4010), 0, "not mapped at &4010 by default");
        cpc.bus.write_io(0x7F00, 0xC2); // RAM configuration 2: blocks 4-7
        assert_eq!(
            cpc.bus.peek(0x4010),
            1,
            "the &4000 window now shows block 5"
        );

        // Writes truncate at the block end; offsets/blocks out of range write nothing.
        assert_eq!(cpc.write_ram_block(7, 0x3FFE, &[9, 9, 9, 9]), 2);
        assert_eq!(cpc.read_ram_block(7, 0x3FFE, 100), Some(vec![9, 9]));
        assert_eq!(cpc.write_ram_block(8, 0, &[1]), 0, "no expansion fitted");
        assert_eq!(
            cpc.write_ram_block(0, 0x4000, &[1]),
            0,
            "offset past the block"
        );
        assert_eq!(cpc.read_ram_block(8, 0, 1), None);

        // An expansion bank adds four more blocks.
        cpc.set_ram_expansion_banks(2).unwrap();
        assert_eq!(cpc.ram_block_count(), 16);
        assert_eq!(cpc.write_ram_block(15, 0, &[0xAB]), 1);
        assert_eq!(cpc.read_ram_block(15, 0, 1), Some(vec![0xAB]));
    }

    fn amx_mouse_motion_and_buttons_reach_the_cpu_on_matrix_line_9() {
        let mut cpc = test_cpc();
        assert!(!cpc.amx_mouse_enabled(), "off by default");

        // Disabled: host mouse input does nothing.
        cpc.mouse_motion(3, 0);
        cpc.mouse_button(crate::MouseButton::Left, true);
        assert_eq!(read_matrix_line(&mut cpc, 9), 0xFF);

        cpc.set_amx_mouse(true);
        cpc.mouse_motion(1, 0); // one mickey right
        cpc.mouse_button(crate::MouseButton::Left, true);
        let line9 = read_matrix_line(&mut cpc, 9);
        assert_eq!(line9 & (1 << 3), 0, "right pulse pulls bit 3 low");
        assert_eq!(line9 & (1 << 4), 0, "left button pulls Fire2 (bit 4) low");
        assert_ne!(line9 & (1 << 2), 0, "no left motion");

        // The mouse shares the port with joystick 1: both sources AND together.
        cpc.press_key(crate::keyboard::CpcKey::Joy1Up);
        let line9 = read_matrix_line(&mut cpc, 9);
        assert_eq!(line9 & 1, 0, "joystick Up still reads");
        assert_eq!(line9 & (1 << 3), 0, "mouse Right still reads");

        // Other matrix lines are untouched.
        assert_eq!(read_matrix_line(&mut cpc, 8), 0xFF);
        assert_eq!(cpc.keyboard_row_state(9), line9, "diagnostic view agrees");

        cpc.set_amx_mouse(false);
        cpc.release_key(crate::keyboard::CpcKey::Joy1Up);
        assert_eq!(read_matrix_line(&mut cpc, 9), 0xFF, "unplugging clears it");
    }

    fn amx_mouse_pulses_advance_with_executed_cycles() {
        let mut cpc = test_cpc();
        cpc.set_amx_mouse(true);
        cpc.mouse_motion(0, -2); // two mickeys up
        assert_eq!(read_matrix_line(&mut cpc, 9) & 1, 0, "first pulse is low");

        let mut spent = 0u64;
        let mut falling_edges = 1; // the pulse already in flight
        let mut was_low = true;
        let budget = u64::from(crate::amx_mouse::PULSE_PERIOD_TSTATES) * 3;
        while spent < budget {
            spent += u64::from(cpc.step());
            let low = cpc.keyboard_row_state(9) & 1 == 0;
            if low && !was_low {
                falling_edges += 1;
            }
            was_low = low;
        }
        assert_eq!(falling_edges, 2, "two mickeys are exactly two pulses");
        assert_eq!(cpc.keyboard_row_state(9), 0xFF, "then the line idles high");
    }
    /// Merged (every check kept): `amx_mouse_motion_and_buttons_reach_the_cpu_on_matrix_line_9`, `amx_mouse_pulses_advance_with_executed_cycles`.
    #[test]
    fn amx_mouse_scans() {
        amx_mouse_motion_and_buttons_reach_the_cpu_on_matrix_line_9();
        amx_mouse_pulses_advance_with_executed_cycles();
    }

    fn gx4000_boots_from_cartridge_bank_zero_by_default() {
        let cpc = test_cpc_cartridge(Model::Gx4000, 3);
        assert_eq!(cpc.bus.peek(0x0000), 0, "bank 0 at &0000");
        assert_eq!(cpc.pc(), 0x0000, "real Z80 reset always starts at PC=0");
    }

    fn gx4000_upper_rom_select_picks_a_cartridge_bank_falling_back_to_zero() {
        let mut cpc = test_cpc_cartridge(Model::Gx4000, 3);
        cpc.bus.write_io(0xDF00, 2);
        assert_eq!(cpc.bus.peek(0xC000), 2, "slot 2 -> bank 2");
        cpc.bus.write_io(0xDF00, 7); // no bank 7 in a 3-bank cartridge
        assert_eq!(
            cpc.bus.peek(0xC000),
            0,
            "unpopulated slot falls back to bank 0"
        );
    }
    /// Merged (every check kept): `gx4000_boots_from_cartridge_bank_zero_by_default`, `gx4000_upper_rom_select_picks_a_cartridge_bank_falling_back_to_zero`.
    #[test]
    fn gx4000_cartridge_banks() {
        gx4000_boots_from_cartridge_bank_zero_by_default();
        gx4000_upper_rom_select_picks_a_cartridge_bank_falling_back_to_zero();
    }

    fn unlock_asic(cpc: &mut Cpc) {
        for byte in [
            0xffu8, 0x77, 0xb3, 0x51, 0xa8, 0xd4, 0x62, 0x39, 0x9c, 0x46, 0x2b, 0x15, 0x8a, 0xcd,
            0xee,
        ] {
            cpc.bus.write_io(0xBC00, byte);
        }
    }

    /// Unlocks the ASIC and maps its register window at `&4000-&7FFF`.
    fn open_plus_window(cpc: &mut Cpc) {
        unlock_asic(cpc);
        cpc.bus.write_io(0x7F00, 0xB8);
    }

    fn plus_with_firmware_layout() -> Cpc {
        let mut cpc = test_cpc_model(Model::Cpc6128Plus);
        program_firmware_layout(&mut cpc);
        open_plus_window(&mut cpc);
        cpc.bus.write_io(0x7F00, 0x80 | 2); // Mode 2, lower ROM stays enabled
        cpc.flush_out_of_step_video();
        cpc
    }

    /// A Plus screen colour entry: `pen` 0-16 (16 = border).
    fn set_plus_ink(cpc: &mut Cpc, pen: u16, red: u8, green: u8, blue: u8) {
        cpc.bus.write(0x6400 + pen * 2, (red << 4) | blue);
        cpc.bus.write(0x6401 + pen * 2, green);
    }

    fn two_fields(cpc: &mut Cpc) {
        advance_characters(cpc, 64 * 312 * 2 + 64 * 2);
    }

    #[test]
    fn plus_palette_writes_land_on_the_character_clock_they_happened_on() {
        let mut cpc = plus_with_firmware_layout();
        set_plus_ink(&mut cpc, 16, 15, 0, 0);
        cpc.flush_out_of_step_video();
        let before = cpc.bus.asic.as_ref().unwrap().pen_color(16);
        assert_eq!(before, (255, 0, 0));

        advance_characters(&mut cpc, 64 * 281 + 8);
        // Green half of the border entry, written 9 T-states into the span.
        cpc.bus.instruction_advanced_tstates = 9;
        cpc.bus.write(0x6421, 0x0F);
        cpc.advance_video(16);
        advance_characters(&mut cpc, 64 * (553 - 281) - 8);

        let rendered = cpc.render_frame().unwrap();
        // Window line 13 (top border: 41 lines after VSYNC begins, the window
        // opens at line 28), change on line character 10 = window character 14.
        assert_eq!(pixel_of(&rendered, 14 * 16 - 1, 13), (255, 0, 0));
        assert_eq!(pixel_of(&rendered, 14 * 16, 13), (255, 255, 0));
        assert_eq!(pixel_of(&rendered, 0, 14), (255, 255, 0));
    }

    #[test]
    fn a_plus_screen_split_switches_the_displayed_memory_at_its_line() {
        let mut cpc = plus_with_firmware_layout();
        set_plus_ink(&mut cpc, 0, 0, 0, 15); // pen 0: blue
        set_plus_ink(&mut cpc, 1, 15, 0, 0); // pen 1: red
        set_plus_ink(&mut cpc, 16, 0, 15, 0);
        cpc.flush_out_of_step_video();
        // The normal screen (&C000) is all pen 1; the split screen memory
        // (MA &2000 = &8000) all pen 0.
        cpc.bus.ram_mut()[0xC000..].fill(0xFF);
        cpc.bus.ram_mut()[0x8000..0xC000].fill(0x00);
        cpc.bus.write(0x6801, 100); // split at display line 100
        cpc.bus.write(0x6802, 0x20); // split address MA &2000 ...
        cpc.bus.write(0x6803, 0x00);
        two_fields(&mut cpc);

        let rendered = cpc.render_frame().unwrap();
        // Display line d is window line 44 + d.
        assert_eq!(pixel_of(&rendered, 64 + 40, 44 + 99), (255, 0, 0));
        assert_eq!(pixel_of(&rendered, 64 + 40, 44 + 100), (0, 0, 255));
        assert_eq!(pixel_of(&rendered, 64 + 40, 44 + 199), (0, 0, 255));
    }

    fn plus_vertical_soft_scroll_shifts_the_raster_lines_fetched() {
        let mut cpc = plus_with_firmware_layout();
        set_plus_ink(&mut cpc, 0, 0, 0, 15); // pen 0: blue
        set_plus_ink(&mut cpc, 1, 15, 0, 0); // pen 1: red
        cpc.flush_out_of_step_video();
        cpc.bus.ram_mut()[0xC000..].fill(0x00);
        // Row 0, raster 3: the first pixel is set. Row 1, raster 0 (80
        // bytes on): the first pixel is set too.
        cpc.bus.ram_mut()[0xC000 + 3 * 0x800] = 0x80;
        cpc.bus.ram_mut()[0xC000 + 80] = 0x80;
        cpc.bus.write(0x6804, 0x30); // vertical scroll 3
        two_fields(&mut cpc);

        let rendered = cpc.render_frame().unwrap();
        let (x, y) = (64, 44);
        // Display line 0 (raster 0) now fetches raster 3 ...
        assert_eq!(pixel_of(&rendered, x, y), (255, 0, 0));
        assert_eq!(pixel_of(&rendered, x, y + 1), (0, 0, 255));
        // ... and display line 5 (raster 5 + 3 = 8) spills into the next
        // row's raster 0.
        assert_eq!(pixel_of(&rendered, x, y + 5), (255, 0, 0));
        assert_eq!(pixel_of(&rendered, x, y + 4), (0, 0, 255));
    }

    fn plus_horizontal_soft_scroll_slides_the_picture_by_the_programmed_dots() {
        let mut cpc = plus_with_firmware_layout();
        set_plus_ink(&mut cpc, 0, 0, 0, 15);
        set_plus_ink(&mut cpc, 1, 15, 0, 0);
        set_plus_ink(&mut cpc, 16, 0, 15, 0);
        cpc.flush_out_of_step_video();
        cpc.bus.ram_mut()[0xC000..].fill(0xFF);
        cpc.bus.write(0x6804, 0x05); // horizontal scroll 5
        two_fields(&mut cpc);

        let rendered = cpc.render_frame().unwrap();
        for dx in 0..5 {
            assert_eq!(
                pixel_of(&rendered, 64 + dx, 44 + 10),
                (0, 255, 0),
                "dot {dx}"
            );
        }
        assert_eq!(pixel_of(&rendered, 64 + 5, 44 + 10), (255, 0, 0));
    }
    /// Merged (every check kept): `plus_vertical_soft_scroll_shifts_the_raster_lines_fetched`, `plus_horizontal_soft_scroll_slides_the_picture_by_the_programmed_dots`.
    #[test]
    fn plus_soft_scroll() {
        plus_vertical_soft_scroll_shifts_the_raster_lines_fetched();
        plus_horizontal_soft_scroll_slides_the_picture_by_the_programmed_dots();
    }

    #[test]
    fn plus_sprites_are_positioned_relative_to_the_display_area() {
        let mut cpc = plus_with_firmware_layout();
        set_plus_ink(&mut cpc, 0, 0, 0, 0);
        cpc.flush_out_of_step_video();
        // Sprite 0 at (16, 8), 1x magnification, all pixels colour 1 (yellow).
        cpc.bus.write(0x6000, 16);
        cpc.bus.write(0x6002, 8);
        cpc.bus.write(0x6004, 0b0101);
        for i in 0..256u16 {
            cpc.bus.write(0x4000 + i, 1);
        }
        cpc.bus.write(0x6422, 0xF0);
        cpc.bus.write(0x6423, 0x0F);
        two_fields(&mut cpc);

        let rendered = cpc.render_frame().unwrap();
        // Display origin is window (64, 44).
        assert_eq!(pixel_of(&rendered, 64 + 16, 44 + 8), (255, 255, 0));
        assert_eq!(pixel_of(&rendered, 64 + 31, 44 + 23), (255, 255, 0));
        assert_eq!(pixel_of(&rendered, 64 + 15, 44 + 8), (0, 0, 0));
        assert_eq!(pixel_of(&rendered, 64 + 32, 44 + 23), (0, 0, 0));
    }

    fn plus_asic_unlocks_via_crtc_select_register_writes() {
        let mut cpc = test_cpc_model(Model::Cpc6128Plus);
        assert_eq!(
            cpc.bus.interrupt_vector_byte(),
            0x00,
            "locked CPC+ reports the real 0x00 vector byte, not classic 0xFF"
        );
        unlock_asic(&mut cpc);
        assert_eq!(
            cpc.bus.interrupt_vector_byte(),
            0x06,
            "unlocked and idle: raster cause with default zero vector register"
        );
    }

    fn plus_asic_register_window_reads_and_writes_once_unlocked_and_relocated() {
        let mut cpc = test_cpc_model(Model::Cpc6128Plus);
        unlock_asic(&mut cpc);
        // Gate Array function 2, bit5 set, bits4:3=0b11 (0x18): ASIC
        // registers visible at &4000 (see `asic::Asic::maybe_write_rmr2`).
        cpc.bus.write_io(0x7F00, 0xB8); // 1011_1000: fn=10, b5=1, b4b3=11
        assert_eq!(cpc.bus.asic.as_ref().unwrap().rmr2(), 0xB8);
        cpc.bus.write(0x6800, 42); // PRI register
        assert_eq!(cpc.bus.peek(0x6800), 42);
        assert_eq!(cpc.bus.asic.as_ref().unwrap().pri(), 42);
    }
    /// Merged (every check kept): `plus_asic_unlocks_via_crtc_select_register_writes`, `plus_asic_register_window_reads_and_writes_once_unlocked_and_relocated`.
    #[test]
    fn plus_asic_unlock_and_window() {
        plus_asic_unlocks_via_crtc_select_register_writes();
        plus_asic_register_window_reads_and_writes_once_unlocked_and_relocated();
    }

    #[test]
    fn gx4000_rmr2_relocates_a_cartridge_bank_to_0x4000() {
        let mut cpc = test_cpc_cartridge(Model::Gx4000, 3);
        unlock_asic(&mut cpc);
        // fn=10, b5=1 (targets RMR2), b4b3=01 (relocate to &4000),
        // b2b0=2 (bank 2): 0x80 | 0x20 | 0x08 | 0x02 = 0xAA.
        cpc.bus.write_io(0x7F00, 0xAA);
        assert_eq!(
            cpc.bus.peek(0x4000),
            2,
            "cartridge bank 2 now visible at &4000"
        );
        assert_eq!(
            cpc.bus.peek(0x0000),
            0,
            "bank 0 stays at &0000, unaffected by the &4000 relocation"
        );
    }

    #[test]
    fn analog_axis_reaches_the_asic() {
        let mut cpc = test_cpc_model(Model::Cpc6128Plus);
        cpc.set_analog_axis(AnalogJoystick::First, AnalogAxis::X, ANALOG_MAX);
        // ADC0 lives at real address &6808 -- 0x2808 relative to &4000.
        assert_eq!(
            cpc.bus.asic.as_ref().unwrap().read_window(0x2808),
            ANALOG_MAX
        );
    }

    #[test]
    fn pri_fires_a_real_cpu_interrupt_at_the_programmed_raster_line() {
        let mut cpc = test_cpc_model(Model::Cpc6128Plus);
        unlock_asic(&mut cpc);
        // PRI: line_counter=0, raster_counter=1 -- reached on the very
        // first scanline advance.
        cpc.bus.asic.as_mut().unwrap().write_high(0x0800, 1);
        cpc.bus.write(0x0300, 0x00); // NOP
        cpc.set_pc(0x0300);
        cpc.cpu.iff1 = true;
        cpc.cpu.im = 1;
        for (register, value) in [(0, 3), (2, 1), (3, 1), (4, 127), (7, 127), (9, 31)] {
            cpc.bus.crtc.select_register(register);
            cpc.bus.crtc.write_register(value);
        }
        let mut fired = false;
        for _ in 0..100 {
            cpc.step();
            if cpc.pc() == 0x0038 {
                fired = true;
                break;
            }
        }
        assert!(fired, "PRI should have fired a real Z80 interrupt");
    }

    #[test]
    fn dma_load_command_reaches_the_ay_chip_through_step() {
        let mut cpc = test_cpc_model(Model::Cpc6128Plus);
        unlock_asic(&mut cpc);
        // DMA channel 0's command list in primary RAM at &8000:
        // LOAD register 0, data 0x10; then STOP.
        cpc.bus.write(0x8000, 0x10); // LOAD: data low byte
        cpc.bus.write(0x8001, 0x00); // LOAD: register 0 (high nibble 0)
        cpc.bus.write(0x8002, 0x20); // STOP bit
        cpc.bus.write(0x8003, 0x40); // control-function opcode
        {
            let asic = cpc.bus.asic.as_mut().unwrap();
            asic.write_high(0x0C00, 0x00); // DMA0 addr lo
            asic.write_high(0x0C01, 0x80); // DMA0 addr hi -> &8000
            asic.write_high(0x0C0F, 0x01); // enable channel 0
        }
        cpc.bus.write(0x0300, 0x00); // NOP
        cpc.set_pc(0x0300);
        for (register, value) in [(0, 3), (2, 1), (3, 1), (4, 127), (7, 127), (9, 31)] {
            cpc.bus.crtc.select_register(register);
            cpc.bus.crtc.write_register(value);
        }
        for _ in 0..20 {
            cpc.step();
        }
        cpc.bus.ay.select_register(0);
        assert_eq!(
            cpc.bus.ay.read(),
            0x10,
            "DMA LOAD should have written AY register 0 through a real step() cycle"
        );
    }

    fn render_frame_of_a_plus_model_uses_the_same_monitor_window() {
        let cpc = test_cpc_model(Model::Cpc464Plus);
        let (w, h, _) = cpc.render_frame().unwrap();
        assert_eq!((w, h), (768, 576));
    }
    /// Merged (every check kept): `render_frame_is_the_fixed_monitor_window_in_every_gate_array_mode`, `render_frame_doubles_every_scanline_into_a_4_by_3_picture`, `render_frame_of_a_plus_model_uses_the_same_monitor_window`.
    #[test]
    fn render_frame_window() {
        render_frame_is_the_fixed_monitor_window_in_every_gate_array_mode();
        render_frame_doubles_every_scanline_into_a_4_by_3_picture();
        render_frame_of_a_plus_model_uses_the_same_monitor_window();
    }

    fn cpc_sna_snapshot_round_trips_cpu_ram_and_hardware_registers() {
        let mut source = test_cpc_model(Model::Cpc6128);
        source.bus.gate_array.write(0x8D);
        source.bus.write(0x4000, 0x5A);
        source.bus.gate_array.write(0x40 | 7);
        source.bus.gate_array.write(0x40 | 0x1D);
        source.set_pc(0x4567);
        source.cpu.af = 0x1234;
        let bytes = cpc_snapshot::write(&source.save_snapshot()).unwrap();
        let snapshot = cpc_snapshot::parse(&bytes).unwrap();

        let mut restored = test_cpc_model(Model::Cpc6128);
        restored.load_snapshot(snapshot).unwrap();
        assert_eq!(restored.pc(), 0x4567);
        assert_eq!(restored.cpu.af, 0x1234);
        assert_eq!(restored.read_memory(0x4000), 0x5A);
        assert_eq!(restored.bus.ram_configuration(), 0);
        assert_eq!(restored.bus.upper_rom_select(), 0);
    }

    fn cpc_sna_snapshot_rejects_wrong_machine_ram_shape() {
        let snapshot = test_cpc_model(Model::Cpc6128).save_snapshot();
        let mut target = test_cpc_model(Model::Cpc464);
        let error = target.load_snapshot(snapshot).unwrap_err();
        assert!(error.contains("RAM size mismatch"));
    }

    fn every_model_starts_with_its_default_crtc_type() {
        for (model, expected) in [
            (Model::Cpc464, crate::CrtcType::Type0),
            (Model::Cpc664, crate::CrtcType::Type0),
            (Model::Cpc6128, crate::CrtcType::Type1),
        ] {
            assert_eq!(test_cpc_model(model).crtc_type(), expected, "{model:?}");
        }
        for model in [Model::Cpc464Plus, Model::Cpc6128Plus] {
            assert_eq!(test_cpc_model(model).crtc_type(), crate::CrtcType::Type3);
        }
        assert_eq!(
            test_cpc_cartridge(Model::Gx4000, 1).crtc_type(),
            crate::CrtcType::Type3
        );
    }

    fn crtc_status_and_register_read_ports_are_decoded_separately() {
        let mut cpc = test_cpc_model(Model::Cpc464);
        let program = |cpc: &mut Cpc, register: u8, value: u8| {
            cpc.bus.write_io(0xBC00, register); // select
            cpc.bus.write_io(0xBD00, value); // write
        };
        program(&mut cpc, 12, 0x2B);
        program(&mut cpc, 14, 0x15);

        // Type 0: R12/R14 read back through &BFxx, &BExx floats high.
        cpc.bus.write_io(0xBC00, 12);
        assert_eq!(cpc.bus.read_io(0xBF00), 0x2B);
        cpc.bus.write_io(0xBC00, 14);
        assert_eq!(cpc.bus.read_io(0xBF00), 0x15);
        assert_eq!(cpc.bus.read_io(0xBE00), 0xFF);

        // Type 1: R12 no longer readable, R14 still is, and &BExx is a
        // real status register (nothing but the border-R6 latch to set).
        cpc.set_crtc_type(crate::CrtcType::Type1);
        cpc.bus.write_io(0xBC00, 12);
        assert_eq!(cpc.bus.read_io(0xBF00), 0x00);
        cpc.bus.write_io(0xBC00, 14);
        assert_eq!(cpc.bus.read_io(0xBF00), 0x15);
        assert_eq!(cpc.bus.read_io(0xBE00) & 0x9F, 0x00);
    }
    /// Merged (every check kept): `every_model_starts_with_its_default_crtc_type`, `crtc_status_and_register_read_ports_are_decoded_separately`.
    #[test]
    fn crtc_type_and_read_ports() {
        every_model_starts_with_its_default_crtc_type();
        crtc_status_and_register_read_ports_are_decoded_separately();
    }

    fn sna_snapshot_round_trips_the_crtc_type() {
        let mut source = test_cpc_model(Model::Cpc6128);
        source.set_crtc_type(crate::CrtcType::Type2);
        let bytes = cpc_snapshot::write(&source.save_snapshot()).unwrap();
        let snapshot = cpc_snapshot::parse(&bytes).unwrap();
        assert_eq!(snapshot.version3.as_ref().unwrap().crtc_type, 2);

        let mut restored = test_cpc_model(Model::Cpc6128);
        assert_eq!(restored.crtc_type(), crate::CrtcType::Type1);
        restored.load_snapshot(snapshot).unwrap();
        assert_eq!(restored.crtc_type(), crate::CrtcType::Type2);
    }

    fn a_snapshot_cannot_swap_a_plus_machine_onto_a_discrete_6845() {
        let mut classic = test_cpc_model(Model::Cpc6128);
        classic.set_crtc_type(crate::CrtcType::Type0);
        let mut snapshot = classic.save_snapshot();
        snapshot.cpc_model = Some(3); // "CPC Plus" hardware marker
        let mut plus = test_cpc_model(Model::Cpc6128Plus);
        plus.load_snapshot(snapshot).unwrap();
        assert_eq!(plus.crtc_type(), crate::CrtcType::Type3);
    }
    /// Merged (every check kept): `cpc_sna_snapshot_round_trips_cpu_ram_and_hardware_registers`, `cpc_sna_snapshot_rejects_wrong_machine_ram_shape`, `sna_snapshot_round_trips_the_crtc_type`, `a_snapshot_cannot_swap_a_plus_machine_onto_a_discrete_6845`.
    #[test]
    fn sna_snapshot_cases() {
        cpc_sna_snapshot_round_trips_cpu_ram_and_hardware_registers();
        cpc_sna_snapshot_rejects_wrong_machine_ram_shape();
        sna_snapshot_round_trips_the_crtc_type();
        a_snapshot_cannot_swap_a_plus_machine_onto_a_discrete_6845();
    }

    #[test]
    fn voc_tape_playback_reflects_in_port_b_ear_bit() {
        // Build minimal VOC file with a 0x01 Sound Data block containing alternating samples
        let mut voc = Vec::new();
        // Header
        voc.extend_from_slice(b"Creative Voice File\x1A");
        voc.extend_from_slice(&26u16.to_le_bytes());
        voc.extend_from_slice(&0x0100u16.to_le_bytes());
        voc.extend_from_slice(&0x1134u16.to_le_bytes());
        // Block 0x01: Sound Data with 4 samples: low, high, low, high
        voc.push(0x01);
        voc.push(231); // freq_divisor: 40kHz
        voc.push(0x00); // codec: 8-bit PCM
        voc.extend_from_slice(&4u32.to_le_bytes()[0..3]); // 4 bytes
        voc.extend_from_slice(&[0x00, 0xFF, 0x00, 0xFF]); // alternating samples
        // Terminator
        voc.push(0x00);

        // Parse VOC tape
        let tape = zx_tape::parse_voc(&voc).expect("parse_voc failed");

        let mut cpc = test_cpc();
        cpc.set_tape_leader_ms(0); // these tests drive the tape from its first pulse
        cpc.load_tape(tape);

        // Enable PPI output mode for Port C (cassette motor control)
        cpc.bus.write_io(0xF700, 0x82); // Mode set: PA/PC output
        // Turn cassette motor on (Port C bit 4 = motor)
        cpc.bus.write_io(0xF700, 0x09); // PPI control byte: set bit 3 (cassette motor)

        // Step the machine a few times and check EAR bit (Port B bit 7)
        // The first pulse is only ~88 tape-domain T-states (~99 CPC
        // T-states) wide -- tick just enough to land inside it, not
        // clear through the whole tiny 4-pulse tape and out the far end
        // (which would read back idle-high, not the tape's own level).
        cpc.bus.tick_tape(10);

        let port_b = cpc.bus.read_io(0xF500);
        // Port B bit 7 should reflect the tape's EAR bit
        // The tape starts with low (0x00), so bit 7 should be 0 (low=0)
        assert_eq!(
            port_b & 0x80,
            0x00,
            "EAR bit (Port B bit 7) should reflect low sample from VOC tape"
        );
    }

    #[test]
    fn a_classic_ink_write_sets_the_asic_palette_of_a_plus_model() {
        // Programs (and the .cpr diagnostics) set colours with `OUT (&7Fxx)` and never touch the ASIC registers; a
        // Plus must show them rather than black.
        for model in [Model::Gx4000, Model::Cpc6128Plus] {
            let mut cpc = if model.has_internal_rom() {
                test_cpc_model(model)
            } else {
                test_cpc_cartridge(model, 1)
            };
            cpc.bus.write_io(0x7F00, 0x01); // select pen 1
            cpc.bus.write_io(0x7F00, 0x40 | 4); // hardware colour 4: blue (level 1 of B)
            assert_eq!(
                cpc.bus.video_state().inks[1],
                [0x00, 0x00, 0x88],
                "{model:?}"
            );
            cpc.bus.write_io(0x7F00, 0x10); // the border
            cpc.bus.write_io(0x7F00, 0x40 | 11); // bright white
            assert_eq!(
                cpc.bus.video_state().inks[16],
                [0xFF, 0xFF, 0xFF],
                "{model:?}"
            );
        }
    }
}
