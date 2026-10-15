//! End-to-end disk proof against the *real* CPC firmware: boot a genuine
//! CPC6128 ROM (skipped when the ROM file isn't present), then have the
//! real AMSDOS write a file to a freshly formatted disk and read it back
//! through the emulated uPD765A -- the whole Format-free write/read command
//! set real software actually sends (`0x45`/`0x46` with MFM flags, Sense
//! Drive Status, Seek/Recalibrate, Sense Interrupt Status, execution-phase
//! byte transfers, end-of-cylinder termination).
//!
//! This is the check the unit tests can't give: they use hand-built command
//! bytes, this uses whatever bytes AMSDOS really sends.

use cpc::{Cpc, Model};
use machine::Machine;

const ROM_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../roms/cpc/cpc6128.rom");

fn rom() -> Option<Vec<u8>> {
    std::fs::read(ROM_PATH).ok()
}

/// A blank CPC "DATA format" disk: 40 tracks, one side, 9 sectors of 512
/// bytes (IDs `0xC1..=0xC9`), filled with `0xE5` (empty directory).
fn blank_data_disk() -> dsk::Disk {
    let mut disk = dsk::Disk::new(1);
    for c in 0..40u8 {
        let ids: Vec<[u8; 4]> = (1..=9).map(|r| [c, 0, 0xC0 + r, 2]).collect();
        assert!(disk.format_track(c, 0, &ids, 0x4E, 0xE5));
    }
    disk.clear_modified();
    disk
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

/// Tiny Z80 byte assembler with labels resolved for `JR NC` fixups.
struct Asm {
    org: u16,
    bytes: Vec<u8>,
    fails: Vec<usize>,
}

impl Asm {
    fn new(org: u16) -> Self {
        Self {
            org,
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
    /// `JR NC, fail`: taken when the firmware reports failure (carry clear).
    /// Records the stage number so a failure says *which* call failed.
    fn check(&mut self, stage: u8) -> &mut Self {
        self.emit(&[0x3E, stage]); // LD A, stage
        self.emit(&[0x30, 0x00]); // JR NC, fail (patched)
        self.fails.push(self.bytes.len() - 1);
        self
    }
    fn finish(mut self, status_addr: u16) -> Vec<u8> {
        // success: LD A,0xFF ; LD (status),A ; HALT
        self.emit(&[
            0x3E,
            0xFF,
            0x32,
            status_addr as u8,
            (status_addr >> 8) as u8,
            0x76,
        ]);
        let fail = self.bytes.len();
        // fail: LD (status),A ; HALT   (A still holds the failing stage)
        self.emit(&[0x32, status_addr as u8, (status_addr >> 8) as u8, 0x76]);
        for at in std::mem::take(&mut self.fails) {
            self.bytes[at] = (fail - (at + 1)) as u8;
        }
        let _ = self.org;
        self.bytes
    }
}

const FIRMWARE: (u16, u16, u16, u16, u16, u16) = (
    0xBC8C, // CAS OUT OPEN
    0xBC98, // CAS OUT DIRECT
    0xBC8F, // CAS OUT CLOSE
    0xBC77, // CAS IN OPEN
    0xBC83, // CAS IN DIRECT
    0xBC7A, // CAS IN CLOSE
);

const PROGRAM: u16 = 0x4000;
const FILENAME: u16 = 0x4100;
const STATUS: u16 = 0x4200;
const IO_BUFFER: u16 = 0x6000;
const SOURCE: u16 = 0x8000;
const LOADED: u16 = 0x9000;
const LEN: u16 = 3000; // spans several 512-byte sectors and 1K blocks

fn pattern(i: usize) -> u8 {
    ((i * 7 + 3) & 0xFF) as u8
}

fn write_then_read_program() -> Vec<u8> {
    let (out_open, out_direct, out_close, in_open, in_direct, in_close) = FIRMWARE;
    let mut a = Asm::new(PROGRAM);
    // Write: OPEN, DIRECT (binary file, HL=data DE=len BC=exec A=type), CLOSE.
    a.ld_hl(FILENAME)
        .emit(&[0x06, 8])
        .ld_de(IO_BUFFER)
        .call(out_open)
        .check(1);
    a.ld_hl(SOURCE)
        .ld_de(LEN)
        .ld_bc(0)
        .emit(&[0x3E, 2])
        .call(out_direct)
        .check(2);
    a.call(out_close).check(3);
    // Read it back: OPEN, DIRECT (HL = destination), CLOSE.
    a.ld_hl(FILENAME)
        .emit(&[0x06, 8])
        .ld_de(IO_BUFFER)
        .call(in_open)
        .check(4);
    a.ld_hl(LOADED).call(in_direct).check(5);
    a.call(in_close).check(6);
    a.finish(STATUS)
}

#[test]
fn real_amsdos_writes_a_file_and_reads_it_back_through_the_fdc() {
    let Some(rom) = rom() else {
        eprintln!("skipping: {ROM_PATH} not present");
        return;
    };
    let mut cpc = Cpc::new(rom, Model::Cpc6128).unwrap();
    cpc.load_disk(blank_data_disk()).unwrap();

    // Let the real firmware boot to the BASIC prompt and initialise AMSDOS.
    run_until(&mut cpc, 25_000_000, |_| false);

    let source: Vec<u8> = (0..LEN as usize).map(pattern).collect();
    cpc.load(SOURCE, &source);
    cpc.load(FILENAME, b"TEST.BIN");
    cpc.load(PROGRAM, &write_then_read_program());
    cpc.load(STATUS, &[0]);
    cpc.set_pc(PROGRAM);

    run_until(&mut cpc, 120_000_000, |c| c.read_memory(STATUS) != 0);

    let status = cpc.read_memory(STATUS);
    assert_ne!(status, 0, "the program never finished");
    assert_eq!(
        status, 0xFF,
        "firmware call failed at stage {status} (1=OUT OPEN 2=OUT DIRECT 3=OUT CLOSE 4=IN OPEN 5=IN DIRECT 6=IN CLOSE)"
    );

    let loaded: Vec<u8> = (0..LEN).map(|i| cpc.read_memory(LOADED + i)).collect();
    assert_eq!(
        loaded, source,
        "data read back through AMSDOS differs from what was written"
    );

    // The write really changed the medium, and it serialises to a `.dsk`
    // that parses back with the file in its directory.
    let image = cpc
        .take_modified_disk_image()
        .expect("the disk must be marked modified after a write");
    let reparsed = dsk::parse(&image).expect("modified image must still be a valid .dsk");
    assert_eq!(reparsed.cylinders(), 40);
    let directory: Vec<u8> = (0xC1..=0xC4)
        .flat_map(|r| reparsed.read_sector(0, 0, r).unwrap().data.clone())
        .collect();
    let name_at = directory
        .windows(11)
        .position(|w| w == b"TEST    BIN")
        .or_else(|| directory.windows(11).position(|w| w == b"TEST    BIN"));
    assert!(
        name_at.is_some(),
        "TEST.BIN missing from the saved directory"
    );
}
