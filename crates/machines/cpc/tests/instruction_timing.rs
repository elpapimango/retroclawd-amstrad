//! CPC instruction timing, checked against two independent references.
//!
//! Every opcode of every prefix table is executed on a real `Cpc` (CPC464
//! bus model, zeroed ROM/RAM) and its elapsed time -- including the slot
//! alignment it charges to whatever runs next -- must be a whole number of
//! NOPs equal to Caprice32's per-opcode NOP table (`src/z80.cpp`'s `cc_op`/
//! `cc_cb`/`cc_ed`/`cc_xy`/`cc_xycb`/`cc_ex`, read directly; GPL-2.0-or-later,
//! numbers only), with taken and not-taken branches both covered, and with
//! the instruction started from three different Gate Array slot phases (by
//! running a short instruction first).
//!
//! The same cases were also fed (outside this repository, see `CREDITS.md`)
//! to ep128emu's real Z80 core driven with its own CPC bus-timing formulas,
//! an access-level model that shares nothing with Caprice32's tables: it
//! agreed with both on every comparable case. CPCEC's source (`cpcec-z8.h`)
//! shows the same structure for the one family this test originally caught
//! (`PUSH`/`RST`/`RET cc`/`DJNZ`: an internal T-state *before* the bus
//! accesses that follow it). `IN`/`OUT` are covered too, with the effective
//! totals Caprice32's `z80_wait_states` macro produces (it consumes the table
//! value before the port access and then resets the count to a remainder) --
//! 12 T for `IN A,(n)`/`OUT (n),A`, 16 T for `IN r,(C)`/`OUT (C),r`, 20 T for
//! `INI`/`IND`/`OUTI`/`OUTD` -- which ep128emu's access-level core reproduces
//! exactly (12/16/20). What isn't covered: `HALT` and interrupt acceptance.

use cpc::{Cpc, Model};
use machine::Machine;

// Generated from Caprice32 src/z80.cpp (cc_op/cc_cb/cc_ed/cc_xy/cc_xycb/cc_ex), read directly.
const CC_OP: [u8; 256] = [
    4, 12, 8, 8, 4, 4, 8, 4, 4, 12, 8, 8, 4, 4, 8, 4, 12, 12, 8, 8, 4, 4, 8, 4, 12, 12, 8, 8, 4, 4,
    8, 4, 8, 12, 20, 8, 4, 4, 8, 4, 8, 12, 20, 8, 4, 4, 8, 4, 8, 12, 16, 8, 12, 12, 12, 4, 8, 12,
    16, 8, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4,
    4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 8, 8, 8, 8, 8, 8, 4, 8, 4,
    4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4,
    4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4,
    4, 4, 4, 4, 4, 8, 4, 8, 12, 12, 12, 12, 16, 8, 16, 8, 12, 12, 4, 12, 20, 8, 16, 8, 12, 12, 8,
    12, 16, 8, 16, 8, 4, 12, 12, 12, 4, 8, 16, 8, 12, 12, 24, 12, 16, 8, 16, 8, 4, 12, 4, 12, 4, 8,
    16, 8, 12, 12, 4, 12, 16, 8, 16, 8, 8, 12, 4, 12, 4, 8, 16,
];
const CC_CB: [u8; 256] = [
    4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4,
    12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4,
    4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4,
    4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4,
    4, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4,
    4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4,
    4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4,
    12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4, 4, 4, 4, 4,
    4, 4, 12, 4, 4, 4, 4, 4, 4, 4, 12, 4,
];
const CC_ED: [u8; 256] = [
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    12, 8, 12, 20, 4, 12, 4, 8, 12, 8, 12, 20, 4, 12, 4, 8, 12, 8, 12, 20, 4, 12, 4, 8, 12, 8, 12,
    20, 4, 12, 4, 8, 12, 8, 12, 20, 4, 12, 4, 16, 12, 8, 12, 20, 4, 12, 4, 16, 12, 8, 12, 20, 4,
    12, 4, 4, 12, 8, 12, 20, 4, 12, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 16, 12, 16, 12, 4, 4, 4, 4, 16, 12, 16, 12, 4, 4, 4, 4,
    16, 12, 16, 12, 4, 4, 4, 4, 16, 12, 16, 12, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
];
const CC_XY: [u8; 256] = [
    4, 12, 8, 8, 4, 4, 8, 4, 4, 12, 8, 8, 4, 4, 8, 4, 12, 12, 8, 8, 4, 4, 8, 4, 12, 12, 8, 8, 4, 4,
    8, 4, 8, 12, 20, 8, 4, 4, 8, 4, 8, 12, 20, 8, 4, 4, 8, 4, 8, 12, 16, 8, 20, 20, 20, 4, 8, 12,
    16, 8, 4, 4, 8, 4, 4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4,
    4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 16, 16, 16, 16, 16,
    16, 4, 16, 4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4,
    4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 4,
    4, 4, 4, 4, 4, 16, 4, 4, 4, 4, 4, 4, 4, 16, 4, 8, 12, 12, 12, 12, 16, 8, 16, 8, 12, 12, 4, 12,
    20, 8, 16, 8, 12, 12, 8, 12, 16, 8, 16, 8, 4, 12, 12, 12, 4, 8, 16, 8, 12, 12, 24, 12, 16, 8,
    16, 8, 4, 12, 4, 12, 4, 8, 16, 8, 12, 12, 4, 12, 16, 8, 16, 8, 8, 12, 4, 12, 4, 8, 16,
];
const CC_XYCB: [u8; 256] = [
    20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
    20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
    20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 16, 16, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
    20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
    20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
    20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
    20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
    20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
];
const CC_EX: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    4, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 8, 4, 4, 0, 0, 0, 0, 4, 8, 4, 4, 0, 0, 0, 0,
    8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0,
    8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0, 8, 0, 0, 0,
];

const PROGRAM: u16 = 0x4000;

fn fresh() -> Cpc {
    Cpc::new(vec![0u8; Model::Cpc464.rom_image_size()], Model::Cpc464).unwrap()
}

#[derive(Clone, Copy)]
struct Setup {
    f: u8,
    b: u8,
}

fn prepare(bytes: &[u8], setup: Setup) -> Cpc {
    let mut cpc = fresh();
    cpc.load(PROGRAM.into(), bytes);
    cpc.set_pc(PROGRAM.into());
    // Memory operands land in zeroed RAM; RET/POP read a return address of
    // 0 (all NOPs); the stack is in RAM.
    for (name, value) in [
        ("HL", 0x8100u64),
        ("DE", 0x8300),
        ("IX", 0x8400),
        ("IY", 0x8500),
        ("SP", 0x9000),
        ("A", 0x55),
        ("F", u64::from(setup.f)),
        ("B", u64::from(setup.b)),
        ("C", 1),
    ] {
        cpc.set_register(name, value).unwrap();
    }
    cpc
}

/// Elapsed T-states of `prelude` plus `instruction`, including the slot
/// alignment the instruction charges to the NOP that follows it. (A prelude
/// that leaves the bus mid-slot makes the *instruction* wait, so only the sum
/// is comparable with the NOP-granular reference.)
fn measure(prelude: &[u8], instruction: &[u8], setup: Setup) -> u32 {
    let mut bytes = prelude.to_vec();
    bytes.extend_from_slice(instruction);
    let mut cpc = prepare(&bytes, setup);
    let before = if prelude.is_empty() { 0 } else { cpc.step() };
    let first = cpc.step();
    let next = cpc.step(); // a NOP: 4 T-states when everything is aligned
    before + first + next - 4
}

/// Whether `instruction` (run after `prelude`, which may change the flags)
/// left the straight-line path, i.e. took a branch.
fn took_branch(prelude: &[u8], instruction: &[u8], setup: Setup) -> bool {
    let mut bytes = prelude.to_vec();
    bytes.extend_from_slice(instruction);
    let mut cpc = prepare(&bytes, setup);
    if !prelude.is_empty() {
        cpc.step();
    }
    let start = cpc.pc();
    cpc.step();
    cpc.pc() != start + z80::disassemble::disassemble(instruction).1 as u64
}

/// Effective totals for the port instructions, whose Caprice32 table entry is
/// consumed before the access and replaced by a remainder (see the module docs).
fn io_nops(prefix: &[u8], opcode: u8, taken: bool) -> Option<u32> {
    match prefix {
        [] if matches!(opcode, 0xD3 | 0xDB) => Some(12),
        [0xDD] | [0xFD] if matches!(opcode, 0xD3 | 0xDB) => Some(16), // prefix is a separate 4 T
        [0xED] if matches!(opcode, 0x40..=0x7F) && opcode & 7 < 2 => Some(16),
        [0xED] if matches!(opcode, 0xA2 | 0xA3 | 0xAA | 0xAB) => Some(20),
        [0xED] if matches!(opcode, 0xB2 | 0xB3 | 0xBA | 0xBB) => Some(if taken { 24 } else { 20 }),
        _ => None,
    }
}

fn nops(prefix: &[u8], opcode: u8, taken: bool) -> u32 {
    if let Some(total) = io_nops(prefix, opcode, taken) {
        return total;
    }
    let extra = |idx: u8| {
        if taken {
            u32::from(CC_EX[idx as usize])
        } else {
            0
        }
    };
    match prefix {
        [] => u32::from(CC_OP[opcode as usize]) + extra(opcode),
        [0xCB] => u32::from(CC_OP[0xCB]) + u32::from(CC_CB[opcode as usize]),
        [0xED] => u32::from(CC_OP[0xED]) + u32::from(CC_ED[opcode as usize]) + extra(opcode),
        [0xDD] | [0xFD] => {
            u32::from(CC_OP[prefix[0] as usize]) + u32::from(CC_XY[opcode as usize]) + extra(opcode)
        }
        [0xDD, 0xCB] | [0xFD, 0xCB] => {
            u32::from(CC_OP[prefix[0] as usize])
                + u32::from(CC_XY[0xCB])
                + u32::from(CC_XYCB[opcode as usize])
        }
        _ => unreachable!(),
    }
}

/// Opcodes the single-instruction sweep can't time: `HALT`, the prefixes
/// themselves, and the repeating block instructions (covered per iteration by
/// `block_instructions_take_the_reference_time_per_iteration`).
fn excluded(prefix: &[u8], opcode: u8) -> bool {
    match prefix {
        [] => matches!(opcode, 0x76 | 0xCB | 0xED | 0xDD | 0xFD),
        [0xED] => matches!(opcode, 0xB0..=0xB3 | 0xB8..=0xBB),
        [0xDD] | [0xFD] => matches!(opcode, 0xCB | 0xED | 0xDD | 0xFD),
        _ => false,
    }
}

/// Instruction lengths: the disassembler knows them for every table.
fn encode(prefix: &[u8], opcode: u8) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    if prefix == [0xDD, 0xCB] || prefix == [0xFD, 0xCB] {
        bytes.extend_from_slice(&[0x10, opcode]); // displacement, then the sub-opcode
    } else {
        bytes.push(opcode);
        bytes.extend_from_slice(&[0x10, 0x81]); // operand bytes; harmless when unused
    }
    bytes.truncate(z80::disassemble::disassemble(&bytes).1);
    bytes
}

#[test]
fn every_opcode_takes_the_reference_number_of_nops_from_any_slot_phase() {
    // Setups: flags clear / B=1 (conditions on "not set" hold, DJNZ falls
    // through) and flags set / B=2 (the opposite). Together every conditional
    // is taken once and not taken once.
    let setups = [Setup { f: 0x00, b: 1 }, Setup { f: 0xFF, b: 2 }];
    // Preludes that leave the next instruction at different slot phases.
    let preludes: [(&[u8], u32); 3] = [(&[], 0), (&[0x03], 8), (&[0x09], 12)]; // -, INC BC, ADD HL,BC

    let mut failures = Vec::new();
    let mut checked = 0;
    for prefix in [
        &[][..],
        &[0xCB],
        &[0xED],
        &[0xDD],
        &[0xFD],
        &[0xDD, 0xCB],
        &[0xFD, 0xCB],
    ] {
        for opcode in 0..=255u8 {
            if excluded(prefix, opcode) {
                continue;
            }
            let instruction = encode(prefix, opcode);
            for setup in setups {
                for (prelude, prelude_t) in preludes {
                    let taken = took_branch(prelude, &instruction, setup);
                    let want = prelude_t + nops(prefix, opcode, taken);
                    let got = measure(prelude, &instruction, setup);
                    checked += 1;
                    if got != want {
                        failures.push(format!(
                            "{:02X?} {:02X} (taken={taken}, after {:02X?}): {got} T, reference {want} T",
                            prefix, opcode, prelude
                        ));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} timings disagree with the reference:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(checked > 9000, "the sweep covered {checked} cases");
}

/// Repeating block instructions: two repeating iterations then a final one,
/// followed by the alignment charged to the next instruction.
#[test]
fn block_instructions_take_the_reference_time_per_iteration() {
    // LDIR/CPIR/LDDR/CPDR count down BC; INIR/OTIR/INDR/OTDR count down B.
    for sub in [0xB0u8, 0xB1, 0xB8, 0xB9, 0xB2, 0xB3, 0xBA, 0xBB] {
        let mut cpc = prepare(&[0xED, sub], Setup { f: 0, b: 0 });
        let counts_b = matches!(sub, 0xB2 | 0xB3 | 0xBA | 0xBB);
        cpc.set_register("BC", if counts_b { 0x0301 } else { 3 })
            .unwrap();
        let elapsed: u32 = (0..3).map(|_| cpc.step()).sum::<u32>() + cpc.step() - 4;
        let repeating = nops(&[0xED], sub, true);
        let last = nops(&[0xED], sub, false);
        assert_eq!(
            elapsed,
            2 * repeating + last,
            "ED {sub:02X} over three iterations"
        );
    }
}
