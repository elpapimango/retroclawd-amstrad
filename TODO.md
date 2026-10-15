# TODO / Roadmap

Backlog for the Amstrad CPC family, moved from the host repository's TODO.md. Host-wide items (plugin architecture, input, shaders, parked machines) stay in the host.

## Amstrad CPC roadmap

### DONE: baseline

CPC 464, 664, and 6128 boot their real combined firmware ROMs to model-specific `Ready` screens.
Implemented hardware includes:

- 64K RAM on 464/664; 128K and all eight documented paging configurations on 6128.
- BASIC in upper-ROM slot 0; AMSDOS in slot 7 on 664/6128; unpopulated slots fall back to BASIC.
- Reused `nec765`/`dsk` FDC path on 664/6128 with real CPC motor/MSR/data port decode.
- Gate Array palette, ROM control, HSYNC/VSYNC-frame-locked interrupt counter, and Modes 0/1/2.
- Register-driven CRTC timing and one-in-four Gate Array/Z80 memory arbitration.
- i8255/AY keyboard and audio, GUI video, reset, and model-checked save states including paging/FDC phase.

### DONE: timing accuracy and standard video modes

CRTC R0/R2-R5/R7/R9 now drive HSYNC, VSYNC, raster position, vertical adjust, and frame length at
the real CPU-clock/4 character rate. Falling HSYNC edges drive the Gate Array's 52-line interrupt
counter; rising VSYNC applies its real two-HSYNC frame-lock reset/early-interrupt rule and is visible
through PPI Port B bit 0. CPU memory accesses use `Bus::contend_memory`/`advance_tstates` to wait for
one of every four Gate Array slots; internal instruction cycles reconcile the phase afterward.

Modes 0, 1, and 2 decode their real bit-interleaved layouts. (This section describes the first,
static-decode design at 160x200/320x200/640x200; it has since been replaced by the per-character
trace and fixed 768x288-line (presented 768x576) monitor window -- see "BACKLOG: finish CPC" items 2 and 3.) Focused tests cover discriminating bit patterns, and actual headless Z80 programs rendered Mode 0
and Mode 2 patterns through `retroclawd`; a live Xvfb/llvmpipe GUI run confirmed a Mode 2 switch
resizes the Bevy texture, quad, shader dimensions, and aspect basis instead of dropping the frame.
The real CPC 464 firmware still boots to `Ready` after the timing changes. Mode 3 is undocumented
and remains **PARKED** until a concrete compatibility case or reliable source justifies it.

Keep the CRTC and Gate Array inside the CPC machine crate until a second machine needs either one.

### DONE: cassette input (`.cdt`/`.tzx`)

The host plugin bridge accepts `.cdt` as the TZX-compatible container CPC tooling uses and installs it
on the selected CPC plugin instance. CPC bus wiring models the cassette motor and live tape input;
controls the cassette motor, Port B bit 7 exposes live tape input, and playback advances at machine
step granularity only while the motor is on. Motor-off pauses transport immediately and returns EAR
idle-high; motor-on resumes from the same position. Save states preserve cassette motor state plus
tape playback position (tape bytes stay external, same convention as ROM/disk). MIC/output recording
remains a separate tape-saving task.

### DONE: CPC 664 and 6128

`cpc::Model` selects CPC464/CPC664/CPC6128 inside one machine crate. The 664 adds the built-in
uPD765A-compatible FDC and 48K combined OS/BASIC/AMSDOS ROM image. The 6128 adds the same disk
hardware plus 128K RAM and the exact eight-configuration paging table. `--disk`/game-config media
reuse the existing standard/Extended DSK parser and NEC765 protocol implementation; CPC-specific
wiring is limited to its broad hardware port decode and disk-motor latch. Focused tests read a
parsed sector through the CPC FDC ports and round-trip paging/FDC phase through save state. Real
664 and 6128 ROMs were booted headlessly to their `64K Microcomputer (v2)` and
`128K Microcomputer (v3)` `Ready` screens.

Decision: CPC remains PAL-only until a real supported model requires another timing standard. Do
not add a speculative region abstraction.

### DONE: Plus/GX4000 ASIC (external parity steps 1-3)

`cpc::Model` adds `Cpc464Plus`/`Cpc6128Plus`/`Gx4000` alongside the classic three. All three share
the real CPC+ ASIC (`crates/machines/cpc/src/asic.rs`), gated behind the real 15-byte unlock
sequence (fed by every CRTC select-register write) and RMR2 (a Gate-Array function-2 write with bit
5 set, disambiguated from the classic MRER at the same port). Implemented, register-offset-accurate
against CPCTech's own "Extra CPC Plus Hardware Information" (hardware-tested against a real
CPC6128+) cross-checked against MAME's own tested `amstrad_m.cpp` ASIC implementation where the two
needed reconciling -- see `CREDITS.md`:

- Memory-mapped ASIC register window (`0x4000-0x7FFF`, visible only once unlocked and RMR2 selects
  it): 16 hardware sprites (16x16, 1x/2x/4x magnification), the 32-entry palette table (4096-colour,
  12-bit RGB, screen pens 0-15 + border at entries 0-16, sprite colours 1-15 reusing entries 17-31),
  PRI/split-line/split-address/soft-scroll/interrupt-vector/DMA registers, and 2 analogue-joystick
  ADC axis pairs.
- RMR2 lower-ROM relocation (`&0000`/`&4000`/`&8000`, cartridge bank `rmr2 & 0x07`) plus the classic
  upper-ROM-select port reused unchanged for cartridge banks.
- GX4000: no internal firmware at all (real hardware fact) -- boots solely from a mandatory
  cartridge image (the same "rom" blob every model uses), 1-8 real 16K banks, validated by `Cpc::new`.
  464Plus/6128Plus keep their own real classic-shaped internal firmware unchanged; RMR2 relocation
  is present but inert without a separately attached cartridge (not wired to any attach path yet).
  6128Plus keeps the real built-in floppy drive its non-Plus counterpart has; GX4000 has neither a
  floppy nor a cassette port (real hardware fact) and rejects tape media with a clear error.
- Vectored Z80 IM2 interrupts: `machine::Bus` gained `interrupt_vector_byte` (defaulted `0xFF`,
  unchanged for every non-CPC-Plus machine); the ASIC supplies the real, cause-dependent byte
  (raster = vector 6, DMA channel N = vector `N*2`) once unlocked, `0x00` while locked (a real,
  CPC-Plus-specific quirk distinct from classic CPC's floating-bus `0xFF`). The real, documented
  hardware bug where a DMA-caused interrupt's `/INT` line stays asserted until software explicitly
  acknowledges it via DCSR is reproduced faithfully, not "fixed" -- `Cpc::step` re-requests the CPU
  interrupt every instruction while unacknowledged, matching real level-triggered `/INT` sampling.
- Programmable Raster Interrupt: compared against the real CRTC line/raster counters (this crate's
  own cycle-driven CRTC, more precise than the vpos-approximation MAME's own implementation uses)
  each falling HSYNC; classic CPC-style interrupts and PRI cannot be active at the same time (real
  hardware fact) -- suppressed while PRI is active, its own counter bookkeeping still runs.
- 3 DMA sound channels: a real opcode engine (LOAD/PAUSE/REPEAT/LOOP/INT/STOP) executing one
  instruction per enabled channel per HSYNC from primary RAM, verified end to end through a real
  `Cpc::step` loop landing values in the AY-3-8910 without disturbing the CPU's own selected
  register.
- Video: the Plus picture comes from the same per-character trace as the classic machines
  (`crates/machines/cpc/src/raster.rs`): the 4096-colour palette and horizontal soft scroll are
  traced at the exact character they were written on, vertical soft scroll offsets the fetched raster
  address, the screen split overrides the CRTC row address (14-bit `MA` format), and only the 16
  hardware sprites are composited afterwards (`plus_video.rs`) in the ASIC's Mode-2-pixel units,
  relative to the display area's origin. Not modelled: the scroll register's "extend border" bit.
- Analogue joystick: 2 real ADC axis pairs, digital-direction-driven (`Cpc::set_analog_axis`,
  `AnalogJoy1Left/Right/Up/Down`/`AnalogJoy2*` key names through the plugin) -- centred at rest by
  deliberate engineering choice (a usable default), not CPCTech's own measured "nothing attached"
  reading. The classic digital joystick port (keyboard matrix line 9) keeps working unchanged on
  every Plus/GX4000 model too, real hardware fact.
- Save states round-trip full ASIC state (unlock/RMR2/registers/DMA channels); the cartridge image
  itself stays external, same `--rom` convention as every other machine here.
- `cpc464plus|cpc6128plus|gx4000` boot to a real screen through the actual `retroclawd`
  binary (`--headless --dump-screen`-verified, not just unit-tested): a synthetic Z80 test program
  ran the real 15-byte unlock sequence, RMR2 write, and an ASIC-register-window write/readback
  round-trip through real I/O and memory instructions, confirmed against `--dump-memory` output.

### DONE: Caprice32/ZEsarUX comparison and gap-closing execution (2026-09-24)

Comparison sources were read from upstream source and documentation, not feature-list
assumptions:

- Caprice32 source: <https://github.com/ColinPitrat/caprice32>, especially
  `src/cartridge.cpp`, `src/slotshandler.cpp`, `src/asic.cpp`, `src/ipf.cpp`,
  `src/fdc.cpp`, `src/tape.cpp`, `src/cap32.cpp`, and `README.md`.
- ZEsarUX source: <https://github.com/chernandezba/zesarux>, especially
  `src/FEATURES`, `src/machines/cpc.c`, `src/snap/snap.c`,
  `src/snap/snap_zsf.c`, `src/storage/pd765.c`, `src/storage/tape.c`,
  `src/dsk.c`, `src/storage/dandanator.c`, and `src/zrcp/remote.c`.
- CPC format references: <https://www.cpcwiki.eu/index.php/Format:CPR_CPC_Plus_cartridge_file_format>
  and <https://www.cpcwiki.eu/index.php?title=Format:SNA_snapshot_file_format>.

Current retroclawd strengths: all six CPC/Plus/GX4000 models; register-driven CRTC;
Gate Array modes 0/1/2, palette, contention, and frame interrupts; i8255/AY keyboard
and audio; standard and Extended DSK parsing; real-time CDT/TZX input; Plus ASIC
sprites, 4096-colour palette, split/scroll, PRI, vectored IM2, DMA sound, analogue
joystick; model-aware internal save states; browser/TCP debugger through plugin APIs.

| Area | Caprice32 | ZEsarUX | retroclawd status | Priority |
| --- | --- | --- | --- | --- |
| Plus/GX4000 | CPC Plus/GX4000 and partial ASIC | No Plus/GX4000 | **DONE**: ASIC plus raw and `.cpr` cartridge loading | DONE |
| CPC snapshots | `.sna` v1/v2/v3 load/save, including FDC/CRTC state | `.zsf` for CPC; `.sna` is Spectrum-only | **DONE**: CPC `.sna` v1/v2/v3 load/save; ASIC stays internal-state-only | DONE |
| Undocumented video | Mode 3 | Mode 3 | **DONE**: Mode 3 160x200 four-colour decode, including Plus | DONE |
| RAM/ROM expansion | Configurable RAM up to 576K, 16 upper-ROM slots, Multiface 2 | 128K, extra ROMs, Dandanator | **DONE**: 8x64K Standard Memory Expansion banking (up to 576K on 6128/6128Plus), loadable slot 1-6/8-15 expansion ROMs | DONE |
| Disk preservation | Standard/Extended DSK, IPF, CT-RAW, weak/random sectors | Standard/Extended DSK; protection detection, no flux decoder | **PARTIAL DONE**: weak-sector copy cycling now real; IPF/CT-RAW still deferred (external decoder/licensing) | P1 remainder |
| FDC timing | Seek is immediate; coarse transfer timing | 80 T-states/track seek; no rotational timing | **DONE**: opt-in real 80 T-states/track Seek/Recalibrate timing (CPC only; `zx_spectrum`'s +3 FDC keeps its original instant behavior) | DONE |
| Tape | CDT/VOC input; no verified save path | CDT input; save output to TAP/TZX/PZX/O/P/P81 | **DONE**: `.voc` input; CPC cassette output recorded to `.cdt`/`.tzx`/`.voc` (`--tape-save`), proven by a real-firmware SAVE->LOAD round trip | DONE |
| Peripheral hardware | Printer, phazer lightguns, Net4CPC, Multiface 2, digital/mouse joystick | Digital joystick, Dandanator, generic ZRCP; no CPC printer/lightgun/Multiface | AMX mouse + both digital joysticks done; no printer/lightgun/network/Multiface/Dandanator | P2 |
| Debugger | Local memory editor/disassembler/breakpoints | Generic ZRCP CPC commands | **PARTIAL DONE**: `assemble`/`extended-stack`/`get-paging-state` now implemented in the ZRCP-compatible plugin; `write-port` added 2026-10-01; remaining ZEsarUX-specific commands still deferred | P3 remainder |

Important source-backed corrections: Caprice32 accepts Mode 3 but does not implement
the Gate Array IN-as-OUT quirk; ZEsarUX has no CPC `.sna` reader, only CPC `.zsf` and
legacy CPC464 `.zx`; neither reference provides CPC Plus support in ZEsarUX. Caprice32's
`.cpr` reader currently fills cartridge pages sequentially and its source comments
defer using `cbXX` chunk IDs, so retroclawd's parser will retain chunk indexes rather
than copy that limitation. Caprice32's RAM maximum is 576K; ZEsarUX is capped at 128K.
Neither reference proves CPC tape saving in `.cdt`/`.voc`; retroclawd's own recorder is
therefore verified by round trip against the real firmware instead (item 6 below).

**Execution plan, completed in this pass:**

1. **DONE P0 cartridge interoperability** — new format-only `cpc-cartridge` crate parses
   RIFF `AMS!` containers and `cb00`-`cb31` 16K chunks, preserves declared bank indexes,
   zero-fills short pages, rejects malformed/truncated/duplicate/out-of-range/oversized
   chunks, and writes canonical sparse containers. CPC plugin creation accepts `.cpr` for
   GX4000, while raw flat cartridge input remains valid. CLI `--rom` and game-config `media`
   both reach the same creation path.
2. **DONE P0 CPC snapshot interoperability** — new format-only `cpc-snapshot` crate parses
   and writes CPC SNA v1/v2/v3 (`MV - SNA`) with CPU, RAM, Gate Array, CRTC, PPI, AY,
   ROM/RAM selection, CPC model, and verified v3 FDC/printer/timing fields. CPC machine and
   plugin load/save `.sna`; model/RAM mismatches return errors. Public SNA has no ASIC fields,
   so Plus/GX4000 ASIC state stays in the existing internal `.state` format rather than an
   invented extension.
3. **DONE P1 display compatibility** — classic and Plus renderers decode undocumented Mode 3
   as 160x200 four-colour output using the verified two Mode-1 2-bit pixels per byte; Plus
   expands that background into its fixed 640-dot sprite coordinate domain. Regression tests
   distinguish Mode 3 from Mode 0.

### DONE: second pass (RAM/ROM expansion, FDC timing, VOC tape, ZRCP compatibility)

4. **DONE P1 RAM/ROM expansion** — `CpcBus` now implements the real, documented Standard
   Memory Expansion register-3 mechanism (bits 5:3 select a 64K bank 0-7, bits 2:0 keep the
   existing 8-configuration paging table) via `Cpc::set_ram_expansion_banks`/`--ram-banks`
   (6128/6128Plus only, matching real hardware -- 464/664 have no register-3 wiring modeled).
   Total addressable RAM: base 64K + up to 8x64K banks = 576K, the same real-hardware-derived
   maximum Caprice32 exposes, not an arbitrary match. Additional upper-ROM slots 1-6/8-15 load
   via `Cpc::load_expansion_rom`/`--rom-slot N=path` (repeatable) or `rom_slotN` blob keys;
   slot 0 (BASIC) and slot 7 (AMSDOS) stay reserved. Both round-trip through save state.
5. **DONE (partial) P1 disk preservation and timing** — `nec765::Fdc` gained opt-in
   (`enable_seek_timing`, default off so `zx_spectrum`'s own +3 FDC usage is unaffected) real
   80 T-states/track Seek/Recalibrate timing (ZEsarUX-sourced), gating Sense Interrupt Status's
   ST0 Seek End bit behind a real `tick`-driven countdown; CPC wires `tick_fdc` every step and
   enables it. `dsk::Sector` now preserves every real stored weak-sector copy (`copies`, not
   just the first), and `nec765::Fdc::weak_read_counter` rotates through them on successive Read
   Data executions (a real, if simplified single global counter, not Caprice32's own more
   precise per-sector-keyed one -- documented explicitly). IPF/CT-RAW remain deferred: a full
   MFM-bitstream/CAPS-container decoder or a licensed external CapsLib dependency is out of
   scope for a single pass without either being built from scratch or accepting a non-commercial
   license constraint neither of which was approved here.
6. **DONE (tape input) P2 tape/peripherals** — `zx-tape` gained `parse_voc`/`write_voc` (real
   VOC block types 0x00-0x09, 8-bit unsigned PCM only, matching Caprice32's own `VOC_THRESHOLD`
   for the level-decode threshold), wired through CPC and ZX Spectrum plugin tape dispatch and
   `--tape`/game-config media. Printer, phazer/lightgun, Dandanator/Multiface, and Net4CPC
   remain deferred -- each needs its own host/plugin I/O contract, not attempted this pass.
7. **DONE (partial) P3 debugger interoperability** — the ZRCP-compatible TCP plugin
   (`debug-zrcp-plugin`) implements real `assemble` (single-instruction assemble-and-poke via
   the existing `z80::disassemble::assemble_one` assembler, now wired end-to-end through a new
   `DebuggerCommand::Assemble` host round trip that didn't exist before), `extended-stack`
   (real SP-relative memory dump), and `get-paging-state` (CPC/128K-Spectrum paging text).
   `write-port` (real `OUT` through each Z80 machine's own bus decode; see `CLAUDE.md`) was added
   2026-10-01. The remaining ZEsarUX-specific/Spectrum-Next-only commands
   (`tbblue-*`/`zxevo-*`/`tsconf-*`/etc.) stay deferred.

PARKED: Gate Array/CRTC IN-as-OUT revision quirk and CPC+ cut-down 8255 quirks. Both references
omit the first, and the exact byte behavior is revision-specific; the second is narrow and no
known boot path requires it. Do not guess either behavior.

Remaining backlog after this pass: see "BACKLOG: finish CPC" below -- that list supersedes this
one (it adds the per-scanline-video, CRTC-type, disk-write, and Arkanoid-timing gaps this
paragraph never tracked).


Second-pass execution acceptance evidence: full workspace `cargo test --workspace --release`
passes with zero failures; `cargo clippy --workspace --all-targets --release -- -D warnings` is
clean; `cargo build --release -p retroclawd` succeeds. Release CLI smoke tests: `--ram-banks`
accepted on CPC6128 and rejected with a real model-mismatch error on CPC464; `--tape *.voc` loads
through the real plugin path. `nec765`/`dsk`/`zx-tape`/`debug-zrcp-plugin` all gained new,
behavior-real (not tautological) unit tests: bounded Seek-timing countdown via `tick`, weak-sector
copy rotation through the real CPC FDC ports end to end, VOC parse/write round trips, and a real
`assemble` -> poke -> `read-memory` readback round trip through the ZRCP command dispatcher. Two
real bugs were found and fixed during this pass before landing: a double-division error in the
VOC pulse-cycle conversion (produced near-zero pulse durations) and a missing `data_offset`
advance in `dsk::parse_track` (would have made every sector after a track's first read back the
first sector's own bytes) -- both are covered by new regression tests (`multi_sector_track_reads_
each_sectors_own_distinct_data`, the VOC parser test suite).

### BACKLOG: finish CPC (exact remaining work, 2026-09-29)

Executed in this order (user-approved). Each item lands with tests, live `retroclawd` verification,
and a `CLAUDE.md`/`CREDITS.md` update; status is tracked here, not in commit messages.

1. **DONE (2026-09-29) Disk write path** -- turned out to need more than writes: the previous
   `nec765` matched only bare opcodes, had no `EXM`/`CB` status bits and reported the wrong
   success status, so it could not run *real* AMSDOS/+3DOS at all (only hand-written unit tests).
   Rewritten as a proper uPD765A (Caprice32 `fdc.cpp` as the reference), with Write Data/Write
   Deleted/Format Track, `dsk` write + lossless `to_bytes`, modified-disk save states, and opt-in
   `--disk-save`/`--disk-writeback` persistence. Proven against the real CPC6128 firmware
   (`crates/machines/cpc/tests/amsdos_disk.rs`: AMSDOS writes then reads a file) and live through the
   binary. Details in `CLAUDE.md` ("FDC rewrite and disk write path"). Still open under this item:
   a second drive (drive B), Scan commands, `MT` multi-track, rotational timing/overrun -- none is
   needed by the software checked so far.
2. **DONE (2026-09-29) Per-scanline video** -- went further than per-scanline: a per-*character*
   trace (`raster.rs`). `Cpc::step` splits its CRTC tick at the T-state of each picture-affecting
   write (Gate Array ink/mode, ASIC palette/scroll, CRTC registers) so every change lands on the
   character clock it really hit; `Frame::render` replays it into a fixed 768x288-line monitor window
   (Mode-2 pixels, anchored on HSYNC/VSYNC, presented line-doubled as 768x576 = 4:3) for every model. Raster bars, split-mode screens,
   `R12/R13` scrolling and `R1/R2/R6` overscan now show. Proven pixel-exact against an independent
   decode on the real 464/664/6128 boot screens, with exact-character write-timing tests and a live
   rainbow-bar run through the binary. Details in `CLAUDE.md` ("CRTC types and per-character
   video"). Still open under this item: sub-character (fractional) Gate Array timing, the Plus
   "extend border" bit, and interlace (`R8` bits 1-0). Scanline doubling was added 2026-10-01
   (the 768x272 window made the GUI window ~2.8:1 on Windows; now 768x288 lines shown twice =
   768x576, exactly 4:3) -- a presentation step in `Cpc::render_frame`
   (`raster::double_scanlines`), not a measured CPC pixel aspect ratio.
3. **DONE (2026-09-29) CRTC types 0-4** -- `crtc::CrtcType` (`--crtc-type`, `Cpc::set_crtc_type`,
   per-model default 464/664 = 0, 6128 = 1, Plus family = 3, SNA v3 `crtc_type`): register write
   masks and read-back, the status port, `R3` sync-width rules, display skew, and the UM6845R/MC6845
   start-address behaviour, from the ACCC as transcribed by CPCSyntaxError (MIT) with Caprice32 and
   MAME cross-checks (`CREDITS.md`; cpcwiki.eu itself is blocked from this environment). Probed live
   through the binary under each type. Still open: interlace, cursor/light pen, and the ACCC's
   counter-overflow "rupture" corner cases beyond the start-address rules.
4. **DONE (2026-10-01) Boot proof per model** -- ROM-gated tests (skip when a ROM file is absent):
   `real_firmware_boots_to_each_models_own_banner` reads the Mode 1 text layer straight out of screen
   RAM for the real CPC464/664/6128 firmware and checks each model's own banner rows ("Amstrad 64K
   Microcomputer (v1)"/"(v2)"/"128K ... (v3)", the copyright year line, "BASIC 1.0"/"1.1") and the
   `Ready` prompt; `plus_and_gx4000_firmware_smoke_when_roms_are_present` only checks that the
   464Plus/6128Plus/GX4000 produce video fields and a 768x576 window when a ROM/cartridge is supplied
   (no banner check). No Plus/GX4000 firmware or cartridge is committed (no legally supplyable copy),
   so those three are **unverified against real code** -- the synthetic-program ASIC tests are the
   only coverage there.
5. **DONE (2026-10-01) Cheap peripherals** -- second digital joystick (`Joy2*`, matrix line 6, shares
   the `6 5 R T G F B` wires with the keyboard so each source is tracked separately; `Joy2` host binding
   via the keymap) and an opt-in AMX-style mouse (`--amx-mouse`, `cpc::Cpc::set_amx_mouse`,
   `crates/machines/cpc/src/amx_mouse.rs`): host mouse motion becomes time-based LOW pulses on the
   joystick-0 direction lines (matrix line 9 bits 0-3) and the buttons Fire2/Fire1/Fire3 (left/right/
   middle), per CPCWiki's description (read via search-result quotes, the page itself is egress-blocked)
   cross-checked against konCePCja. **Epistemic tier:** the protocol shape is sourced, the pulse
   width/spacing (one 300 Hz polling interval per pulse, 5% high gap, queue cap 255) is this project's
   engineering choice, and no real AMX-reading software has been run against it -- only a Z80 polling
   probe driven by real X mouse events (see `CLAUDE.md`). Kempston/other CPC mice (Symbiface, PS/2
   adapters) are not modelled. (Multiface, printer, lightguns, Dandanator, Net4CPC stay P2 until a
   concrete need.)
6. **DONE (2026-10-01) Tape saving** -- the PPI Port C bit 5 MIC line is timestamped with absolute
   T-states (`cycles_total + instruction_advanced_tstates`, 4 MHz -> the 3.5469 MHz tape clock) into
   `crate::tape_recorder::TapeRecorder`, gated by the motor bit like the playback side; edges become a
   `zx_tape::Tape` (`Tape::from_pulses`) written as `.cdt`/`.tzx` (`write_tzx`: `0x13` pulse sequences,
   long silences as `0x20` pauses) or `.voc` (`write_voc`, 44.1 kHz). `--tape-save <path>` follows the
   disk-persistence policy: the file is written only when something was recorded, at exit
   (`HostedMachine`'s `Drop`, `MachineCommand::ExportTape`, protocol v4); a run that never touched the
   cassette output says so and writes nothing. Proven against the **real CPC464 firmware**
   (`crates/machines/cpc/tests/tape_save.rs`): `CAS OUT OPEN/DIRECT/CLOSE` saves a 1000-byte file, the
   recording is written as CDT and as VOC, and a freshly booted machine `LOAD`s each back to identical
   bytes. Live through the binary: a patched SNA ran the same `SAVE` and produced a valid TZX (115
   blocks, 29004 pulses, ~21 s, ends with a `0x20` pause) and a 931861-byte VOC; a run with no SAVE
   wrote nothing. Limits: ZX Spectrum has no MIC recording yet (the plugin command answers
   "unsupported"), only the CPC cassette output is recorded.
7. **Arkanoid.cdt** -- **LOADS AND RUNS (2026-10-01).** The earlier timing theories (cycle drift, IM1 4-vs-5 NOP,
   interrupt phase) were wrong: tracing what actually executed showed the firmware had never read the first
   file (`ARKANOID`, the stage-1 program) -- the image starts straight on a 2.73 s pilot and the firmware samples
   only after a 2.39 s motor-start delay, leaving 232 pilot pulses -- so it took the *second* file (`!`, the
   SpeedLock stub, which needs stage 1's setup) as the program and sat in a NOP sled. Four real fixes, the first
   three backed by Caprice32's and CPCEC's sources (details in `CLAUDE.md` "Cassette loading: motor, pause edge,
   leader" and `CREDITS.md`): (1) PPI Port C data writes drive the cassette motor (SpeedLock's stub starts the
   tape with `OUT (C),A` to `0xF6xx`); (2) a TZX pause opens with a 1 ms terminating edge (three SpeedLock data
   blocks end on a low pulse, so the loader lost the last byte's last edge and wiped memory); (3) a default 2000 ms
   leading silence (`--tape-leader <ms>`, CPCEC's `tape_safetydelay`); (4) the CPC video frame splitter cut any field
   longer than 312 lines (the title uses a 316-line field, `R4=38 R5=4`) and the 4-line stub became the "latest
   frame", so the title showed one frame in ~90 (`raster::FrameBuilder`'s `synced` state, `SYNCED_MAX_FRAME_LINES`).
   **Result:** stage 1, the stub and `SpeedLock 1 Block 1`-`3` load (tape position 812856/990832) and the game's
   title screen ("(C) 1987 IMAGINE TAITO", controls, 1/2 PLAYER) is shown; pressing `1` runs the story intro
   (scrolling text and stars), then round 1 appears with bricks and paddle (seen live through the real GUI under
   Xvfb, `Z`/`X` move the paddle); the user confirmed the sound is fine. **Not yet exercised:** later rounds, and
   `SpeedLock 1 Block 4`/`5` (5114 bytes each), which the game presumably loads on demand -- the tape stays stopped
   at 812856 until then. **PARKED (2026-10-10):** playing further to trigger them is deferred, not planned;
   resume if someone reports Arkanoid stalling or resetting mid-game (note where, then trace with
   `tests/tape_load_probe.rs`). `Killapede`, `Dan Dare` and `Joe Blade 2` also load
   now (same leader cause), as do `Bomb Jack`, `Jet Set Willy`, `Joe Blade`, `Manic Miner`, `Oh Mummy`. Use
   `tests/tape_load_probe.rs` (ignored) with the tapes under `software/cpc/` as the behavioural check.
   The open decision on a default leader is resolved: 2000 ms, as CPCEC does.
8. **Debug gaps** -- **DONE (2026-10-01):** real banked `space` read/write for CPC 128K/expansion RAM
   (`ram-bank:N` = physical 16K block N, see `CLAUDE.md`; `.POK` reaches blocks 0-7 only because bit 3 of
   a POK bank field means "ignore bank"); **DONE (2026-10-01):** ZRCP `write-port` (`WriteIo`/`WritePort`,
   protocol v5, live-verified over TCP); **DONE (2026-10-01):** I/O-port breakpoints (`machine::PortWatcher`
   fed from the Z80 core's I/O funnel, ZRCP `set-portbreakpoint`/`get-portbreakpoints`/`clear-portbreakpoints`,
   debugger HTTP API and panel; protocol v6, live-verified over ZRCP and in the real browser UI -- see
   `CLAUDE.md`). Also fixed a ZRCP `run` stale-snapshot race and a debugger-page panel-clipping bug found
   while verifying it.
9. **Reference checklist** -- **DONE (2026-10-01):** steps 8-10 below executed against ep128emu's real
   sources, with the CPC accuracy reference set added; it produced two new small backlog items (TZX `0x15`/loops/
   `0x2A`/`0x2B`, Recalibrate's 77-step limit) listed in the table.

Explicitly not planned: IPF/CT-RAW (decoder/licensing), IN-as-OUT and CPC+ 8255 quirks (no hardware
source; see PARKED note above).

### BACKLOG: cross-emulator comparison addendum (ep128emu as additional source)

Source set to consult for all future CPC work in this roadmap:

- https://docs.libretro.com/library/ep128emu/
- https://github.com/libretro/ep128emu-core
- https://github.com/istvan-v/ep128emu

Current comparison snapshot versus retroclawd's listed CPC baseline:

- ep128emu advertises additional CPC RAM configurations (192K/320K/576K) and lower+expansion ROM
  slot handling beyond the currently listed 464/664/6128 baseline here.
- ep128emu documents CPC `.img` disk handling and `.sna` snapshot loading paths that are not yet
  represented as completed items in this roadmap.
- ep128emu itself flags partial/incomplete areas (for example `.cdt` and some FDC paths), so use it
  as a code/reference source together with hardware docs, not as sole truth.

Steps 8-10 below were executed on 2026-10-01 by reading ep128emu's actual CPC sources (`src/cpcmem.cpp`,
`cpcdisk.cpp`, `fdc765.cpp`, `tape.cpp`, `cpc_snap.cpp`, `cpc464vm.cpp`, plus its README) and Caprice32's
`src/tape.cpp`, not just the libretro summary page. The libretro page's `.img` claim does not hold for the CPC
core: `cpcdisk.cpp` accepts only `MV - CPCEMU`/`EXTENDED CPC DSK` headers and throws "unknown CPC disk image
file format" for anything else (`.img` is the Enterprise `ep_fdd.cpp` path).

8. **CPC feature-delta checklist against ep128emu** (what it does that retroclawd's CPC doesn't, what
   retroclawd does that it doesn't, and which differences are deliberate):

   | Area | ep128emu | retroclawd | Verdict |
   |---|---|---|---|
   | RAM sizes | 64/128/192/320/576K via `setRAMSize` (0/1/2/4/8 expansion blocks), set independent of model | `--cpc-ram-banks` 0-7 extra banks on 6128/6128Plus only (128K..576K); 464/664 rejected | Covers every ep128emu size and the in-between ones. **Open question, no hardware source yet:** does a 464/664 + DK'Tronics-style expansion need support? ep128emu allows it (no model gate); our `CREDITS.md` entry says only the 6128 wires register 3. |
   | Bank bits beyond the fitted RAM | masks them off (`setPaging`: `0xFFC7` for 128K, `0xFFCF` 192K, `0xFFDF` 320K) | `ram_bank_select % populated_banks` | Same result for the power-of-two populations ep128emu offers; differs for 3/5/6/7 fitted banks only. Not checked against hardware -- leave until a source exists. |
   | ROM slots | lower ROM + upper ROMs addressed by the high byte of the select port (6 usable bits); an unpopulated slot falls back to BASIC | slots 1-6/8-15 loadable (`--cpc-rom-slot`), 0 = BASIC, 7 = AMSDOS, unpopulated -> BASIC | Equivalent fallback. ep128emu's README says ROMs 0-7 only; its code accepts up to 64 numbers. Ours stops at 15 (the real select decode is a hardware question, not settled by ep128emu). |
   | Disk image formats | standard + extended DSK only (no `.img`), opened `r+b` so **writes go straight back to the file**, plus raw PC floppy device access | standard + extended DSK, writes **opt-in** (`--disk-save`/`--disk-writeback`) | Deliberate difference (no silent host-file writes). Raw `.img`/real-device access: not planned, no CPC software needs it. |
   | Disk write path | FORMAT TRACK (`0x0D`) is "not implemented" (`// FIXME` in `fdc765.cpp`) | full Write Data/Format Track, proven against real AMSDOS | Ours is ahead. |
   | FDC timing | models rotation angle per unit, sector-ID search over two index pulses, per-byte overrun (ST1 `0x10`), step rate from Specify | instantaneous execution, no rotational latency, no overrun; Seek/Recalibrate gated by an opt-in per-track countdown | Deliberate and documented (`nec765` module doc). Copy-protected disks that depend on rotation/overrun are the case this would matter for; revisit only with a failing real disk. |
   | Recalibrate | issues at most 77 step pulses (`recalibrateSteps = 77`) | `seek(params[0], 0)` -- no 77-step limit/EC bit | **Small gap**: from a cylinder beyond 77 a real drive reports Equipment Check. Cheap to add; take the 77 from the uPD765 datasheet (ep128emu is only a pointer). |
   | Weak / short sectors | data longer than the sector = weak (random copy); data shorter = repeated cyclically to fill the sector (`buf[i] = buf[i - dataSize]`) | all stored weak copies kept, rotating counter | Check what our `nec765` does for a stored length *shorter* than the ID's `N` size; unverified either way. |
   | `.cdt`/TZX blocks | `0x10-0x15`, `0x20-0x22`, `0x24/0x25` (loop), `0x2A`, `0x2B` (signal level), `0x30-0x33`, `0x35`, `0x5A`; anything else stops the tape | `0x10-0x14`, `0x20-0x22`, `0x30-0x33`, `0x5A` (informational blocks added 2026-10-01); every other ID is a clean `UnsupportedBlockType` | **Remaining gaps**: Direct Recording `0x15`, loop `0x24/0x25`, `0x2A`, `0x2B`. None of the nine `software/cpc/` tapes uses them (all nine parse), so no known tape is blocked; implement when one appears. Caprice32 also parses `0x15`. |
   | `0x34`/`0x35` | `0x34` rejected; `0x35` reads a **10**-byte identification string then a `u32` length | not accepted | **References disagree**: Caprice32 skips `0x35` as `u32` at offset `0x10` (a 16-byte string, total header `0x14`) and `0x34` as 8 bytes; ep128emu reads 10 bytes. Unresolved here -- the TZX spec page was unreachable (egress blocked). Do not enable either until the spec text is read; step 10 applies. |
   | Snapshots | loads SNA v1/v2; v3 loads but the extra hardware state is ignored; saves only its own format | SNA v1/v2/v3 load **and** save with CRTC type, RAM/paging/PPI/PSG state; `.state` internal | Ours is ahead. |
   | CRTC | type 0 only | types 0-4 | Ours is ahead. |
   | 8255 PPI | modes 1/2 unsupported | mode 0 only | Same; no CPC software uses them. |
   | Tape recording | none (read-only tape) | CDT/VOC recording with `--tape-save` | Ours is ahead. |
   | Debug | memory/I/O register peeks in the low port range via a debug address map | ZRCP + browser debugger, banked RAM, write-port, port breakpoints | Different mechanism, ours covers the use cases. |
   | Mouse/lightgun/printer | none documented for CPC | AMX mouse; no printer/phazer | Nothing to import. |

9. **Using ep128emu per remaining backlog item** (reference notes to read *before* coding each):
   - TZX `0x15`/`0x24/0x25`/`0x2A/0x2B`: ep128emu `tape.cpp` lines ~1097-1210 (readable, short) and
     Caprice32 `tape.cpp` `0x15` for the sample-level semantics; the spec decides every disagreement.
   - Recalibrate step limit: `fdc765.cpp` (`recalibrateSteps`), datasheet for the actual number.
   - FDC rotation/overrun, only if a real disk needs it: `fdc765.cpp` `rotationAngles`/`getPhysicalSector`/
     the `sectorDelay`/`dataIsNotReady` overrun logic.
   - Interrupt timing / Arkanoid: `cpc464vm.cpp` `executeInterrupt`, `memoryWait`, `memoryWaitM1`,
     `ioPortWait` (already used as the cross-check in `tests/instruction_timing.rs`; see `CREDITS.md`).
   - Anything not in the table above has no ep128emu counterpart to consult.

10. **Disagreement policy** (applied, with a live example): when ep128emu and another reference differ, pick
    neither until a hardware source or an executable test distinguishes them. Examples already on the board:
    - IM1 interrupt acceptance: CPCEC and this emulator 4 NOPs, ep128emu about 5 -- varying it changes
      nothing for Arkanoid, so the question stays open (see item 7).
    - TZX `0x35` identification length (above) -- left unsupported rather than guessed.
    - Where all references agree (the opcode timing sweep, I/O alignment) the behavior is locked in by a test.
    Also treat ep128emu's own README caveats (`.cdt` support "not complete", FDC gaps) as reasons not to use it
    as the sole source for those areas.

**CPC accuracy reference set** (use for any future timing/hardware question; none is a substitute for
hardware captures): Caprice32 (`src/z80.cpp` per-opcode NOP tables, `src/crtc.cpp`, `src/fdc.cpp`,
`src/tape.cpp`); ep128emu (`src/cpc464vm.cpp` access-level timing formulas, `fdc765.cpp`, `tape.cpp`);
CPCEC (`cpcec.c`/`cpcec-z8.h`: GA interrupt logic `irq_delay`/`irq_steps`/`irq_timer`, `Z80_WAIT_IR1X`,
stated IM1 = 13 T); MAME `amstrad.cpp`/`amstrad_m.cpp` (CPC+ ASIC); Arnold and konCePCja as further
cross-checks; CPCTech/CPCWiki hardware documents for register-level facts. Clones are kept in the scratchpad
during a session, never vendored (GPL; numbers and structure only, see `CREDITS.md`).

Do not import code blindly; treat ep128emu as a secondary implementation source, not a substitute
for hardware verification.

