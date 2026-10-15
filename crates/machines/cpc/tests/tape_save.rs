//! End-to-end cassette-saving proof against the *real* CPC464 firmware (skipped
//! when the ROM file isn't present): the genuine firmware `SAVE`s a file
//! through the cassette output line, the recording is written out as a
//! `.cdt`/`.tzx`, and a freshly booted machine `LOAD`s that file back off the
//! emulated tape deck and must end up with the same bytes in memory.
//!
//! This is the check the unit tests can't give: they use hand-built pulse
//! sequences, this uses whatever the firmware's own tape writer really emits.

use cpc::{Cpc, Model};
use machine::Machine;

const ROM_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../roms/cpc/cpc464.rom");

fn rom() -> Option<Vec<u8>> {
    std::fs::read(ROM_PATH).ok()
}

/// Runs until `done` holds or `max_cycles` elapse; returns cycles run.
fn run_until(cpc: &mut Cpc, max_cycles: u64, mut done: impl FnMut(&Cpc) -> bool) -> u64 {
    let mut spent = 0u64;
    while spent < max_cycles {
        spent += u64::from(cpc.step());
        if spent % 4096 < 8 && done(cpc) {
            break;
        }
    }
    spent
}

/// Tiny Z80 byte assembler with `JR NC, fail` fixups, as in the disk test.
struct Asm {
    bytes: Vec<u8>,
    fails: Vec<usize>,
}

impl Asm {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            fails: Vec::new(),
        }
    }
    fn emit(&mut self, b: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(b);
        self
    }
    fn ld_hl(&mut self, v: u16) -> &mut Self {
        self.emit(&[0x21, v as u8, (v >> 8) as u8])
    }
    fn ld_de(&mut self, v: u16) -> &mut Self {
        self.emit(&[0x11, v as u8, (v >> 8) as u8])
    }
    fn ld_bc(&mut self, v: u16) -> &mut Self {
        self.emit(&[0x01, v as u8, (v >> 8) as u8])
    }
    fn call(&mut self, v: u16) -> &mut Self {
        self.emit(&[0xCD, v as u8, (v >> 8) as u8])
    }
    /// Records the stage number, then `JR NC, fail`: taken on firmware failure.
    fn check(&mut self, stage: u8) -> &mut Self {
        self.emit(&[0x3E, stage]);
        self.emit(&[0x30, 0x00]);
        self.fails.push(self.bytes.len() - 1);
        self
    }
    fn finish(mut self, status_addr: u16) -> Vec<u8> {
        self.emit(&[
            0x3E,
            0xFF,
            0x32,
            status_addr as u8,
            (status_addr >> 8) as u8,
            0x76,
        ]);
        let fail = self.bytes.len();
        self.emit(&[0x32, status_addr as u8, (status_addr >> 8) as u8, 0x76]);
        for at in std::mem::take(&mut self.fails) {
            self.bytes[at] = (fail - (at + 1)) as u8;
        }
        self.bytes
    }
}

const CAS_NOISY: u16 = 0xBC6B;
const CAS_IN_OPEN: u16 = 0xBC77;
const CAS_IN_CLOSE: u16 = 0xBC7A;
const CAS_IN_DIRECT: u16 = 0xBC83;
const CAS_OUT_OPEN: u16 = 0xBC8C;
const CAS_OUT_CLOSE: u16 = 0xBC8F;
const CAS_OUT_DIRECT: u16 = 0xBC98;

const PROGRAM: u16 = 0x4000;
const FILENAME: u16 = 0x4100;
const STATUS: u16 = 0x4200;
const IO_BUFFER: u16 = 0x6000;
const SOURCE: u16 = 0x8000;
const LOADED: u16 = 0x9000;
const LEN: u16 = 1000;

fn pattern(i: usize) -> u8 {
    ((i * 11 + 5) & 0xFF) as u8
}

/// `CAS NOISY` off (no "Press REC and PLAY" prompt to wait on), then
/// `CAS OUT OPEN`/`DIRECT`/`CLOSE`.
fn save_program() -> Vec<u8> {
    let mut a = Asm::new();
    a.emit(&[0x3E, 0xFF]).call(CAS_NOISY);
    a.ld_hl(FILENAME)
        .emit(&[0x06, 4])
        .ld_de(IO_BUFFER)
        .call(CAS_OUT_OPEN)
        .check(1);
    a.ld_hl(SOURCE)
        .ld_de(LEN)
        .ld_bc(0)
        .emit(&[0x3E, 2])
        .call(CAS_OUT_DIRECT)
        .check(2);
    a.call(CAS_OUT_CLOSE).check(3);
    a.finish(STATUS)
}

/// `CAS NOISY` off, then `CAS IN OPEN`/`DIRECT`/`CLOSE`.
fn load_program() -> Vec<u8> {
    let mut a = Asm::new();
    a.emit(&[0x3E, 0xFF]).call(CAS_NOISY);
    a.ld_hl(FILENAME)
        .emit(&[0x06, 4])
        .ld_de(IO_BUFFER)
        .call(CAS_IN_OPEN)
        .check(4);
    a.ld_hl(LOADED).call(CAS_IN_DIRECT).check(5);
    a.call(CAS_IN_CLOSE).check(6);
    a.finish(STATUS)
}

fn booted(rom: &[u8]) -> Cpc {
    let mut cpc = Cpc::new(rom.to_vec(), Model::Cpc464).unwrap();
    // Let the real firmware boot to the BASIC prompt.
    run_until(&mut cpc, 25_000_000, |_| false);
    cpc
}

/// Runs `program` for at most `max_cycles` and returns its status byte (`0` =
/// never finished, `0xFF` = every firmware call succeeded).
fn run_program_status(cpc: &mut Cpc, program: &[u8], max_cycles: u64) -> u8 {
    cpc.load(FILENAME, b"TEST");
    cpc.load(PROGRAM, program);
    cpc.load(STATUS, &[0]);
    cpc.set_pc(PROGRAM);
    run_until(cpc, max_cycles, |c| c.read_memory(STATUS) != 0);
    cpc.read_memory(STATUS)
}

fn run_program(cpc: &mut Cpc, program: &[u8], max_cycles: u64) {
    let status = run_program_status(cpc, program, max_cycles);
    assert_ne!(status, 0, "the program never finished");
    assert_eq!(
        status, 0xFF,
        "firmware call failed at stage {status} (1=OUT OPEN 2=OUT DIRECT 3=OUT CLOSE 4=IN OPEN 5=IN DIRECT 6=IN CLOSE)"
    );
}

#[test]
fn real_firmware_save_is_recorded_and_loads_back_from_the_written_cdt() {
    let Some(rom) = rom() else {
        eprintln!("skipping: {ROM_PATH} not present");
        return;
    };

    // --- SAVE on one machine -------------------------------------------
    let mut saver = booted(&rom);
    assert_eq!(saver.recorded_pulse_count(), 0, "booting records nothing");
    let source: Vec<u8> = (0..LEN as usize).map(pattern).collect();
    saver.load(SOURCE, &source);
    run_program(&mut saver, &save_program(), 400_000_000);

    let tape = saver
        .recorded_tape()
        .expect("the firmware's SAVE must have written to the cassette line");
    assert!(
        saver.recorded_pulse_count() > 10_000,
        "a 1000-byte SAVE is ~16000 pulses, got {}",
        saver.recorded_pulse_count()
    );
    let cdt = zx_tape::write_tzx(&tape);
    assert_eq!(&cdt[..8], b"ZXTape!\x1a");

    // --- LOAD it back on a fresh machine --------------------------------
    let mut loader = booted(&rom);
    loader.load_tape(zx_tape::parse_tzx(&cdt).expect("the written .cdt parses"));
    run_program(&mut loader, &load_program(), 400_000_000);

    let loaded: Vec<u8> = (0..LEN).map(|i| loader.read_memory(LOADED + i)).collect();
    assert_eq!(
        loaded, source,
        "bytes loaded back from the recorded tape differ from what was saved"
    );

    // --- the same recording as a .voc (8-bit PCM at 44.1 kHz) ------------
    let voc = zx_tape::write_voc(&tape, 44_100).expect("the recording writes as .voc");
    let mut voc_loader = booted(&rom);
    voc_loader.load_tape(zx_tape::parse_voc(&voc).expect("the written .voc parses"));
    run_program(&mut voc_loader, &load_program(), 400_000_000);
    let from_voc: Vec<u8> = (0..LEN)
        .map(|i| voc_loader.read_memory(LOADED + i))
        .collect();
    assert_eq!(
        from_voc, source,
        "bytes loaded back from the recorded .voc differ"
    );
}

/// Pulse durations (3.5469 MHz T-states) of a CDT written by
/// `zx_tape::write_tzx`: only `0x13` pulse-sequence and `0x20` pause blocks.
fn cdt_pulses(cdt: &[u8]) -> Vec<u16> {
    let mut pulses = Vec::new();
    let mut at = 10;
    while at < cdt.len() {
        match cdt[at] {
            0x20 => at += 3,
            0x13 => {
                let count = usize::from(cdt[at + 1]);
                for i in 0..count {
                    let o = at + 2 + i * 2;
                    pulses.push(u16::from_le_bytes([cdt[o], cdt[o + 1]]));
                }
                at += 2 + count * 2;
            }
            other => panic!("unexpected block 0x{other:02X} in a recorded CDT"),
        }
    }
    pulses
}

/// A CDT that opens straight on a pilot tone only `pilot_ms` long -- how
/// Arkanoid, Killapede and others are stored (the blank lead-in of the real
/// tape trimmed off).
fn pilot_first_cdt(cdt: &[u8], pilot_ms: u32) -> Vec<u8> {
    let all = cdt_pulses(cdt);
    // Skip any leading gap pulses: the pilot starts where 100 consecutive
    // pulses agree within +/-25%.
    let near =
        |a: u16, b: u16| u32::from(a) * 4 < u32::from(b) * 5 && u32::from(a) * 5 > u32::from(b) * 4;
    let start = (0..all.len() - 100)
        .find(|&i| all[i..i + 100].iter().all(|&p| near(p, all[i]) && p < 4000))
        .expect("a pilot tone");
    let pulses = &all[start..];
    let first = u32::from(pulses[0]);
    let pilot_len = pulses.iter().take_while(|&&p| near(p, pulses[0])).count();
    let wanted = (u64::from(pilot_ms) * 3_546_900 / 1000 / u64::from(first)) as usize;
    assert!(
        pilot_len > wanted,
        "recorded pilot ({pilot_len} pulses) is shorter than the {wanted} wanted"
    );
    let kept = &pulses[pilot_len - wanted..];
    let mut out = cdt[..10].to_vec();
    for chunk in kept.chunks(255) {
        out.push(0x13);
        out.push(chunk.len() as u8);
        for pulse in chunk {
            out.extend_from_slice(&pulse.to_le_bytes());
        }
    }
    out
}

/// The same recording trimmed so the CDT opens straight on a short pilot. The
/// real firmware spends ~2.4 s after motor-on before it samples, so without a
/// leader that pilot is gone and the file is never found; with the default
/// leader it loads.
#[test]
fn a_cdt_that_starts_on_a_short_pilot_needs_the_leading_silence_to_load() {
    let Some(rom) = rom() else {
        eprintln!("skipping: {ROM_PATH} not present");
        return;
    };
    let mut saver = booted(&rom);
    let source: Vec<u8> = (0..LEN as usize).map(pattern).collect();
    saver.load(SOURCE, &source);
    run_program(&mut saver, &save_program(), 400_000_000);
    let cdt = zx_tape::write_tzx(&saver.recorded_tape().expect("recorded SAVE"));
    let tape = zx_tape::parse_tzx(&pilot_first_cdt(&cdt, 2200)).expect("parses");

    let mut without = booted(&rom);
    without.set_tape_leader_ms(0);
    without.load_tape(tape.clone());
    let status = run_program_status(&mut without, &load_program(), 250_000_000);
    let got: Vec<u8> = (0..LEN).map(|i| without.read_memory(LOADED + i)).collect();
    assert!(
        status != 0xFF && got != source,
        "without a leader the first block is missed (status {status:#04X})"
    );

    let mut with = booted(&rom);
    assert_eq!(with.tape_leader_ms(), cpc::DEFAULT_TAPE_LEADER_MS);
    with.load_tape(tape);
    run_program(&mut with, &load_program(), 400_000_000);
    let got: Vec<u8> = (0..LEN).map(|i| with.read_memory(LOADED + i)).collect();
    assert_eq!(got, source, "with the default leader the file loads");
}
