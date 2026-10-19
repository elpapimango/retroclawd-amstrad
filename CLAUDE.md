# CLAUDE.md

Guidance for Claude Code (claude.ai/code) working in this repo.

## What this repository is

The Amstrad CPC machine family of retroclawd, split out of the main repository (`elpapimango/retroclawd`, the application, referred to as "the host"). Everything here builds into `*_machine_plugin` shared libraries that the host application loads at run time. Shared crates (`machine`, `plugin-api`, `z80`, `mos6502`, `ay-3-8910`, `nec765`, `dsk`, `zx-tape`) come from `elpapimango/retroclawd-crates` as a git dependency pinned to a commit (`rev`); changing that rev means checking `docs/plugin-abi.md` in that repository. Directory layout is the same as in the host (tests find ROMs/software at `CARGO_MANIFEST_DIR/../../../roms|software`).

## Commands

- `cargo build --release --workspace` — builds the plugin libraries into `target/release/`
- `cargo test --workspace` — all tests (ROM-gated tests skip when a ROM is absent; `-- --ignored` runs the slow real-firmware ones, use `--release`)
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`
- Private host: `CARGO_NET_GIT_FETCH_WITH_CLI=true` and git credentials for `github.com/elpapimango/retroclawd`

## Scenario manifests (2026-10-18)

`cpc464, cpc664 and cpc6128` have `<id>.manifest.toml` files here: the playbook's standard scenarios (S1 boot, S2 typed input, S3 media, S4 audio, S5 state, S6
timing, S7 hardware test) against ZEsarUX 13.0, run by `rc-scenarios` of `retroclawd-tools` (see the host's `CLAUDE.md` for the command and
environment). The test media come from `scenarios/cpc/hello.asm` (an rcld layout; the disk is only listed with CAT, `RUN"HELLO` ends in a reset in both emulators); `scenarios/build.sh` builds them into the git-ignored `software/scenarios/`. Goldens are
not committed. Known gaps are `status = "known-gap"` entries with a note under `docs/`: S6 (`docs/cpc-timing-gap.md`) and the diagnostics' CRTC/FDC lines (`docs/cpc-diag-crtc-fdc-gap.md`).

## Crate notes

The notes below were written in the host repository before the split and refer to it in places (`retroclawd`'s GUI, `HostedMachine`, `emu_thread`, the CLI flags): those parts are host code and are documented in the host's CLAUDE.md.

- **Second drive (2026-10-16)**: `nec765::Fdc` addresses two units (`DRIVES`, `insert_disk_in(unit, ..)`, per-unit head position in
  `FdcState`; units 2 and 3 and an empty unit are not ready; Sense Interrupt Status keeps the unit bits in ST0; the old `insert_disk`/`disk()`
  mean drive A). `CpcBus` keeps `pristine_disks[2]`; `Cpc::load_disk_in(drive, ..)`, `eject_disk_from`, `disk_in`, `take_modified_disk_image_of`,
  `mark_disk_saved_in`; the save state carries drive B's modified image (`modified_disk_b`). The plugin handles `MediaSlot::Disk { drive }`
  (0 and 1) for attach, detach, `ExportDisk` and `MarkDiskSaved`, and reports two `DiskStatus` entries. The one motor line spins both drives
  (as on the real interface). Tests: controller (independent units, per-drive cylinders, not-ready cases), `Cpc` (reads from A and B,
  independent modified tracking), plugin (drive 1 attach/export/detach, drive 2 refused), and in the host suite the real 6128 firmware reading
  `B:SAME.BIN` through AMSDOS.
- **CPC keyboard: punctuation, function keys and typing (2026-10-16)**: the matrix is now complete and read from the firmware's
  own key translation tables (normal at ROM `0x1D69`, shifted at `0x1DB9`, ten lines of eight bits), which the plugin tests compare
  against (`every_character_a_key_claims_is_what_the_firmware_makes_of_its_matrix_position`, ROM-gated): the keys `Caret`, `Minus`, `At`,
  `Semicolon`, `Colon`, `LeftBracket`, `RightBracket`, `Backslash`, `F0`-`F9` and `FDot` exist, and 21 shifted symbols are combo keys
  (`DoubleQuote` = Shift+2, `Exclamation`, `Hash`, `Dollar`, `Percent`, `Ampersand`, `Apostrophe`, `LeftParen`, `RightParen`,
  `Underscore`, `Equals`, `Pound`, `Pipe`, `LeftBrace`, `RightBrace`, `Backtick`, `Plus`, `Asterisk`, `Question`, `GreaterThan`,
  `LessThan`) that press Shift and the base key together, with their characters in `CPC_CHARS` so a symbolic layout, `--type` and paste can
  produce them. Before this only `, . /` were mapped, so BASIC could not be typed headless (no `"`). **Two real bugs found on the way:**
  `Comma` and `Period` were swapped (`,` is line 4 bit 7 and `.` line 3 bit 7), and the plugin pressed joystick 1 Fire 1 together with Space,
  which the firmware turns into the character `X` -- every typed space came out as "X "; that alias is gone (arrows still press the
  joystick directions; bind `Joy1Fire1` with `[keys]` for games). The `rc-suite` CPC case types a program of every symbol into the real
  firmware and reads the string BASIC built back out of memory.
- **Classic ink writes on a CPC+ (2026-10-16)**: a Plus takes its colours from the ASIC palette (`video_state`), and the Gate Array
  ink write (`OUT (&7Fxx)`, function `01`) used to be dropped there, so every program that sets colours the old way -- every CPC
  program, and the GX4000 diagnostics cartridge -- showed black. `Asic::set_legacy_ink` now writes the 12-bit form of the hardware colour
  into the pen's palette entry (levels off/half/full become `0`/`8`/`F`, which renders like the classic palette). Unit test
  `a_classic_ink_write_sets_the_asic_palette_of_a_plus_model` (mutation-checked). The earlier note that the cartridge "parks in a VSYNC
  wait" was wrong: it ran, with a black screen; it programs the CRTC itself.
- **Malformed `.cpr` images (2026-10-16)**: `cpc_cartridge::parse` stays strict; `parse_lenient` (used by the plugin for the creation ROM
  and cartridge attach) repairs the two defects of real images -- a RIFF size field that does not match the file and a last page shorter
  than its chunk header says (zero-filled) -- and reports each repair on stderr. The released `AmstradDiag.cpr` has both (size `0x040C`,
  page 20 bytes short). Anything else (signature, chunk ids, duplicate pages, a cut chunk header) is still an error.
- **`i8255`** (`crates/chips/i8255`) — Intel 8255 PPI, **mode 0 (basic
  I/O) only** (modes 1/2 exist on real hardware but nothing this project
  models uses them; a mode-set write's group-mode bits are stored/
  echoed back but never change `read_*`/`write_*` behavior). Same
  host-agnostic-chip convention as `ay-3-8910`/`vic-ii`: 3 independently
  directional 8-bit ports (`Port C`'s two nibbles independently, per
  real hardware), `set_input_*`/real port reads split the same "driven
  output reflects what was written, undriven input reflects the outside
  world" way `c64::bus::C64Bus`'s 6510-port/CIA1-port handling already
  does. Real, commonly-documented 8255 behavior modeled: default
  power-up state is every port input; any mode-set write resets every
  port's output latch to `0`; the bit-set/reset sub-mode only ever
  touches Port C, direction untouched.
- **`cpc`** (`crates/machines/cpc`) — CPC464/CPC664/CPC6128 selected by
  `cpc::Model`, not separate machine crates. Standard combined ROM images are
  32K for 464 (OS+BASIC) or 48K for 664/6128 (OS+BASIC+AMSDOS); upper-ROM
  slot 0 is BASIC, slot 7 is AMSDOS, and unpopulated slots fall back to BASIC.
  CPC464/664 have 64K RAM; CPC6128 has eight physical 16K banks and the exact
  eight PAL paging configurations cited in `CREDITS.md`. The CRTC always reads
  primary RAM regardless of the CPU mapping. CPC664/6128 reuse `nec765` and
  `dsk` behind CPC's loose motor/MSR/data port decode; the controller is
  the full uPD765A described under "FDC rewrite and disk write path" below
  (instant execution, no overrun, drive 0 only).
  Model-checked internal save states include all RAM, paging, upper-ROM latch,
  disk motor, and FDC phase but exclude externally supplied ROM/disk bytes.
  `z80::Cpu` + private `CpcBus` also contain `crate::gate_array::GateArray`
  (pen/border/ink selection, ROM control, HSYNC-latched mode, frame-locked
  interrupts), the type-aware `crate::crtc::Crtc`, and the per-character
  video trace in `crate::raster` (see "CRTC types and per-character video"
  below).
  Real CPC I/O port decode is famously loose (the Gate
  Array only decodes 2 address bits) -- `CpcBus::write_io` uses
  independent `if`s, not an `else if` chain, since a single real `OUT`
  can legitimately hit more than one chip's decode range at once, a
  real quirk some CPC software relies on. The genuinely tricky part this
  track's own research flagged in advance and got right: CPC has no
  direct AY register/data ports at all (unlike ZX Spectrum 128K's
  `0xfffd`/`0xbffd`) -- every PSG access goes through `i8255::Ppi`'s
  Port A (the data byte) plus Port C's BDIR/BC1 bits (bits 7:6, function
  select: `01`=read/`10`=write/`11`=select-register), and `CpcBus::
  write_port_c` is where that whole real protocol actually lives -- a
  `01` (read) response is fed into the PPI's own Port A *input* latch
  (`set_input_a`) rather than answered directly, so a later Port A `IN`
  (once firmware has correctly reconfigured Port A as PPI input, exactly
  like real firmware must -- the 8255 would otherwise still be actively
  driving that bus, electrically conflicting with the AY) reads it back
  through the PPI's own existing input/output mechanism with zero
  special-casing needed on the read path itself. CPU memory accesses use
  the existing `machine::Bus::contend_memory`/`advance_tstates` hooks to
  align each access to one of the Gate Array's four-T-state RAM slots;
  `CpcBus` reconciles non-bus instruction cycles afterward and serializes
  the live slot phase. **Boots a real firmware ROM to
  an actual boot screen** (`--headless --dump-screen`-verified, not
  just unit-tested) on the first real-ROM attempt. A real lurking bug
  *was* found later, just not by that first check (fixed 2026-09-18,
  user tried the actual GUI window and hit "no video output implemented
  for cpc464 yet" even though `--dump-screen` worked fine): `gate_array::
  GateArray::new` defaulted `screen_mode` to the derived-`Default` `0`
  instead of real hardware's actual Mode-1 power-on state.
  At the time, `Cpc::render_frame` returned `None` for any mode but 1,
  and `retroclawd`'s GUI startup path checks
  `render_frame().is_some()` on the machine exactly as constructed --
  before a single instruction has run. `--headless --dump-screen` never
  hit this: it renders only *after* running real cycles, by which point
  the firmware's own boot code had already corrected the mode to 1
  itself, masking the wrong default. Same "boots at t=0, not just
  eventually" bar every other machine here already meets (`zx81`/
  `zx_spectrum`'s `render_frame` never depends on any runtime-settable
  mode at all) -- see `gate_array::GateArray::new`'s own doc comment for
  the fix and its regression tests. `gate_array::PALETTE` and
  `crate::keyboard` (`CpcKey`/`KeyMatrix`, the real 10x8 matrix scanned
  through PPI Port C row-select + Port A/AY-register-14 read-back, same
  real protocol `write_port_c` already drives) both now use real tables
  fetched via `WebSearch`/`WebFetch` (which reach real pages even though
  this sandbox's own `Bash`/`curl` cannot -- confirmed live) from
  CPCTech's own Gate Array/keyboard docs, not guesses -- boot screen
  shows the real classic blue, and `AnyMachine::Cpc`'s `key_event`
  covers letters/digits/space/enter/shift/ctrl/capslock/tab/escape/
  delete/the 4 dedicated cursor keys/comma/period/slash. **CPC joystick 1**
  (`CpcKey::Joy1Up`/`Joy1Down`/`Joy1Left`/`Joy1Right`/`Joy1Fire1`/
  `Joy1Fire2`/`Joy1Fire3`, matrix line 9 alongside `Delete`, bit order
  DEL(7)/Fire3(6)/Fire2(5)/Fire1(4)/Right(3)/Left(2)/Down(1)/Up(0) per
  CPCTech's own table) is a genuinely separate physical device from the
  cursor keys on real hardware (unlike ZX Spectrum's Sinclair/Cursor
  joysticks, which really are wired onto keyboard-matrix keys) but reuses
  the same 10x8 matrix scan mechanism. **Joystick 2** (`CpcKey::Joy2Up`/
  `Joy2Down`/`Joy2Left`/`Joy2Right`/`Joy2Fire1`/`Joy2Fire2`/`Joy2Fire3`) is
  wired onto matrix line 6 -- the same physical lines as the `6 5 R T G F B`
  keys (Up = bit 0 ... Fire3 = bit 6) -- so `KeyMatrix` tracks it in its own
  `joystick2` byte OR-ed into line 6 on read: a typed `6` and a pushed stick
  are independent sources of one wired-AND line, and releasing one never
  clears a line the other still holds. Stable plugin key names `Joy1*`/
  `Joy2*` (`cpc_key_from_name`) let the `[keys]` config bind any host key to
  any joystick wire. **Regression found and fixed 2026-10-01**: the host's
  old "arrow keys/Space also press joystick 1" rule lived in the deleted
  `any_machine::cpc_joystick_key` and was silently dropped when key mapping
  moved into the machine plugins, so joystick 1 was unreachable from the GUI
  while this paragraph still claimed otherwise. The CPC plugin's `KeyEvent`
  handler now restores it (`cpc_joystick1_alias`: arrows + Space press the
  cursor/space keys AND joystick 1's directions/Fire1, same "rides the same
  physical keys" precedent as `zx_spectrum`'s Kempston-via-arrows), covered
  by plugin tests driving the real ABI. Fire2/Fire3 and all of joystick 2
  have no default host key (no natural second set); bind them via
  `[machines.cpc*.keys]`. Found missing while root-causing a real Arkanoid.cdt
  hang (see the cassette bullet below).
  Audio: same
  pipeline shape as `zx_spectrum` (`audio_cycle_accumulator: f64` +
  `ay_cycle_debt: u32`, ticked in `Machine::step`, `mix_audio_sample`
  feeding `audio_buffer`/`drain_audio`) -- CPC's real AY clock divider is
  CPU-clock/4 (1MHz), not Spectrum 128K's /2, the only difference; the AY
  chip itself (`ay-3-8910`) and its PPI-mediated access protocol were
  already in place from keyboard work, so this was pure host wiring, no
  new chip logic. Cassette input is now wired too: `.cdt`/`.tzx` through
  `zx-tape`, Port C bit 4 motor control, Port B bit 7 input, immediate
  motor-off transport pause with idle-high EAR (`TapeMotorMode::
  CapriceStrict`, default -- see its own doc comment on `TapeMotorMode`
  near the top of this file for the full Caprice32/ZEsarUX comparison),
  and save-state round-trips of motor/tape position.
  **Arkanoid.cdt -- earlier diagnosis withdrawn.** An earlier pass blamed a
  cycle-timing drift in a `0x0030` patch/restore loop and `RST 0x30` at
  `0xB9AF`. That was wrong: the traces were of a tape whose first file had
  never been read at all (see "Cassette loading: motor, pause edge, leader"
  below for what actually stopped it, and what is still open).

  **Instruction-timing audit against reference emulators (2026-10-01)** --
  the external reference the paragraph above said was missing turned out to
  be available: `crates/machines/cpc/tests/instruction_timing.rs` runs every
  opcode of every prefix table (CB/ED/DD/FD/DDCB/FDCB, taken and not-taken
  branches, three different Gate Array slot phases -- 10470 timings) on a real
  `Cpc` and requires the elapsed time, *including the alignment it charges to
  the next instruction*, to equal Caprice32's per-opcode NOP tables
  (`src/z80.cpp` `cc_op`/`cc_cb`/`cc_ed`/`cc_xy`/`cc_xycb`/`cc_ex`, numbers
  only). The same cases were also fed (scratch tooling, not in the repo) to
  ep128emu's real Z80 core driven with its own CPC bus-timing formulas
  (`src/cpc464vm.cpp`'s `memoryWait`/`memoryWaitM1`/`ioPortWait`/
  `updateCycle(s)`), an access-level model sharing nothing with Caprice32's
  tables: it agreed with Caprice32 on every comparable case, and CPCEC's
  `cpcec-z8.h` (a third source) is structured the same way. **Result: the
  base model was right (2793 of 3506 cases agreed on the first run) except
  for one family** -- `PUSH`, `RST`, `RET cc` (taken) and `DJNZ` (not taken),
  and their `DD`/`FD` forms, were one NOP too fast: each has an internal
  T-state *before* a later bus access (the extended `M1`), and `CpcBus` only
  reconciled internal cycles at the end of the instruction, so the following
  write/read was aligned against the wrong slot phase. Fixed with a new
  defaulted `machine::Bus::internal_cycles(n)` hook (the `z80` core calls it
  from the two `push16` helpers, `djnz` and taken `ret_cc`; `CpcBus` counts
  it like elapsed time; every other bus keeps the no-op default, so the
  Spectrum/ZX81/C64 are untouched). A mutation check (hook disabled) makes the
  test fail with 297 disagreements. Block instructions (`LDIR` etc.) were
  verified per iteration (`24`/`24`/`20` T-states) and match all three
  references. **A second real bug, found by extending the same comparison to
  port instructions**: `CpcBus` never aligned `IN`/`OUT` cycles (its
  `contend_io` was the no-op default), so `OUT (C),r`/`IN r,(C)` took 12
  T-states (3 NOPs) and `INI`/`OUTI` 16, where Caprice32's effective totals
  (its `z80_wait_states` macro consumes the table value before the port access
  and then resets the count to a remainder) and ep128emu's measured core both
  give **16** and **20**; `OUT (n),A`/`IN A,(n)` were right at 12 by luck.
  `OUT (C),r` is the standard CPC idiom for Gate Array/CRTC writes, so this
  mattered to every raster-timed program. Working ep128emu's `ioPortWait` through
  by hand shows an I/O cycle always ends 3 T-states into a slot (a memory cycle
  does too), i.e. `4 + ((3 - phase) & 3)` T-states in our model's terms:
  `CpcBus::contend_io` now returns `(7 - phase) & 3`, and the block I/O
  handlers (`INI`/`IND`/`OUTI`/`OUTD` and their repeating forms) report their
  extended-`M1` internal T-state before the access through the same
  `Bus::internal_cycles` hook. The sweep now includes every `IN`/`OUT` form
  (disabling `contend_io` makes 96 timings fail); only `HALT` and interrupt
  acceptance remain outside the comparison.
  **This did not fix Arkanoid.** `crates/machines/cpc/tests/tape_load_probe.rs`
  (ignored; `TAPE=<path.cdt> MAX_CYCLES=n cargo test -p cpc --test
  tape_load_probe -- --ignored --nocapture`, `OUT_PPM=<file>` saves the final
  frame) types `RUN"` on the real CPC464 firmware, plays a tape in real time and
  prints PC/tape position. Before and after the timing fix `software/cpc/
  Arkanoid.cdt` behaves the same -- and still does after the I/O alignment fix
  below: the tape reaches event 45541 at ~140M cycles, the loader runs, and the
  machine ends at the BASIC banner. An
  experiment (reverted, not committed) adding 0-4 T-states to IM1 interrupt
  acceptance showed only two distinct outcomes (4 NOPs vs 5 NOPs, the
  quantization absorbs 1-2 T-states) and both end at the BASIC banner too, so
  the interrupt-acknowledge cost is not the cause. **Open question the
  references disagree on:** IM1 acceptance is 4 NOPs here and in CPCEC's source
  comment (13 T/4 NOP, "equals LD A,(nn)"), but ep128emu's model works out to
  about 5; our core also doesn't place the acknowledge cycle's 7 T-states in
  place before the pushes (harmless from an aligned start, where it still comes
  to 4 NOPs). Per `TODO.md`'s own rule for disagreeing references this needs a
  hardware source or a behavioural test that distinguishes them -- Arkanoid
  could not (the 4-vs-5 NOP question stays open; Arkanoid's real causes turned
  out to be elsewhere -- see "Cassette loading: motor, pause edge, leader").
  Also found and fixed on the way: the `z80` core **panicked** on 196
  undefined `ED xx` opcodes. On real silicon those are 8-T-state NOPs, and
  `ED 4C/54/5C/64/6C/74/7C` (NEG), `ED 55/5D/65/6D/75/7D` (RETN) and
  `ED 4E/66/6E` (IM 0), `ED 76` (IM 1), `ED 7E` (IM 2) are real undocumented
  mirrors (Sean Young, *The Undocumented Z80 Documented*) that software can
  execute; the ED table now defaults to `opcodes::ed::undefined_ed` (8 T) with
  those mirrors wired to their documented twins (tests compare each mirror's
  result/flags/timing against the documented opcode). ZEXALL/ZEXDOC were not
  re-run -- the change only fills entries that previously panicked.
  **Held `/INT`, EI delay and the refresh register (2026-10-01)** -- three
  real core/bus gaps found by diffing against CPCEC's `irq_delay`/`irq_steps`/
  `z80_irq_ack`, Caprice32's `z80_int_handler` and ep128emu's `executeInterrupt`
  (all three agree, see `CREDITS.md`), then by tracing why a self-saved fast
  tape could not be read back. (1) The Gate Array's interrupt request is a
  **held level**: `GateArray::interrupt_line` is raised when the counter fires
  (or a PRI matches) and stays up until the CPU acknowledges it
  (`Bus::interrupt_acknowledged`, a new defaulted `machine::Bus` hook the `z80`
  core calls from `accept_interrupt` in every interrupt mode; `CpcBus` drops the
  line and **clears bit 5 of the Gate Array's scanline counter**, as all three
  references do) or an `RMR` write with bit 4 set clears it (and the counter).
  `Cpc::step` samples the line at the start of every instruction. Before this a
  request raised while the CPU had interrupts disabled was silently dropped,
  which no reference does. Mutation checks: with the acknowledge hook disabled
  the real-firmware boot-banner test fails (the line never drops) and so do the
  new GA/bus tests. (2) The Z80 core had **no `EI` delay**: an interrupt could be
  taken right after `EI`, before the next instruction (`EI; RET`); `Cpu::ei_delay`
  now holds a waiting maskable interrupt off for exactly one instruction (an NMI
  is never delayed) and keeps the request pending rather than dropping it.
  (3) The core **never incremented `R`**: its low 7 bits now count M1 cycles
  (every opcode/prefix fetch -- `DD CB d op` is two, `DD DD op` three -- the
  interrupt/NMI acknowledge and each re-executed HALT NOP), bit 7 only changes
  through `LD R,A`. `Cpu::step` gained the increments, tests cover each case,
  and ZEXALL/ZEXDOC-style regression was re-run after the change (the full
  `zexall` example: every group OK). The `.state` format changed (new `Cpu`
  field `ei_delay`, new `GateArray::interrupt_line`) -- internal-only, old
  `.state` files no longer load. **Effect on real tapes**
  (`tests/tape_load_probe.rs`, real CPC464 firmware, 2.0-2.5G cycles each):
  before, `Joe Blade` reset to BASIC after its first blocks and `Jet Set Willy`
  reset ~240M cycles in; now `Joe Blade` reaches its title screen, `Jet Set
  Willy` its "Loading .Please Wait..." screen (still loading when the run
  ended), `Bomb Jack` its title screen and `Oh Mummy` its game (it also
  needed the TZX informational-block fix to parse at all); `Manic Miner`
  still loads and runs its attract demo. The held-`/INT` change alone did not move any
  of them (a separate run with only it left Joe Blade resetting); the R
  counter is what the SpeedLock-style loaders and the firmware's tape loop
  (`LD A,R` at ROM `0x29D4`) depend on. **Then failing:** `Arkanoid`, `Killapede`, `Dan Dare` and `Joe Blade 2` --
all four were fixed by the cassette changes in the next paragraph (a missing
leader, not emulation timing). Self-saved tapes written at the
  firmware's fast speeds fail to read back for the same pilot-length reason
  (a 1000-byte file at `SPEED WRITE 1`), so the round-trip test stays at the
  default speed. The R change also exposed a **real `write_voc` bug**: the VOC
  header stores its sample rate as an 8-bit divisor (`1e6/(256-d)`, whole-
  microsecond periods, so 44100 Hz really is 45454 Hz) but the samples were
  generated at the requested rate, so every pulse read back ~3% short (4% at
  96 kHz) -- enough to pull a recorded tape's pilot tone under the firmware's
  2.3 s motor delay and make `tape_save`'s VOC round trip hang. The writer now
  generates samples at the rate the header encodes (regression test
  `write_voc_round_trip_preserves_total_time_at_common_rates`, mutation-checked).
  **CPC Plus/GX4000 ASIC**: `cpc::Model` adds `Cpc464Plus`/`Cpc6128Plus`/
  `Gx4000`. All three share the real CPC+ ASIC (new `crates/machines/cpc/
  src/asic.rs`), gated behind the real 15-byte unlock sequence (fed by
  every CRTC select-register write) and RMR2 (a Gate Array function-2
  write with bit 5 set, disambiguated from the classic MRER at the same
  port -- `Asic::maybe_write_rmr2`). Register offsets, the unlock
  sequence, and every quirk are sourced from CPCTech's own "Extra CPC
  Plus Hardware Information" (hardware-tested against a real CPC6128+)
  cross-checked against MAME's own tested `amstrad_m.cpp` (see
  `CREDITS.md`). Memory-mapped ASIC register window (`&4000-&7FFF`,
  visible only once unlocked and `rmr2 & 0x18 == 0x18`): 16 hardware
  sprites (16x16, 1x/2x/4x magnification), a 32-entry 4096-colour
  (12-bit RGB) palette (screen pens 0-15 + border at entries 0-16,
  sprite colours 1-15 reusing entries 17-31), PRI/split/soft-scroll/
  interrupt-vector/DMA registers, and 2 analogue-joystick ADC axis
  pairs. RMR2 lower-ROM relocation repositions a cartridge bank
  (`rmr2 & 0x07`) to `&0000`/`&4000`/`&8000`; the classic upper-ROM-
  select port is reused unchanged for cartridge banks too. GX4000 has
  no internal firmware at all (real hardware fact) -- boots solely from
  a mandatory cartridge image (the same "rom" blob every model already
  uses), 1-8 real 16K banks, validated by `Cpc::new`
  (`MAX_CARTRIDGE_BANKS`); 464Plus/6128Plus keep their own classic-
  shaped internal firmware unchanged, RMR2 relocation present but inert
  without a separately attached cartridge (not wired to any attach path
  this pass). 6128Plus keeps the real built-in floppy its non-Plus
  counterpart has (`Model::has_128k_paging`/`has_fdc` both extended);
  GX4000 has neither a floppy nor a cassette port and rejects tape media
  with a clear plugin-level error.

  **Cassette loading: motor, pause edge, leader (2026-10-01)** -- the three
  real causes behind `Arkanoid.cdt` (and `Killapede`/`Dan Dare`/`Joe Blade 2`)
  not loading, found by tracing the actual firmware/stub execution rather than
  tuning timing; each is backed by two reference emulators' sources (see
  `CREDITS.md`'s "CPC tape loading" section). (1) **Port C data writes drive
  the cassette motor** (`CpcBus::write_port_c`): the relay hangs off PC4, so
  `OUT (C),A` to `0xF6xx` with bit 4 set starts the tape exactly like a
  bit-set/reset command (Caprice32 `z80_OUT_handler`, CPCEC `tape_enabled`).
  It used to be ignored, so a SpeedLock stage that starts the tape that way
  sat forever on a stopped tape. (2) **A TZX pause opens with a 1 ms
  terminating edge** (`zx_tape::push_pause`, Caprice32 `tape.cpp`, CPCEC
  `tape_tails`): silence is low, and when the last data pulse was low there is
  no edge to end it, so a loader measuring that pulse timed out -- three of
  Arkanoid's five SpeedLock blocks end on a low pulse, and the loader failed
  their last byte and wiped memory. This changes every pause-bearing block
  (`.tap` too, so the Spectrum's pulse streams gained one 1 ms pulse where the
  last data pulse was low); the Spectrum/CPC tests were updated and the real
  CPC tapes re-run. (3) **A 2000 ms leading silence** on every attached tape
  (`zx_tape::Tape::with_leading_silence`, `cpc::DEFAULT_TAPE_LEADER_MS`,
  `Cpc::set_tape_leader_ms`, plugin option `tape_leader` / `--tape-leader <ms>`,
  `0` plays the image as stored): the firmware waits 2.39 s after motor-on
  before sampling (`ROM 0x2A72`, measured), Arkanoid's first pilot is 2.73 s,
  so only 232 pilot pulses were left and the header was missed -- the firmware
  then took the *second* file (`!`, a SpeedLock stage that needs the first
  file's setup) as the program, which is why it looped forever on a NOP sled
  (CPCEC's `tape_safetydelay()` opens every TZX/CDT with the same 2000 ms).
  `Killapede`/`Dan Dare`/`Joe Blade 2` had the identical leader problem.
  Regression tests: `cpc::bus` (`ppi_port_c_data_writes_drive_the_cassette_motor_from_bit_4`,
  `a_port_c_data_write_resumes_a_tape_the_motor_off_had_paused`,
  `load_tape_prepends_the_default_leader_and_zero_disables_it`), `zx-tape`
  (`a_pause_after_a_low_last_pulse_opens_with_a_one_ms_terminating_edge` and
  friends, `with_leading_silence` tests) and the real-firmware
  `tests/tape_save.rs::a_cdt_that_starts_on_a_short_pilot_needs_the_leading_silence_to_load`
  (a pilot-first CDT trimmed from a firmware SAVE: missed without a leader,
  loads with it; mutation-checked). **Result** (`tests/tape_load_probe.rs`,
  real CPC464 firmware, 1.8G cycles each): `Arkanoid` loads its stage-1
  program, the SpeedLock stub and `SpeedLock 1 Block 1`-`3` (tape position
  812856 of 990832) and runs its game code to the title/key-select loop (its
  title screen is shown with the frame-splitting fix described under "CRTC
  types and per-character video" -- before it, the picture was visible one
  frame in ~90); `Bomb Jack`, `Dan Dare`, `Jet Set Willy`, `Joe Blade`,
  `Joe Blade 2`, `Killapede`, `Manic Miner` and `Oh Mummy` all load to their
  title screen or running game.

  Vectored Z80 IM2 interrupts needed a real core change: `machine::Bus`
  gained `interrupt_vector_byte` (defaulted `0xFF`, unchanged for every
  other machine here), called from `z80::Cpu::accept_interrupt`'s own
  IM2 branch instead of the old hardcoded floating-bus assumption. The
  ASIC supplies the real, cause-dependent byte (raster = vector `6`, DMA
  channel N = vector `N*2`, matching MAME's own tested assignment,
  verified against a real game, Pang) once unlocked, `0x00` while locked
  (a real, CPC-Plus-specific quirk, distinct from classic CPC's
  floating-bus `0xFF`). The documented hardware bug where a DMA-caused
  interrupt's `/INT` line stays asserted until software explicitly
  acknowledges it via DCSR (`&6C0F`) is reproduced faithfully, not
  "fixed" -- `Asic::irq_unacknowledged` tracks it, and `Cpc::step`
  re-requests the CPU interrupt every instruction while it's set,
  matching real level-triggered `/INT` sampling without needing any
  further z80-core change (an earlier design draft added a second Bus
  hook, `interrupt_line_held`, for this -- reverted once tracing through
  `z80::Cpu::step`'s own edge-triggered `irq_pending` consumption showed
  it would immediately drop the re-armed flag on the very next
  instruction if `iff1` was still false, which it always is right after
  acceptance; re-requesting every `Cpc::step` call achieves the same
  real effect with no core change at all).

  Programmable Raster Interrupt compares against this crate's own real,
  cycle-driven CRTC line/raster counters (`Crtc::vertical_counter`/
  `raster_counter`, new public(crate) getters) each falling HSYNC --
  more precise than MAME's own vpos-approximation, since this CRTC
  already tracks real per-character position rather than a per-hsync
  scanline count. Classic CPC-style interrupts and PRI cannot be active
  at the same time (real hardware fact) -- `GateArray::hsync_falling_edge`
  keeps running unconditionally (its own counter bookkeeping never
  stops), only the resulting interrupt *assertion* is suppressed while
  PRI is active; a firing PRI also resets the classic counter's top bit
  (`GateArray::reset_top_bit`), matching the real interaction the two
  mechanisms have on actual hardware.

  3 DMA sound channels: a real opcode engine (`Asic::step_dma_channel`)
  implementing LOAD/PAUSE/REPEAT/LOOP/INT/STOP, executing one
  instruction per enabled channel per HSYNC from primary RAM (same
  "CRTC always reads primary RAM" convention this crate's video already
  established) -- verified end to end through a real `Cpc::step` loop
  landing a value in the AY-3-8910 without disturbing the CPU's own
  currently-selected PSG register (`CpcBus::step_asic_dma` saves/
  restores `psg_selected_register` around the DMA-driven write).

  Video: the Plus picture is produced by the same per-character trace as
  the classic machines (`crate::raster`, see below) -- the 4096-colour
  palette is traced as `Ink` changes (ASIC palette writes at `&6400-&6421`
  resolved to RGB at write time), the soft-scroll register as an `HScroll`
  change plus a vertical raster offset applied when the video bytes are
  fetched (`raster::fetch_video_bytes`: `RA + scroll`, spilling into the next
  character row past `R9`), and the screen split as a CRTC-level row-address
  override (`Crtc::set_split`, address in `R12/R13`'s 14-bit `MA` format --
  Caprice32's reading of `SSA`, replacing this crate's earlier byte-address
  reading). Only the 16 hardware sprites remain a post-composite
  (`plus_video::composite_sprites`, from the ASIC's live sprite registers),
  positioned relative to the display area's origin in the window, in the
  ASIC's fixed Mode-2-pixel dot-clock units. Not traced: the "extend
  border" bit of the scroll register, and a locked/unlocked Plus still takes
  its screen colours only from the ASIC palette (classic Gate Array ink
  writes aren't routed to it, unchanged from before).

  Analogue joystick: 2 real ADC axis pairs (`Cpc::set_analog_axis`,
  `AnalogJoystick`/`AnalogAxis`), digital-direction-driven --
  `ANALOG_CENTER` (`0x1F`) at rest is a deliberate engineering default
  (a real, usable "virtually attached, centred" joystick), not CPCTech's
  own measured "nothing attached" reading of `0x3F` (same epistemic tier
  as `zx_spectrum::SpeakerType`'s filter cutoffs). Reachable through the
  plugin's existing `KeyEvent` dispatch via new stable key names
  (`AnalogJoy1Left/Right/Up/Down`/`AnalogJoy2*`), same "arbitrary string
  passes through the existing `[keys]` config" mechanism every other key
  already uses -- no GUI-side default binding added, opt-in via
  `[machines.*.keys]` same as ZX Spectrum's own Symbol-Shift combos.
  The classic digital joystick port (keyboard matrix line 9,
  `CpcKey::Joy1*`) keeps working unchanged on every Plus/GX4000 model
  too, real hardware fact -- most real GX4000 games use it, not the
  analogue port.

  Save states round-trip full ASIC state (`Asic` derives serde directly,
  same "no external media held" reasoning as `ay_3_8910::Ay3_8910`); the
  cartridge image itself stays external, same `--rom` convention as
  every other machine here. `--machine cpc464plus|cpc6128plus|gx4000`
  boot to a real screen through the actual `retroclawd` binary
  (`--headless --dump-screen`-verified, not just unit-tested): a
  synthetic Z80 test program ran the real 15-byte unlock sequence, an
  RMR2 write making the ASIC register window visible, and a PRI-register
  write, confirmed by reading the same address back via `--dump-memory`
  through the real, running machine.

  **CPC parity formats and Mode 3**: new format-only crates
  `cpc-cartridge` and `cpc-snapshot` cover CPC Plus/GX4000 RIFF `AMS!`
  `.cpr` containers and CPC `MV - SNA` v1/v2/v3 snapshots. `.cpr` chunk
  IDs (`cb00`-`cb31`) stay authoritative, including sparse pages; GX4000
  creation accepts both raw flat banks and `.cpr`, while game-config
  `media = "*.cpr"` promotes the file to the creation `rom` blob. CPC
  plugin commands load/save `.sna`, with explicit machine/RAM mismatch
  errors. Public SNA has no CPC+ ASIC fields, so ASIC state stays in the
  project's internal `.state` format instead of an invented extension.
  The picture path renders undocumented Mode 3 (`raster::decode_cell`): two
  Mode-1 2-bit pixels per byte, four Mode-2 dots each. Reference behavior: Caprice32 accepts Mode 3 and
  ZEsarUX's `src/machines/cpc.c` renders the same two-pixel expansion (see
  `CREDITS.md`). Remaining gaps after the first pass were RAM expansion
  above 128K, IPF/CT-RAW disks, FDC timing, VOC/tape output, peripherals,
  ZRCP compatibility, and deliberately parked revision-specific IN-as-OUT/
  cut-down-8255 quirks.

  **Second parity pass (RAM/ROM expansion, FDC timing, VOC tape, ZRCP)**:
  `CpcBus` implements the real Standard Memory Expansion register-3
  mechanism (bits 5:3 select a 64K bank 0-7, bits 2:0 keep the existing
  8-configuration paging table) -- `Cpc::set_ram_expansion_banks`/
  `--cpc-ram-banks` (6128/6128Plus only, matching real hardware wiring)
  grows RAM up to the real 576K maximum (base 64K + 8x64K banks). Upper-
  ROM slots 1-6/8-15 load real expansion ROM images via `Cpc::
  load_expansion_rom`/`--cpc-rom-slot N=path` (repeatable) or `rom_slotN`
  blob keys; slot 0 (BASIC)/slot 7 (AMSDOS) stay reserved. Both round-trip
  through save state.

  `nec765::Fdc` gained opt-in (`enable_seek_timing`, default off) real
  80 T-states/track Seek/Recalibrate timing (ZEsarUX-sourced), gating
  Sense Interrupt Status's ST0 Seek End bit behind a `tick`-driven
  countdown -- opt-in specifically so `zx_spectrum`'s own +3 FDC usage
  (which never calls `tick`) keeps its original instant-completion
  behavior unchanged; only `cpc::bus::CpcBus` enables it, ticking every
  `Cpc::step`. `dsk::Sector` now preserves every real stored weak-sector
  copy (`copies`, not just the first), and `nec765::DiskMedium::
  read_sector` gained an `attempt` parameter so `Fdc::weak_read_counter`
  (a real, deliberately simplified single rolling counter, not Caprice32's
  own per-`(cylinder,head,sector)`-keyed one) rotates through them on
  successive Read Data executions -- both `cpc`'s and `zx_spectrum`'s own
  `DiskMedium` wrappers implement this. IPF/CT-RAW remain deferred (full
  MFM/CAPS decode or a licensed external dependency, out of scope here).

  **FDC rewrite and disk write path (2026-09-29)**: a real-firmware check
  found the previous `nec765` could never have run a real AMSDOS/+3DOS
  disk. It matched only bare opcodes (`0x06`, where firmware sends
  `0x46` -- the `MF` bit), had no `EXM`/`CB` bits in the main status
  register (firmware polls bit 5 to tell the execution phase from the
  result phase), reported ST0 `0x00` on success (real CPC/+3 hardware has
  no terminal-count line, so a successful read ends "abnormal termination"
  ST0 `0x40` + "end of cylinder" ST1 `0x80`), and had no Sense Drive
  Status, so it only passed hand-written unit tests. `nec765` is now a
  proper uPD765A, following the datasheet and Caprice32's `fdc.cpp` where
  real software depends on a detail the datasheet leaves open (see
  `CREDITS.md`): `opcode & 0x1F` selects the command with `MT`/`MF`/`SK`
  decoded; MSR is idle `0x80`, receiving parameters `0x90`, execution-
  read `0xF0`, execution-write `0xB0`, result `0xD0` (one not-ready `0x10`
  poll after an execution phase starts, like Caprice32); Read Data/
  Read Deleted/Read Track/Write Data/Write Deleted/Format Track/Read ID/
  Sense Drive Status/Seek/Recalibrate/Sense Interrupt Status/Specify are
  implemented, sectors chain up to `EOT`, deleted marks and `SK` follow
  Caprice32's `LOAD_RESULT_WITH_STATUS` termination rules, and sectors
  are found by matching all four ID bytes (`CHRN`) on the *physical*
  cylinder the head was seeked to (not the command's `C`). `DiskMedium`
  is now index-based (`sector_count`/`sector_info`/`sector_data` plus
  `write_sector`/`format_track`/`write_protected`; the write hooks default
  to read-only), and `impl DiskMedium for dsk::Disk` lives once in
  `nec765` behind the opt-in `dsk` feature (both CPC and the +3 host used
  to need their own wrapper) so hosts hold an `Fdc<dsk::Disk>` directly.
  Hosts mirror their motor latch with `Fdc::set_motor` -- a drive is only
  ready while its motor spins. `dsk::Disk` gained `write_sector_at`/
  `format_track`/`new` and a lossless `to_bytes` (Extended DSK is chosen
  automatically when the model can't be standard: weak-sector copies or
  odd sector lengths), keeps each sector's ST1/ST2 and the track header
  fields, and clamps hostile size codes. Proven end to end against the
  **real CPC6128 firmware** (`crates/machines/cpc/tests/amsdos_disk.rs`,
  skipped if the ROM file is absent): a hand-assembled program calls the
  firmware's own `CAS OUT OPEN/DIRECT/CLOSE` then `CAS IN OPEN/DIRECT/
  CLOSE`, so the real AMSDOS writes a 3000-byte file to a freshly
  formatted disk and reads it back through the emulated controller; the
  test then checks the read-back bytes, that the medium is marked
  modified, and that the serialised `.dsk` parses with `TEST.BIN` in its
  directory (a mutation check -- making the medium write-protected -- was
  confirmed to fail it). Save states carry the disk only once it has been
  written (`modified_disk`, a serialised `.dsk`), and restoring a state
  taken before any write puts back the disk as originally inserted (each
  host keeps a `pristine_disk` copy) -- so an unwritten disk still adds
  nothing to a state, while reverse-stepping/`--load-state` stay
  consistent with the medium. Persistence is opt-in and never silent:
  guest writes always land in the emulated medium, and reach the host
  filesystem only via `--disk-save <path>` (write the modified image
  there, leaving `--disk` untouched) or `--disk-writeback` (atomically
  overwrite the `--disk` file); with neither, a modified disk is
  discarded with a warning on stderr, and an untouched disk says nothing.
  The write happens in `HostedMachine`'s `Drop` (end of a headless run, or
  when the GUI's emulation thread ends) via the new `MachineCommand::
  ExportDisk`/`MarkDiskSaved` and `MachineResponse::DiskImage` (protocol
  version 3); run-ahead clones never persist. Verified live through the
  real binary: a hand-assembled Z80 program writing a sector through the
  FDC ports produced a modified `--disk-save` file (exactly the 512 written
  bytes, original byte-identical), `--disk-writeback` rewrote the source
  atomically with no temp file left behind, the default path warned, and
  a run that never wrote produced no file and no message. Deliberate
  simplifications, all documented in `nec765`'s module doc: execution is
  instantaneous (no rotational latency, no overrun -- a byte the CPU is
  slow to fetch is never lost), `MT` is accepted but not acted on, the
  Scan commands answer "invalid command", only drive 0 exists (another
  unit finds it not ready), and the seek-busy `DnB` bits aren't modelled.
  Old `.state` files from before this change no longer load (the FDC
  state layout changed; the format was always internal-only).

  **CRTC types and per-character video (2026-09-29)**: the static
  "decode the registers as they stand when `render_frame` is called" video
  was replaced, because it could never show raster bars, split-mode screens,
  overscan or hardware scrolling. Two pieces:

  *CRTC types* (`crtc::CrtcType`, `--crtc-type 0-4`, `Cpc::set_crtc_type`,
  `Model::default_crtc_type`: 464/664 = 0, 6128 = 1, Plus family = 3 --
  konCePCja's table, a convention since real machines varied): register
  write masks (`R3` high nibble and `R8` skew field exist on types 0/3/4
  only), register read-back (type 0 reads `R12-R17`, type 1 `R14-R17` plus
  `R31`, type 2 only `R16/R17`, types 3/4 a 3-bit register window with the
  two ASIC status registers), the `&BExx` status port (only type 1 has one;
  types 0/2 float high, 3/4 mirror the read port), `R3` sync-width rules
  (`R3l = 0` is no HSYNC on types 0/1, 16 characters on 2/3/4; VSYNC width
  programmable on 0/3/4, fixed 16 on 1/2), display skew, the UM6845R
  re-reading `R12/R13` every line while `C4 = 0`, and the MC6845 taking its
  frame start from `VMA'`. The type is stored in `Crtc` (so it round-trips
  save states) and in the SNA v3 header's `crtc_type` byte (a snapshot can't
  put a Plus on a discrete 6845). Sources and limits (interlace, cursor,
  light pen and sub-character effects are not modelled) are in `crtc.rs`'s
  module doc and `CREDITS.md`. `Crtc::tick_with` now also reports a
  `CharClock` per 1 MHz character (`MA`, `RA`, `DISPEN` with skew and `R6`,
  HSYNC/VSYNC), and the address generator (`VMA`/`VMA'`) follows the 6845
  rules; `crtc::ram_address` is the real `MA/RA -> RAM` wiring
  (`MA0-9 -> A1-A10`, `MA12-13 -> A14-A15`, `RA0-2 -> A11-A13`).

  *Per-character trace* (`crate::raster`): `Cpc::step` ticks the CRTC in
  segments split at the T-states of the instruction's picture-affecting
  writes (`CpcBus::video_events`: Gate Array ink/mode, ASIC palette/scroll,
  CRTC register writes, each stamped with `instruction_advanced_tstates`,
  which the Z80 core advances *before* calling `write_io`, so the stamp is
  the end of the write's own I/O cycle). CRTC register writes still apply
  immediately (in-instruction reads see them) but the CRTC is rewound to a
  clone taken at the first write (`crtc_replay_base`) and re-ticked with them
  applied at their real character; writes made through the bus outside an
  instruction (debugger pokes, tests) are only flushed into the trace
  (`flush_out_of_step_video`). `FrameBuilder` records, per character clock,
  the two fetched video bytes plus display/HSYNC/VSYNC flags, and logs
  `Mode`/`Ink`/`HScroll` changes against the exact cell they take effect on
  (a mode request latches on the HSYNC falling edge, like the Gate Array).
  A `Frame` runs from one VSYNC start to the next (or 312 lines with no
  VSYNC -- the free-run cut applies only while no VSYNC is arriving: once one
  has begun the builder is `synced` and gives up only after
  `SYNCED_MAX_FRAME_LINES` = 624 lines; the first version cut every field at
  312 lines, which split Arkanoid's 316-line title field (`R4=38, R5=4`) into
  a 312-line frame plus a 4-line stub that became the "latest frame", so its
  title screen showed one frame in ~90 and the rest black);
  `Frame::render` replays it into a fixed **768x288-line window of
  Mode-2 pixels** (Mode 1 pixels are 2 wide, Mode 0 4) anchored on the sync
  pulses -- the window opens 14 characters after HSYNC begins and 28 lines
  after VSYNC begins, so `R2`/`R7` move the picture as on a monitor and the
  firmware layout shows the 40x25 display with 64 px / 44 lines of border
  (the standard 312-line field ends 4 lines short of the window, those rows
  show the border colour). The three window constants are this project's
  own engineering choices (see `CREDITS.md`). `Frame::render` is 1 scanline
  per row (the space CPC+ sprites live in); `Cpc::render_frame` then shows
  every scanline twice (`raster::double_scanlines`), so the presented
  picture is **768x576, exactly 4:3** (the square-pixel PAL raster) --
  fixed 2026-10-01 after the first 768x272 version made the Win11 window
  ~2.8:1 and stretched. Same size for every mode and model, replacing the
  old per-mode 160/320/640-wide output plus `BORDER_PX`. Until a machine has completed a
  field of its own (fresh, or just state/snapshot-loaded -- the trace isn't
  saved) `render_frame` runs a clone of the CRTC through two fields with the
  Gate Array state frozen (`raster::synthetic_frame`), so it always has a
  picture. Verified: an independent decode of screen RAM matches the
  rendered window pixel for pixel on the real CPC464/664/6128 boot screens
  (`bus::tests::real_firmware_boot_screen_matches_an_independent_decode_of_
  screen_ram`); write-timing tests pin an ink write and an `R1` write to the
  exact character they landed on (`an_ink_write_lands_on_the_character_
  clock_it_happened_on`, `a_crtc_register_write_takes_effect_on_the_
  character_it_landed_on`); and through the real binary a hand-assembled
  program changing the border ink every scanline produced a rainbow-bar
  picture with the display area untouched, and a probe program's register/
  status reads matched the modelled per-type behaviour under `--crtc-type
  0..4`. Accuracy tier, stated plainly: the Gate Array changes ink/mode at
  whole-character granularity (real silicon switches a fraction of a
  character in, chip- and mode-dependent), video RAM is read at the
  character clock but CPU writes are only ordered against it to instruction
  accuracy, and purely internal Z80 cycles inside an instruction can put a
  stamp a T-state or two off.

  `zx-tape` gained `parse_voc`/`write_voc` (real VOC block types
  0x00-0x09, 8-bit unsigned PCM only, Caprice32's own `VOC_THRESHOLD`
  128-level decode threshold), wired through CPC and ZX Spectrum plugin
  tape dispatch, `--tape`, and game-config media.

  **Cassette saving and boot proofs (2026-10-01)**: the CPC records what
  firmware writes to the cassette output. PPI Port C bit 5 (MIC) is
  sampled by `CpcBus::note_cassette_output` into
  `crate::tape_recorder::TapeRecorder`, gated by the motor bit (Port C bit
  4) exactly like the playback side; each level change is stamped with an
  absolute T-state (`cycles_total + instruction_advanced_tstates`, the
  same stamp the video trace uses, so it lands on the I/O cycle of the
  `OUT` itself) and converted from the 4 MHz CPU clock to the 3.5469 MHz
  tape clock `zx-tape` counts in. `Cpc::recorded_tape` turns the edges into
  a `zx_tape::Tape` (`Tape::from_pulses`; zero-length pulses are dropped);
  `zx_tape::write_tzx` writes it as `.cdt`/`.tzx` (`0x13` pulse-sequence
  blocks, up to 255 pulses each, long silences split out as `0x20`
  pauses with the line level re-aligned) and `write_voc` as `.voc`.
  `--tape-save <path>` (common flag, same policy as `--disk-save`) writes
  the recording when the machine is dropped (end of a headless run, GUI
  emulation thread exit) via `MachineCommand::ExportTape { format }` /
  `MachineResponse::TapeImage(Option<Vec<u8>>)` (protocol version 4,
  `HostedMachine::export_tape`/`finish_tape_on_exit`); a run that never
  toggled the output writes nothing and says so on stderr. Only the CPC
  plugin answers it -- ZX Spectrum/ZX81/C64 reply "unsupported" (no MIC
  recorder there yet). Verified two ways: `crates/machines/cpc/tests/
  tape_save.rs` has the **real CPC464 firmware** `CAS OUT OPEN/DIRECT/
  CLOSE` a 1000-byte file, writes the recording as CDT and VOC, and has a
  freshly booted machine `CAS IN` each back to identical bytes; and
  through the binary a patched SNA doing the same save produced a valid
  TZX (115 blocks, 29004 pulses) and VOC, while a snapshot that never
  touched the tape wrote no file. The boot proofs are ROM-gated tests in
  `cpc::bus` (`real_firmware_boots_to_each_models_own_banner` checks the
  464/664/6128 banner rows read back out of Mode 1 screen RAM;
  `plus_and_gx4000_firmware_smoke_when_roms_are_present` only runs when a
  Plus/GX4000 ROM is supplied, none is committed). The second digital
  joystick (`CpcKey::Joy2*`, matrix line 6) shares the `6 5 R T G F B`
  wires with the keyboard, so `KeyMatrix` tracks it as its own source and
  ORs it in on read.

  **AMX mouse (2026-10-01)**: `crate::amx_mouse::AmxMouse` is an opt-in
  (`--amx-mouse`, a `Bool` machine option; `Cpc::set_amx_mouse`) mouse on
  the joystick-0 port, i.e. matrix line 9, shared with digital joystick 1 --
  hence off by default. Host motion (`MachineCommand::MouseMotion`, the same
  `i8` deltas the Spectrum's Kempston mouse gets) is queued per axis and sent
  as one **time-based LOW pulse per mickey** on the direction bit (Up/Down/
  Left/Right = bits 0/1/2/3, `dy < 0` = up); buttons are Left = Fire2 (bit 4),
  Right = Fire1 (bit 5), Middle = Fire3 (bit 6), active low. `CpcBus::
  keyboard_line` ANDs the mouse's `line9_mask` into line 9 wherever the
  matrix is read (the AY register-14 latch, `Cpc::keyboard_row_state`), and
  `finish_instruction` ticks the pulse generators with the instruction's own
  T-states. **What is sourced and what isn't**: pulses with no hardware motion
  counters, the bit layout and AMX Art's 300 Hz polling come from CPCWiki's
  "AMX Mouse" page (egress-blocked here, so read via search-result quotes)
  and konCePCja's `amx_mouse.cpp`; the pulse width/spacing is this project's
  own choice (one 300 Hz polling interval, low for all but the last 5%, queue
  capped at 255 mickeys per axis, excess dropped) because no source gives
  it. konCePCja paces consumption by software instead (one mickey per line-9
  deselect/reselect); not copied. Pulses/buttons are transient input, not in
  save states. **Verified live** through the real GUI under Xvfb: a Z80 probe
  (patched into a boot SNA) polls line 9 through the real PPI/AY register-14
  path and counts falling edges per bit into memory; XTest-injected X mouse
  events (python-xlib; the window must be focused, there is no window
  manager) produced exactly the expected counts read back over the debugger's
  `/api/memory-bytes` -- 50 left, 30 up, 20 down, 187 right (that included a
  127-mickey clamped pointer-warp the script itself caused, then 60 real) and one edge each for
  the left/right/middle buttons -- and an identical run without `--amx-mouse`
  counted nothing. No real AMX-aware CPC software was run (none available). Printer, phazer/
  lightgun, Dandanator/Multiface, and Net4CPC remain deferred.

  `debug-zrcp-plugin` implements real `assemble` (single-instruction
  assemble-and-poke reusing the existing `z80::disassemble::assemble_one`
  assembler, now wired end-to-end through a new `DebuggerCommand::
  Assemble` host round trip that previously didn't exist -- the original
  `DebuggerHostCommand::Assemble` handler was a `StatusCode::Unsupported`
  stub), `extended-stack` (real SP-relative memory dump), and
  `get-paging-state` (any machine whose `"overlay"` chip has fields, via
  `GetOverlaySnapshot` -- see the debug-contract genericity paragraph
  below for why this is no longer `machine_id`-gated to CPC/Spectrum
  specifically). `write-port <port> <value>` (added 2026-10-01) performs a
  real `OUT (port),value` between instructions: `DebuggerHostCommand::
  WritePort` -> `DebuggerCommand::WritePort` (one-shot reply, like
  `Assemble`) -> `HostedMachine::write_port` -> `MachineCommand::WriteIo` ->
  each Z80 machine's own `write_port` (`Cpc`/`ZxSpectrum`/`Zx81`, which just
  drive the bus's own `write_io`, so the machine's real port decode and side
  effects apply -- a CPC's loose decode can reach several chips from one
  write, the ZX81's `OUT (0xFE)` starts its NMI generator). The C64 plugin
  answers a clear error (no I/O-port space). Ports/values accept the usual
  ZRCP number syntax and a value over 255 or a port over 65535 is rejected
  before reaching the host. Verified live over a real TCP ZRCP session
  against the GUI on a CPC6128: `write-port 0x7f00 0xc1` switched RAM
  configuration so the byte read back at `&4123` changed from block 3's to a
  value previously poked into physical block 1, and `0xc3` switched it back.
  `MachineCommand::WriteIo`/`DebuggerHostCommand::WritePort` are
  protocol version 5. The remaining ZEsarUX-specific/Spectrum-Next-only
  commands stay deferred.

  **I/O-port breakpoints (2026-10-01)**: pause the machine after an `IN`/
  `OUT` matching `port & mask == watched & mask` (mask `FFFF` = one exact
  port; a partial mask covers a loosely-decoded machine's mirrors, e.g. mask
  `C000` value `4000` is every CPC Gate Array address), IN, OUT or both.
  Layers: `machine::PortWatcher`/`PortWatch`/`PortHit` (latches the *first*
  matching access since it was last taken; with no watches set `note` is a
  single `is_empty` check) lives on each Z80 machine's bus
  (`Cpc`/`ZxSpectrum`/`Zx81`, never in save states); the one place that feeds
  it is the Z80 core's existing I/O funnel (`Cpu::io_read`/`io_write`), which
  now calls a new defaulted `machine::Bus::io_accessed(port, value, write)`
  after every `IN`/`OUT` -- so no bus has to wrap its early-returning port
  decode, the 6502 machines never call it, and debugger-initiated
  `write_port` calls (which go straight to the bus) can't trip a watch.
  `Machine::set_port_watches`/`take_port_hit` are defaulted (a machine
  without a port space says no) and reach the host as `MachineCommand::
  SetPortWatches`/`TakePortHit` (protocol v6, with `PortWatchSpec`/
  `PortHitInfo`); the C64 plugin answers a clear error. The host's
  `DebuggerLink` keeps the list (`DebuggerHostCommand::SetPortBreakpoint`/
  `ClearPortBreakpoints`, upsert keyed by `port`+`mask`, neither direction =
  remove; the machine is told first so a refusal leaves the host list
  truthful, and clearing an already-empty list always succeeds) and takes the
  hit after every instruction -- in running mode inside `on_post_step_running`,
  and in single-step/step-over via `step_once` -- pausing and publishing
  `DebuggerState::last_port_hit` (cleared on resume and on the next step,
  like `last_break`) plus a `last_event`. Surfaces: ZRCP `set-portbreakpoint
  <port> <type> [mask]` (type 0 remove/1 IN/2 OUT/3 both, mirroring the
  `set-membreakpoint` type numbers), `get-portbreakpoints`,
  `clear-portbreakpoints`, and `run` now reports `Port breakpoint fired: OUT
  (BC00H),00H` -- these are **retroclawd extensions, not ZEsarUX commands**
  (ZEsarUX's own `set-membreakpoint` here stays a session-local compatibility
  map that never actually stops the machine); the browser debugger's state
  JSON gains `port_breakpoints`/`last_port_hit`, with `POST /api/port-
  breakpoint/set` and `/clear-all` and a "Port breakpoints (IN / OUT)" panel
  (hex port/mask, IN/OUT checkboxes, list with remove buttons, the hit shown
  in the status bar). ZRCP only ever starts for Z80 machines (a deliberate,
  pre-existing gate), so on the C64 port breakpoints are reachable only through
  the HTTP API and are refused there. Two real bugs found by the live check and
  fixed with it: (1) ZRCP's `run` sent `Resume` and then read the *published*
  debugger state, which the host only refreshes on its next tick, so it often
  saw the old paused snapshot -- reporting a stale `Breakpoint fired`/`Port
  breakpoint fired` or an instant `Run stopped`; `run_outcome_ready` now treats
  a paused state identical to the pre-resume one (same `pc`/`last_break`/
  `last_port_hit`) as stale for up to 300 ms (unit-tested as a pure
  function); (2) the debugger page's right column let content-sized panels
  shrink and clip their own bodies once enough were stacked (the existing
  Breakpoints list included), fixed with `.col > .panel:not(.fill) {
  flex-shrink: 0 }` so the column scrolls instead. **Verified live** through
  the real GUI on a CPC464 running a Z80 probe loop (`OUT &7F10`, `OUT &BC00`,
  `IN &F400`) over a real ZRCP TCP session: an exact OUT watch stopped twice
  with PC right after the `OUT` and `OUT (BC00H),00H` reported, an exact IN
  watch stopped after passing the two OUTs (so direction filtering works) with
  `IN (F400H) -> 00H`, a masked watch (`4000`/`C000`, OUT) stopped on `OUT
  (7F10H),10H`; the HTTP state/endpoints and the real browser UI (Chromium via
  Playwright: add, masked add, bad-input message, hit in the status bar,
  remove, clear) behaved; and the C64 refused with HTTP 400 and kept running.
  Hits are detected after each instruction completes; the Z80 core runs a
  block `INIR`/`OTIR` one iteration per step (PC rewound), so every
  iteration's `IN`/`OUT` is checked and a hit stops at the first matching
  one.

  **Debug-contract genericity cleanup (2026-09-28)**: two real "the host
  hardcodes machine-specific knowledge instead of using the plugin
  contract generically" gaps, found and fixed together. First: CPC used
  to expose two separate `ChipSnapshot`s -- the shared, cross-machine
  `chip_id: "overlay"` (GUI `--show-overlay` panel data) and a second,
  CPC-only `"cpc.overlay"` (richer PPI/keyboard/PSG/tape/disk telemetry,
  feeding only `--debug`'s diagnostic log and ZRCP's `get-paging-state`)
  -- and the host's own reader for the second one
  (`cpc_overlay_debug_snapshot`, now deleted) required 5 specific field
  names to *all* be present, silently returning nothing at all if even
  one was missing or renamed, and was only ever called for a
  `machine_id` starting with `"cpc"` -- meaning `get-paging-state` always
  replied `"OK unavailable"` for ZX Spectrum 128K too, despite its own
  help text and its ZRCP-side `machine_id` gate both claiming it worked
  there (a real, live bug, not hypothetical -- `zx-spectrum-machine-
  plugin` never emitted a `"cpc.overlay"`-chip_id snapshot at all).
  Fixed by merging CPC's richer fields directly into its own `"overlay"`
  chip (`cpc-machine-plugin::overlay_chip_snapshot`, one chip now, not
  two) and replacing every host-side consumer (`plugin_bridge.rs`'s
  `overlay_chip_snapshot`/`HostedMachine::overlay_diagnostic_snapshot`,
  `emu_thread.rs`'s diagnostic log, `debugger_plugin_bridge.rs`'s
  `GetOverlaySnapshot` handler, `debug-zrcp-plugin`'s `get-paging-state`/
  `get-io-ports`/`get-cpc-io`) with a generic reader that formats
  whatever `NamedValue` fields are actually present (`key=value` pairs)
  instead of requiring specific field names -- `get-paging-state` now
  correctly returns real paging text for ZX Spectrum 128K/+2A/+3 too,
  and any future machine that adds overlay fields needs zero host
  changes to have them show up in both the diagnostic log and ZRCP.
  Second: the debugger's `PokeBank` command (banked/paged memory poke --
  the `.POK` trainer importer's own primary use, see
  `debug-ui-plugin`'s `/api/pok/apply`) was pre-rejected in
  `debugger_plugin_bridge.rs` for any `machine_id` not starting with
  `"zx-spectrum"`, *before* ever asking the machine plugin -- meaning a
  future machine that implemented real banked-write support (e.g. CPC's
  own 128K/expansion RAM) would still have been refused at the host
  layer. The three other machine plugins' own `WriteMemory` handlers
  made this gate load-bearing in a way that wasn't obvious: they matched
  `WriteMemory { address, bytes, .. }`, silently discarding a non-`None`
  `space` and writing flat memory anyway -- removing the host gate alone
  would have made a banked poke on C64/ZX81/CPC *silently write to the
  wrong place* instead of erroring. Fixed both ends together: `c64`/
  `zx81`/`cpc`-machine-plugin's `WriteMemory` handlers now explicitly
  reject a `Some(space)` request with a clear error instead of silently
  ignoring it, and `debugger_plugin_bridge.rs`'s `PokeBank` handler lost
  its `machine_id` pre-check entirely -- the machine plugin's own answer
  is now the sole authority. This also needed `PokeBank` to become a
  real request/reply (a new `Sender<Result<(), String>>` reply channel on
  `DebuggerCommand::PokeBank`, mirroring `Assemble`'s existing shape) --
  it was previously fire-and-forget, meaning `.POK` import would have
  reported false success (`Ack` immediately, the real error only ever
  reaching `DebuggerState.last_event` on a later poll) for any rejected
  banked poke once the host-side pre-check was removed.

  **CPC banked RAM access (2026-10-01)**: the CPC plugin no longer rejects a
  `space` -- it now implements `ram-bank:N`, the same name the ZX Spectrum
  plugin uses, for `MachineCommand`/`DebugCommand` `ReadMemory`/`WriteMemory`
  alike. `N` is a **physical 16K RAM block** (`Cpc::ram_block_count`/
  `read_ram_block`/`write_ram_block`: 0-3 the base 64K, 4-7 a 6128's second
  64K, 8+ the Standard Memory Expansion banks, up to 36 blocks) and the
  address is an offset within it (`0..=0x3FFF`), independent of whatever the
  Gate Array has paged in. A `space` that isn't `ram-bank:N`, a block the
  machine doesn't have, or an offset past the block is a clear error, never
  a silent flat access. `GetAddressSpaces` lists `cpu` plus a 14-bit
  `ram-bank` space with a `blocks` count. `.POK` import reaches it through
  the existing `PokeBank` path, but a `.POK` bank field's bit 3 means
  "ignore bank", so only blocks 0-7 are reachable that way; expansion blocks
  8+ need `MachineCommand::WriteMemory` directly. Verified live through the
  real GUI on a CPC6128 (patched snapshot parked in RAM configuration 3,
  where `&4000` shows block 3): a two-entry `.POK` to bank 3 changed the
  bytes read back at `&4123` over `/api/memory-bytes`, `&C123` (block 7 in
  that configuration) stayed untouched, a bank 5 poke succeeded without
  disturbing the visible window, and an offset past the block was rejected.

  Two real bugs were found and fixed while building this pass, each with
  its own new regression test: a double-division error in the VOC
  pulse-cycle conversion (produced near-zero pulse durations for every
  decoded sample run) and a missing `data_offset` advance in `dsk::
  parse_track` (every sector after a track's first would have silently
  read back the first sector's own bytes).
