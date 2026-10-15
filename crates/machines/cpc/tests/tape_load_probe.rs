//! Exploratory: types `RUN"` on a real CPC464 firmware, plays a game tape in
//! real time and prints PC / tape position as it goes, to see whether the
//! loader gets past its protection checks. Ignored: slow, and needs the
//! firmware and tape files.

use cpc::{Cpc, CpcKey, Model};
use machine::Machine;

fn run(cpc: &mut Cpc, cycles: u64) {
    let mut spent = 0u64;
    while spent < cycles {
        spent += u64::from(cpc.step());
    }
}

fn tap(cpc: &mut Cpc, key: CpcKey) {
    cpc.press_key(key);
    run(cpc, 400_000);
    cpc.release_key(key);
    run(cpc, 400_000);
}

#[test]
#[ignore = "exploratory; set TAPE=<path.cdt> and optionally MAX_CYCLES"]
fn load_a_game_tape() {
    let rom = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../roms/cpc/cpc464.rom"
    ))
    .expect("cpc464.rom");
    let path = std::env::var("TAPE").expect("TAPE=<path to .cdt>");
    let max: u64 = std::env::var("MAX_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300_000_000);
    let tape = zx_tape::parse_tzx(&std::fs::read(&path).unwrap()).expect("parse cdt");

    let mut cpc = Cpc::new(rom, Model::Cpc464).unwrap();
    run(&mut cpc, 25_000_000); // boot to the BASIC prompt
    cpc.load_tape(tape);
    for key in [CpcKey::R, CpcKey::U, CpcKey::N] {
        tap(&mut cpc, key);
    }
    cpc.press_key(CpcKey::Shift);
    cpc.press_key(CpcKey::Digit2);
    run(&mut cpc, 400_000);
    cpc.release_key(CpcKey::Digit2);
    cpc.release_key(CpcKey::Shift);
    run(&mut cpc, 400_000);
    tap(&mut cpc, CpcKey::Enter);
    run(&mut cpc, 3_000_000);
    tap(&mut cpc, CpcKey::Space); // "Press PLAY then any key"

    let mut spent = 0u64;
    let mut next_report = 0u64;
    let mut resets = 0;
    let mut last_pc = cpc.pc();
    while spent < max {
        spent += u64::from(cpc.step());
        let pc = cpc.pc();
        // A jump to the firmware reset vector from running code.
        if pc == 0x0000 && last_pc != 0x0000 && last_pc > 0x0100 {
            resets += 1;
            println!("RESET #{resets} at +{spent} cycles (from PC={last_pc:04X})");
        }
        last_pc = pc;
        if spent >= next_report {
            let pos = cpc.tape_playback_state().map(|s| (s.position, s.events));
            println!("t={spent:>10} PC={pc:04X} tape={pos:?}");
            next_report += 10_000_000;
        }
    }
    println!("DONE resets={resets}");
    if let Ok(out) = std::env::var("OUT_PPM") {
        let (w, h, rgb) = cpc.render_frame().expect("frame");
        let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
        data.extend_from_slice(&rgb);
        std::fs::write(out, data).unwrap();
    }
}
