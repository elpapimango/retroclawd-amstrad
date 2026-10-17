//! Parser and writer for Amstrad CPC `MV - SNA` snapshots.
//!
//! Layout follows Caprice32's packed `t_SNA_header` and the CPCWiki SNA
//! format: a fixed 256-byte header followed by a raw RAM dump whose size is
//! stored in 1-KiB units. Versions 1, 2, and 3 are accepted. Version 2 adds
//! machine identity; version 3 adds FDC, printer, AY-envelope, CRTC timing,
//! Gate Array timing, and pending-interrupt state.
//!
//! Sources: Caprice32 `src/cap32.h` and `src/slotshandler.cpp`, plus CPCWiki's
//! `Format:SNA_snapshot_file_format` page. This crate stays format-only; a
//! machine crate owns applying these fields to live hardware.

use z80::Cpu;

pub const HEADER_SIZE: usize = 256;
const MAGIC: &[u8; 8] = b"MV - SNA";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateArrayState {
    pub selected_pen: u8,
    pub pen_colors: [u8; 17],
    pub rom_config: u8,
    pub ram_config: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrtcState {
    pub selected_register: u8,
    pub registers: [u8; 18],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpiState {
    pub port_a: u8,
    pub port_b: u8,
    pub port_c: u8,
    pub control: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AyState {
    pub selected_register: u8,
    pub registers: [u8; 16],
}

/// Version-3 fields. Filename fields are retained even though retroclawd does
/// not attach media from a snapshot; preserving them keeps parse/write lossless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version3State {
    pub drive_a_filename: [u8; 13],
    pub drive_b_filename: [u8; 13],
    pub cartridge_filename: [u8; 13],
    pub fdc_motor: u8,
    pub drive_a_track: u8,
    pub drive_b_track: u8,
    pub drive_c_track: u8,
    pub drive_d_track: u8,
    pub printer_data: u8,
    pub psg_envelope_step: u8,
    pub psg_envelope_direction: u8,
    pub crtc_type: u8,
    pub crtc_address: u16,
    pub crtc_scanline: u16,
    pub crtc_char_count: u16,
    pub crtc_line_count: u8,
    pub crtc_raster_count: u8,
    pub crtc_vt_adjust_count: u8,
    pub crtc_hsync_count: u8,
    pub crtc_vsync_count: u8,
    pub crtc_flags: u16,
    pub gate_array_interrupt_delay: u8,
    pub gate_array_scanline_count: u8,
    pub z80_interrupt_pending: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub version: u8,
    pub cpu: Cpu,
    pub gate_array: GateArrayState,
    pub crtc: CrtcState,
    pub upper_rom: u8,
    pub ppi: PpiState,
    pub ay: AyState,
    pub ram: Vec<u8>,
    /// CPC model id used by Caprice32: 0=464, 1=664, 2=6128, 3=Plus.
    pub cpc_model: Option<u8>,
    pub last_interrupt: Option<u8>,
    pub screen_modes: [u8; 6],
    pub version3: Option<Version3State>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TooShort,
    InvalidMagic,
    UnsupportedVersion(u8),
    ZeroRam,
    InvalidRamSize(u16),
    TruncatedRam { expected: usize, actual: usize },
    TrailingBytes { expected: usize, actual: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "CPC SNA file is shorter than its 256-byte header"),
            Self::InvalidMagic => write!(f, "not a CPC SNA file (missing MV - SNA header)"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported CPC SNA version {v}"),
            Self::ZeroRam => write!(f, "CPC SNA contains a zero-size RAM dump"),
            Self::InvalidRamSize(kib) => write!(f, "invalid CPC SNA RAM size {kib} KiB"),
            Self::TruncatedRam { expected, actual } => {
                write!(
                    f,
                    "CPC SNA RAM dump truncated: expected {expected} bytes, got {actual}"
                )
            }
            Self::TrailingBytes { expected, actual } => {
                write!(
                    f,
                    "CPC SNA has trailing bytes: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    UnsupportedVersion(u8),
    ZeroRam,
    RamNotKiBAligned,
    RamTooLarge,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported CPC SNA version {v}"),
            Self::ZeroRam => write!(f, "cannot write a CPC SNA with zero RAM"),
            Self::RamNotKiBAligned => write!(f, "CPC SNA RAM must be aligned to 1 KiB"),
            Self::RamTooLarge => write!(f, "CPC SNA RAM exceeds its 65535 KiB header limit"),
        }
    }
}

impl std::error::Error for WriteError {}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn array<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    bytes[offset..offset + N]
        .try_into()
        .expect("fixed SNA field")
}

/// Parse one complete CPC SNA file.
pub fn parse(bytes: &[u8]) -> Result<Snapshot, Error> {
    if bytes.len() < HEADER_SIZE {
        return Err(Error::TooShort);
    }
    if &bytes[..8] != MAGIC {
        return Err(Error::InvalidMagic);
    }
    let version = bytes[16];
    if !(1..=3).contains(&version) {
        return Err(Error::UnsupportedVersion(version));
    }
    let ram_kib = u16_at(bytes, 107);
    if ram_kib == 0 {
        return Err(Error::ZeroRam);
    }
    if !ram_kib.is_multiple_of(64) {
        return Err(Error::InvalidRamSize(ram_kib));
    }
    let ram_len = usize::from(ram_kib) * 1024;
    let expected = HEADER_SIZE + ram_len;
    if bytes.len() < expected {
        return Err(Error::TruncatedRam {
            expected: ram_len,
            actual: bytes.len().saturating_sub(HEADER_SIZE),
        });
    }
    if bytes.len() > expected {
        return Err(Error::TrailingBytes {
            expected,
            actual: bytes.len(),
        });
    }

    let mut cpu = Cpu::new();
    cpu.af = u16_at(bytes, 17);
    cpu.bc = u16_at(bytes, 19);
    cpu.de = u16_at(bytes, 21);
    cpu.hl = u16_at(bytes, 23);
    cpu.r = bytes[25];
    cpu.i = bytes[26];
    cpu.iff1 = bytes[27] != 0;
    cpu.iff2 = bytes[28] != 0;
    cpu.ix = u16_at(bytes, 29);
    cpu.iy = u16_at(bytes, 31);
    cpu.sp = u16_at(bytes, 33);
    cpu.pc = u16_at(bytes, 35);
    cpu.im = bytes[37];
    cpu.af_ = u16_at(bytes, 38);
    cpu.bc_ = u16_at(bytes, 40);
    cpu.de_ = u16_at(bytes, 42);
    cpu.hl_ = u16_at(bytes, 44);

    let version3 = (version >= 3).then(|| Version3State {
        drive_a_filename: array(bytes, 117),
        drive_b_filename: array(bytes, 130),
        cartridge_filename: array(bytes, 143),
        fdc_motor: bytes[156],
        drive_a_track: bytes[157],
        drive_b_track: bytes[158],
        drive_c_track: bytes[159],
        drive_d_track: bytes[160],
        printer_data: bytes[161],
        psg_envelope_step: bytes[162],
        psg_envelope_direction: bytes[163],
        crtc_type: bytes[164],
        crtc_address: u16_at(bytes, 165),
        crtc_scanline: u16_at(bytes, 167),
        crtc_char_count: u16_at(bytes, 169),
        crtc_line_count: bytes[171],
        crtc_raster_count: bytes[172],
        crtc_vt_adjust_count: bytes[173],
        crtc_hsync_count: bytes[174],
        crtc_vsync_count: bytes[175],
        crtc_flags: u16_at(bytes, 176),
        gate_array_interrupt_delay: bytes[178],
        gate_array_scanline_count: bytes[179],
        z80_interrupt_pending: bytes[180],
    });

    Ok(Snapshot {
        version,
        cpu,
        gate_array: GateArrayState {
            selected_pen: bytes[46],
            pen_colors: array(bytes, 47),
            rom_config: bytes[64],
            ram_config: bytes[65],
        },
        crtc: CrtcState {
            selected_register: bytes[66],
            registers: array(bytes, 67),
        },
        upper_rom: bytes[85],
        ppi: PpiState {
            port_a: bytes[86],
            port_b: bytes[87],
            port_c: bytes[88],
            control: bytes[89],
        },
        ay: AyState {
            selected_register: bytes[90],
            registers: array(bytes, 91),
        },
        ram: bytes[HEADER_SIZE..].to_vec(),
        cpc_model: (version >= 2).then_some(bytes[109]),
        last_interrupt: (version >= 2).then_some(bytes[110]),
        screen_modes: array(bytes, 111),
        version3,
    })
}

/// Write a CPC SNA file. Fields unavailable in a selected version remain zero.
pub fn write(snapshot: &Snapshot) -> Result<Vec<u8>, WriteError> {
    if !(1..=3).contains(&snapshot.version) {
        return Err(WriteError::UnsupportedVersion(snapshot.version));
    }
    if snapshot.ram.is_empty() {
        return Err(WriteError::ZeroRam);
    }
    if !snapshot.ram.len().is_multiple_of(1024) {
        return Err(WriteError::RamNotKiBAligned);
    }
    let ram_kib = snapshot.ram.len() / 1024;
    if ram_kib > usize::from(u16::MAX) {
        return Err(WriteError::RamTooLarge);
    }

    let mut header = [0u8; HEADER_SIZE];
    header[..8].copy_from_slice(MAGIC);
    header[16] = snapshot.version;
    put_u16(&mut header, 17, snapshot.cpu.af);
    put_u16(&mut header, 19, snapshot.cpu.bc);
    put_u16(&mut header, 21, snapshot.cpu.de);
    put_u16(&mut header, 23, snapshot.cpu.hl);
    header[25] = snapshot.cpu.r;
    header[26] = snapshot.cpu.i;
    header[27] = u8::from(snapshot.cpu.iff1);
    header[28] = u8::from(snapshot.cpu.iff2);
    put_u16(&mut header, 29, snapshot.cpu.ix);
    put_u16(&mut header, 31, snapshot.cpu.iy);
    put_u16(&mut header, 33, snapshot.cpu.sp);
    put_u16(&mut header, 35, snapshot.cpu.pc);
    header[37] = snapshot.cpu.im;
    put_u16(&mut header, 38, snapshot.cpu.af_);
    put_u16(&mut header, 40, snapshot.cpu.bc_);
    put_u16(&mut header, 42, snapshot.cpu.de_);
    put_u16(&mut header, 44, snapshot.cpu.hl_);
    header[46] = snapshot.gate_array.selected_pen;
    header[47..64].copy_from_slice(&snapshot.gate_array.pen_colors);
    header[64] = snapshot.gate_array.rom_config;
    header[65] = snapshot.gate_array.ram_config;
    header[66] = snapshot.crtc.selected_register;
    header[67..85].copy_from_slice(&snapshot.crtc.registers);
    header[85] = snapshot.upper_rom;
    header[86] = snapshot.ppi.port_a;
    header[87] = snapshot.ppi.port_b;
    header[88] = snapshot.ppi.port_c;
    header[89] = snapshot.ppi.control;
    header[90] = snapshot.ay.selected_register;
    header[91..107].copy_from_slice(&snapshot.ay.registers);
    put_u16(&mut header, 107, ram_kib as u16);
    if snapshot.version >= 2 {
        header[109] = snapshot.cpc_model.unwrap_or(0);
        header[110] = snapshot.last_interrupt.unwrap_or(0);
        header[111..117].copy_from_slice(&snapshot.screen_modes);
    }
    if snapshot.version >= 3
        && let Some(v3) = &snapshot.version3
    {
        header[117..130].copy_from_slice(&v3.drive_a_filename);
        header[130..143].copy_from_slice(&v3.drive_b_filename);
        header[143..156].copy_from_slice(&v3.cartridge_filename);
        header[156] = v3.fdc_motor;
        header[157] = v3.drive_a_track;
        header[158] = v3.drive_b_track;
        header[159] = v3.drive_c_track;
        header[160] = v3.drive_d_track;
        header[161] = v3.printer_data;
        header[162] = v3.psg_envelope_step;
        header[163] = v3.psg_envelope_direction;
        header[164] = v3.crtc_type;
        put_u16(&mut header, 165, v3.crtc_address);
        put_u16(&mut header, 167, v3.crtc_scanline);
        put_u16(&mut header, 169, v3.crtc_char_count);
        header[171] = v3.crtc_line_count;
        header[172] = v3.crtc_raster_count;
        header[173] = v3.crtc_vt_adjust_count;
        header[174] = v3.crtc_hsync_count;
        header[175] = v3.crtc_vsync_count;
        put_u16(&mut header, 176, v3.crtc_flags);
        header[178] = v3.gate_array_interrupt_delay;
        header[179] = v3.gate_array_scanline_count;
        header[180] = v3.z80_interrupt_pending;
    }
    let mut bytes = Vec::with_capacity(HEADER_SIZE + snapshot.ram.len());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&snapshot.ram);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        let mut cpu = Cpu::new();
        cpu.af = 0x1234;
        cpu.pc = 0x4567;
        Snapshot {
            version: 3,
            cpu,
            gate_array: GateArrayState {
                selected_pen: 4,
                pen_colors: [0; 17],
                rom_config: 0x0C,
                ram_config: 3,
            },
            crtc: CrtcState {
                selected_register: 7,
                registers: [1; 18],
            },
            upper_rom: 7,
            ppi: PpiState {
                port_a: 1,
                port_b: 2,
                port_c: 3,
                control: 0x82,
            },
            ay: AyState {
                selected_register: 13,
                registers: [4; 16],
            },
            ram: vec![0xA5; 64 * 1024],
            cpc_model: Some(2),
            last_interrupt: Some(8),
            screen_modes: [1, 2, 3, 4, 5, 6],
            version3: Some(Version3State {
                drive_a_filename: [b'A'; 13],
                drive_b_filename: [b'B'; 13],
                cartridge_filename: [b'C'; 13],
                fdc_motor: 1,
                drive_a_track: 4,
                drive_b_track: 5,
                drive_c_track: 0,
                drive_d_track: 0,
                printer_data: 0x80,
                psg_envelope_step: 7,
                psg_envelope_direction: 0xff,
                crtc_type: 0,
                crtc_address: 1,
                crtc_scanline: 2,
                crtc_char_count: 3,
                crtc_line_count: 4,
                crtc_raster_count: 5,
                crtc_vt_adjust_count: 6,
                crtc_hsync_count: 7,
                crtc_vsync_count: 8,
                crtc_flags: 9,
                gate_array_interrupt_delay: 10,
                gate_array_scanline_count: 11,
                z80_interrupt_pending: 1,
            }),
        }
    }

    #[test]
    fn parses_and_writes_v3_round_trip() {
        let snapshot = sample();
        let bytes = write(&snapshot).unwrap();
        assert_eq!(bytes.len(), HEADER_SIZE + 64 * 1024);
        assert_eq!(parse(&bytes).unwrap(), snapshot);
    }

    fn accepts_v1_without_versioned_tail() {
        let mut snapshot = sample();
        snapshot.version = 1;
        snapshot.cpc_model = None;
        snapshot.last_interrupt = None;
        snapshot.version3 = None;
        let parsed = parse(&write(&snapshot).unwrap()).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.cpc_model, None);
        assert_eq!(parsed.version3, None);
    }

    fn rejects_bad_magic_and_truncation() {
        assert_eq!(parse(&[0; HEADER_SIZE]), Err(Error::InvalidMagic));
        let mut bytes = write(&sample()).unwrap();
        bytes.truncate(HEADER_SIZE + 3);
        assert!(matches!(parse(&bytes), Err(Error::TruncatedRam { .. })));
    }
    /// Merged (every check kept): `accepts_v1_without_versioned_tail`, `rejects_bad_magic_and_truncation`.
    #[test]
    fn snapshot_edge_cases() {
        accepts_v1_without_versioned_tail();
        rejects_bad_magic_and_truncation();
    }
}
