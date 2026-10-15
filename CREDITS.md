# Credits

Third-party references this codebase's implementation is based on -- none
of this code is copied from these sources; each is a fresh Rust
implementation of documented behavior, cited here because that behavior
(file layouts, byte offsets, register constants) came from reading them,
not from independent derivation.


## Amstrad CPC hardware (design research 2026-09-14, `crates/machines/cpc` implemented 2026-09-17)

This entry records the hardware sources used by `crates/machines/cpc`.
The original design pass was verified directly against real emulator source;
the 2026-09-21 timing/video pass rechecked the CRTC counter model against
MAME/Caprice32 and the three official pixel layouts against CPCTech.

**Z80 wait-state mechanism** (not literal Spectrum-style contention): the
Gate Array grants the Z80 bus access only 1 cycle in 4, rounding every
memory access up to a whole multiple of 1µs -- CPCWiki (via search, direct
fetch returned 403 from the research environment), cross-checked against
MAME's Amstrad CPC driver, `src/mame/amstrad/amstrad_m.cpp` (commit
`5346408d5efb5054b84f0015ed18c4e59db99a1c`), lines 2189/2195:
`adjust_icount(-((4 - total_cycles()%4)%4))` on every access.

**Gate Array port decode and function-select register** (address bits
15=0/14=1 select the Gate Array; the top 2 bits of the *data byte* --
00=pen/border select, 01=ink color select, 10=mode/ROM-enable/interrupt-
reset, 11=RAM banking, handled by a separate chip listening on the same
port) and the **interrupt mechanism** (a 6-bit Gate Array counter
incrementing on every CRTC HSYNC, firing at 52 and resetting, with a real
VSYNC-quirk early-reset/suppression rule at count ≥32 two HSYNCs after
VSYNC) -- MAME `amstrad_m.cpp` lines 1580-1702 (Gate Array write) and
832-870 (HSYNC/interrupt), cross-checked against Caprice32
(`github.com/ColinPitrat/caprice32`, `src/crtc.cpp` lines 626-690) and
ZEsarUX (`github.com/chernandezba/zesarux`, `src/machines/cpc.c` lines
835-865, itself citing `cpctech.cpc-live.com/source/split.html`) -- all
three independently implement the identical polarity, resolving ZEsarUX's
own source comment expressing authorial doubt about it.

**6845 CRTC scan timing** (R0 horizontal total, R2/R3 sync position/width,
R4/R5 vertical total/adjust, R7 VSYNC position, R9 maximum raster, with a
1MHz character clock) -- MAME's MC6845 implementation at the same pinned
commit, `src/devices/video/mc6845.cpp` (`register_w`, `line_timer`,
`hsync_on`/`hsync_off`), cross-checked against Caprice32
<https://github.com/ColinPitrat/caprice32/blob/master/src/crtc.cpp>'s
character/raster/line counters. The CPC-specific HSYNC/VSYNC callbacks into
the Gate Array remain sourced from `amstrad_m.cpp` above.

**CPC6128 RAM banking** (128K as 8×16K blocks, config byte's low 3 bits
indexing one of 8 predefined window-mapping tables) -- MAME `amstrad_m.cpp`
lines 104-114 (`RamConfigurations`), cross-checked against Caprice32
`src/cap32.cpp` line 342 confirming non-128K models only accept
configuration 0.

**CPC664/6128 upper ROM and floppy wiring** -- MAME's pinned
`amstrad_m.cpp`: unused upper-ROM slots fall back to BASIC, slot 7 maps
AMSDOS (`m_Amstrad_ROM_Table`, lines 2753-2774); A13=0 writes the upper-ROM
latch (`rom_select`, lines 1932-2026). FDC access is selected by A10=0 and
A7=0, with A8/A0 choosing motor control, main-status, or data register
(lines 1861-1889 and 2065-2104), matching the conventional literal ports
`&FA7E`, `&FB7E`, and `&FB7F`. The shared `nec765` chip behavior and CPCEMU
DSK parser retain their own datasheet/format citations elsewhere in this
file.

**AY-3-8910 access via the Intel 8255 PPI** (Port A = PSG data bus, Port C
bits 7/6 = PSG BDIR/BC1 function select + bits 3-0 = keyboard row select,
Port B = VSYNC/cassette/printer-busy status; base addresses `&F4xx`-`&F7xx`
selected by address bits 9-8) and the **keyboard scan mechanism**
(10 rows × 8 columns; a selected row's data is read back through the AY
chip's own register 14 via PPI Port A, not a direct PPI pin) -- ZEsarUX
`src/machines/cpc.c` lines 894-930 (citing CPCWiki), cross-checked against
MAME `amstrad_m.cpp` lines 2094-2108, 2504-2533, 2538-2630.

**`.cdt` tape format is genuinely TZX-format-identical** (same block-type
IDs/structure; "CDT" is Amstrad tooling's own extension convention for
the same container format Tomaz Kac's TZX spec defines, though a real
cassette-encoded program's own pulse/checksum content still isn't
cross-machine-compatible) -- CPCWiki's "Format:CDT" page, via search.
This project's existing `zx-tape` TZX parser is expected to load real
`.cdt` files with only an extension-recognition change at the dispatch
layer -- not independently verified against MAME/Caprice32/ZEsarUX's own
tape-loading source.

**CPC's native `.dsk` format is confirmed the same CPCEMU format**
`crates/dsk` already parses (see that crate's own credit entry above) --
CPCWiki's "Format:DSK" page states the 1991 CPCEMU origin is shared across
CPC/PCW/+3 disks, cross-referenced against
<https://fileformats.archiveteam.org/wiki/DSK_(CPCEMU)>.

## Amstrad CPC Plus/GX4000 ASIC (`crates/machines/cpc/src/asic.rs`, `crates/machines/cpc/src/plus_video.rs`)

Primary source: CPCTech's own "Extra CPC Plus Hardware Information",
<https://cpctech.cpcwiki.de/docs/cpcplus.html> -- hardware-tested against a
real CPC6128+ (the page's own stated methodology), fetched directly this
session. Every register offset in `asic.rs`'s own constants (sprite pixel
data at `&4000-&5000`, sprite position/magnification at `&6000+`, the
32-entry palette at `&6400`, PRI/split/soft-scroll/interrupt-vector
registers at `&6800-&6805`, the analogue ADC ports at `&6808-&680F`, DMA
channel registers at `&6C00-&6C0F` including the real DCSR bit layout),
the RMR2 lower-ROM-relocation bit layout (`b5`=target-select, `b4b3`=
position, `b2b0`=cartridge bank), the 12-bit-RGB palette byte layout
(low byte = R:upper nibble/B:lower nibble, high byte's low nibble = G),
the sprite X/Y coordinate formulas (CRTC-character-based, in Mode-2-
equivalent dot-clock units regardless of current screen mode), the DMA
opcode encoding (`0RDD` LOAD/`1NNN` PAUSE/`2NNN` REPEAT/`4xxx` control
functions), and the documented DMA-interrupt auto-clear bug (reproduced
faithfully, not "fixed") all come from this document.

Cross-checked against MAME's own tested `amstrad_m.cpp`/`amstrad.h`
(`github.com/mamedev/mame`, GPL-2.0+, shallow-read directly this session
at the `master` branch) for facts CPCTech's own page documents the
mechanism of but not every literal constant: the 15-byte ASIC unlock
sequence itself (`asic_unlock_seq`), the interrupt-vector-cause-to-vector-
low-bits mapping (raster=6, DMA channel N=`N*2` -- verified working
against a real game, Pang, per MAME's own changelog, a higher epistemic
tier than CPCTech's own less-certain table), the exact ASIC-register-
window-visible condition (`rmr2 & 0x18 == 0x18`, gated behind the classic
lower-ROM-enable bit), and the IM2 vector byte's real value while the
ASIC is locked (`0x00`, distinct from classic CPC's floating-bus `0xFF`).

This codebase's own `machine::Bus::interrupt_vector_byte` hook (the Z80
core mechanism that lets a real vectoring controller supply this byte,
defaulted `0xFF`/unchanged for every non-CPC-Plus machine) and the
`Cpc::step`-driven re-request of a DMA-caused, still-unacknowledged
interrupt every instruction (matching real level-triggered `/INT`
sampling) are this project's own design, built to reproduce the documented
behavior above -- not copied from either source.

The fixed 640-wide (Mode-2-dot-clock-equivalent) Plus/GX4000 render domain
in `plus_video.rs`, and its Mode 0/1 pixel-replication-to-fill-width
scheme, are this project's own design decision derived from the cited
sprite-coordinate formula (real hardware composites sprites at that fixed
dot-clock density regardless of screen mode) -- not a literal claim from
either source about how to structure a *snapshot*-architecture renderer,
since both cited sources describe real hardware's live, cycle-by-cycle
pixel generation instead (see `asic.rs`'s own module doc comment for why
a snapshot decode still reproduces the real picture for this feature set).

## CPC parity formats (`crates/formats/cpc-cartridge`, `crates/formats/cpc-snapshot`)

`.cpr` cartridge layout comes from CPCWiki's
<https://www.cpcwiki.eu/index.php/CPR_file_format> and Caprice32's
`src/cartridge.cpp`:
<https://github.com/ColinPitrat/Caprice32/blob/master/src/cartridge.cpp>.
The parser accepts RIFF `AMS!` files, retains each declared `cb00`-`cb31`
bank index, zero-fills short 16K chunks, and rejects oversized, duplicate,
unknown, out-of-range, or truncated chunks. Caprice32's source comment says
its own loader still fills pages sequentially instead of using chunk IDs;
this implementation deliberately keeps declared IDs so sparse/non-sequential
cartridges cannot silently map to the wrong bank.

CPC `MV - SNA` header offsets and v1/v2/v3 field meanings come from Caprice32's
packed `t_SNA_header` in `src/cap32.h` and `snapshot_load`/`snapshot_save` in
`src/slotshandler.cpp`:
<https://github.com/ColinPitrat/Caprice32/blob/master/src/cap32.h> and
<https://github.com/ColinPitrat/Caprice32/blob/master/src/slotshandler.cpp>.
The CPCWiki format page is the independent format reference:
<https://www.cpcwiki.eu/index.php?title=Format:SNA_snapshot_file_format>.
`cpc-snapshot` preserves the fixed 256-byte header and raw RAM dump, including
version-3 FDC/printer/AY-envelope/CRTC/Gate Array timing fields. Applying
those fields to live hardware remains `cpc::Cpc`'s concern; Plus ASIC state is
not fabricated into a public SNA extension and stays in the project's internal
save-state format.

Undocumented Mode 3 decode is cross-checked against ZEsarUX's CPC renderer in
`src/machines/cpc.c` (case 3: two Mode-1 2-bit pixels, each expanded to four
dot clocks):
<https://github.com/chernandezba/zesarux/blob/main/src/machines/cpc.c>.
Caprice32's Gate Array mode register accepts all four values, but neither
reference implements the separate Gate Array/CRTC IN-as-OUT quirk; that quirk
remains deliberately parked rather than guessed.

## CPC RAM expansion, FDC timing, VOC tape, ZRCP compatibility (second parity pass)

The Standard Memory Expansion register-3 bank-select mechanism (bits 5:3 = 64K bank
0-7, bits 2:0 = the existing 8-configuration paging table) is documented directly
in ZEsarUX's own `src/machines/cpc.c` (`cpc_set_memory_pages`'s own comment block,
read directly this session):
<https://raw.githubusercontent.com/chernandezba/zesarux/main/src/machines/cpc.c>.
The resulting 576K maximum (64K base + 8x64K banks) is a consequence of that real
3-bit field width, independently matching Caprice32's own documented maximum --
not copied from Caprice32's source, derived from the same real register.

`nec765::Fdc`'s opt-in real Seek/Recalibrate timing (80 T-states per track step)
comes from ZEsarUX's `src/storage/pd765.c` (`signal_se.max = 80`), cited already
in this project's own `CLAUDE.md`. The opt-in design (default off, only CPC calls
`enable_seek_timing`) is this project's own choice to avoid regressing
`zx_spectrum`'s existing +3 FDC behavior, which never drove the new `tick` method.

`dsk::Sector`'s weak-sector `copies` field and `nec765::Fdc::weak_read_counter`'s
read-attempt rotation are this project's own design, informed by (not copied from)
Caprice32's `t_sector`/`weak_versions_`/`weak_read_version_` fields in `src/disk.h`
-- Caprice32 keys its own rotation per real `(cylinder, head, sector)` address;
this project's simpler single global rolling counter is an explicit, documented
scope reduction, not a claim of equivalent precision.

`.voc` (Creative Voice File) parsing in `crates/formats/zx-tape` follows the block
layout at <https://wiki.multimedia.cx/index.php/Creative_Voice>. The 128-level
decode threshold matches Caprice32's own `VOC_THRESHOLD` constant (`src/cap32.h`).

`assemble`/`extended-stack`/`get-paging-state` in `debug-zrcp-plugin` are this
project's own implementations of real ZEsarUX Remote Command Protocol command
names (syntax cross-checked against ZEsarUX's own `src/zrcp/remote.c` command
table read in an earlier session); `assemble` reuses this project's own existing
`z80::disassemble::assemble_one` Z80 assembler rather than a new one.

## Amstrad CPC implementation details (`crates/machines/cpc`, 2026-09-17)

The timing/video details below were rechecked during the 2026-09-21 pass.
The subsequent model pass booted real CPC464, CPC664, and CPC6128 firmware
images to their `Ready` screens and exercised a parsed DSK sector through the
CPC FDC ports.

- **CPC I/O port decode** -- Gate Array `A15=0,A14=1`; RAM paging PAL
  `A15=0` plus data function `11`; CRTC `A14=0`; upper-ROM latch `A13=0`;
  PPI `A11=0`; expansion peripherals `A10=0`, with FDC additionally selected
  by `A7=0`. `A9:A8` or FDC `A8:A0` select each device's sub-function. These
  independent broad decodes come from MAME `amstrad_m.cpp`'s I/O allocation
  table and read/write handlers, cross-checked against CPCTech's Gate Array
  and RAM-paging pages.
- **CRTC screen start address** (`(R12 << 10) | (R13 << 1)`) and the active
  R0/R2-R5/R7/R9 timing counters -- MAME `mc6845.cpp` and Caprice32
  `src/crtc.cpp` cited above. CRTC state, the CPU/Gate-Array bus phase, and
  the resulting sync positions are serialized in internal save states.
- **Official Mode 0/1/2 pixel decode** and resolutions -- CPCTech's Display
  pixel data format
  <https://cpctech.cpcwiki.de/docs/graphics.html>. Mode 0's byte layout is
  `L0 R0 L2 R2 L1 R1 L3 R3`; Mode 1 is
  `P0.0 P1.0 P2.0 P3.0 P0.1 P1.1 P2.1 P3.1`; Mode 2 is one MSB-first bit
  per pixel. All share the CPC screen interleave
  `screen_start + character_row*80 + raster*0x0800 + column`.
- **27-color hardware palette** (`gate_array::PALETTE`) -- ~~originally
  shipped as an explicitly-flagged structural-only placeholder~~ fixed
  same day: the real hardware-number-to-RGB-level table, fetched from
  CPCTech's own Gate Array documentation
  <https://cpctech.cpcwiki.de/docs/garray.html> (`WebSearch`/`WebFetch`
  reach real pages even though this sandbox's own `Bash`/`curl` cannot
  -- confirmed live). Boot screen now shows the real classic blue, not
  the old structurally-plausible-but-wrong guess.
- Intel 8255 PPI register/mode-bit layout (`crates/chips/i8255`) -- the
  chip's own well-documented, widely-published architecture, same "how
  the chip works, not attributed to one source" tier as this project's
  `ay-3-8910`/6502-opcode-semantics entries.

## Amstrad CPC keyboard matrix (`crates/machines/cpc/src/keyboard.rs`)

The complete 10x8 keyboard matrix table (which key sits at which PPI-
Port-C-selected line / bit) -- CPCTech's own keyboard documentation,
<https://cpctech.cpcwiki.de/docs/keyboard.html>, fetched directly this
session (see this file's Gate Array entry above for the sibling fetch).

## CPC CRTC types and per-character video (`crates/machines/cpc`, 2026-09-29)

**CRTC type behaviour** (`crtc.rs`: register write masks, register/status read-back, `R3` sync-width
rules, `R8` display skew, the UM6845R's per-line `R12/R13` re-read, the MC6845's `VMA'` frame start)
comes from the *Amstrad CPC CRTC Compendium* (ACCC, Longshot, CC BY-NC-ND) **as transcribed with
section references** in CPCSyntaxError's `src/core/crtc.h` and `crtc_type_0.cpp`/`crtc_type_1.h`/
`crtc_type_2.cpp`/`crtc_type_3.h`/`crtc_type_4.cpp` (WacKEDmaN, MIT), shallow-cloned and read
directly. The ACCC PDF itself was unreachable from this environment (cpcwiki.eu and cpctech.cpcwiki.de
are blocked by the egress proxy), so every fact used is one those sources state with its ACCC section:
§4.3 write masks, §14.2/§16.2.1 VSYNC width programmable on types 0/3/4 only, §14.6 `R3l=0` (no HSYNC
on types 0/1, 16 characters on 2/3/4), §19.2 skew field on types 0/3/4 only, §21.2/§21.3/§28.1.9
register and status reads (types 3/4 use a 3-bit register window with the two ASIC status registers),
§20.3.3/§13.4 the MC6845's frame-start-from-`VMA'` rule, §13.3 the UM6845R re-reading `R12/R13` while
`C4=0`. No code or comment text was copied. Known limits kept from those sources' own scope: interlace,
cursor/light pen, and the sub-character/"rupture" corner cases are not modelled.

- **Per-model default type** (464/664 = 0, 6128 = 1, CPC+ family = 3) is konCePCja's
  `src/crtc_types.cpp` table (GPL-2.0-or-later, read directly). Real machines varied by production
  batch, so it is a convention and `--crtc-type` overrides it.
- **Type capability cross-check**: MAME's `src/devices/video/mc6845.cpp` (BSD-3-Clause) --
  `m_supports_disp_start_addr_r` (MC6845 cannot read back `R12/R13`; HD6845S can),
  `m_supports_vert_sync_width` and the status-register capability flags -- and MAME's own
  `src/mame/amstrad/amstrad.cpp` note that CPC CRTC types 0-4 are unfinished there, which is why the
  ACCC-derived sources above are the primary reference.
- **Type-independent counter and address rules** follow Caprice32's `src/crtc.cpp` (already cited
  under the disk section): the display window is `[skew, skew + R1)` with skew 3 disabling display
  (`update_skew`), `VMA' = VMA + C0` when `C0 == R1` on the row's last raster, `VMA` reloaded from
  `R12/R13` when `C4 == 0`, the vertical display enable cleared when `C4 == R6`, the Plus screen split
  as a `VMA'` override at the programmed scanline with the address in `R12/R13` (14-bit `MA`) format
  (`asic.cpp` `split_addr`), and the Plus vertical soft scroll as `RA + scroll` spilling into the next
  row past `R9` (`crtc.cpp` `next_address` arithmetic).
- **MA/RA to RAM address**: `MA0-9 -> A1-A10`, `MA12-13 -> A14-A15`, `RA0-2 -> A11-A13`, `MA10-11`
  unconnected, second byte of a character at `A0 = 1` -- CPCSyntaxError's
  `GX4000::rasterByteAddress`, checked here against the real CPC464/664/6128 firmware boot screens
  (`bus::tests::real_firmware_boot_screen_matches_an_independent_decode_of_screen_ram`).

**Monitor window constants** (`raster.rs`: 48 characters by 288 lines, the window opens 14 characters
after HSYNC begins and 28 lines after VSYNC begins, a monitor with no VSYNC free-runs at 312 lines) are
this project's own engineering choices -- like `machine::with_border`'s 32 px -- fixed so the standard
firmware layout shows the 40x25 display centred with 4 characters (64 px) and 44 lines of border. They
are not broadcast or monitor measurements. The standard 312-line field ends 4 lines short of the
window's last line, so those rows show the border colour (also an engineering choice, chosen over black
bars across a coloured border). `Cpc::render_frame` shows every scanline twice, so the presented picture
is 768x576: the square-pixel 4:3 PAL frame (ITU-R BT.601's 768x576 square-pixel sampling) -- the
window is *4:3 by construction*, not a measured CPC pixel aspect ratio.

### CPC cassette recording, second joystick, and AMX mouse (2026-10-01)

- **Cassette output**: 8255 Port C bit 5 = cassette write data ("MIC"), bit 4 = motor ("remote"), Port B
  bit 7 = cassette read data -- per CPCWiki's "8255" page and the CPC-POWER "8255 PPI" article (found via
  web search; the CPCWiki page itself is egress-blocked here, so read as quoted search results). The
  motor/read half is the assignment the playback side already used (Caprice32 `src/z80.cpp` tape gate,
  `src/cap32.cpp` `CPC.tape_motor = val & 0x10`). Caprice32 has no recording path, so nothing about the
  *recording* was taken from a reference: edges are timestamped from this emulator's own T-state clock and
  converted 4 MHz -> 3.5469 MHz, and the real CPC464 firmware itself, which drives bit 5 in its `SAVE`
  routine, reads the result back correctly (`crates/machines/cpc/tests/tape_save.rs`) -- that round trip
  is the actual evidence the bit assignment and timing are right. The TZX `0x13`/`0x20` block layouts
  written by `zx_tape::write_tzx` are the same ones `parse_tzx` already reads (TZX specification, cited
  under the tape section).
- **Joystick 2** sits on matrix line 6, bits Up(0)/Down(1)/Left(2)/Right(3)/Fire1(4)/Fire2(5)/Fire3(6),
  sharing its wires with the keyboard `6 5 R T G F B` columns -- CPCWiki's "Digital Joysticks" page
  (found via web search, quoted in the result; the page is egress-blocked here). konCePCja defines no
  joystick-2 keys, so there was no second emulator to cross-check against.
- **AMX mouse** (`crates/machines/cpc/src/amx_mouse.rs`):
  - *Protocol facts* -- the interface converts mouse signals into joystick-0 signals, issuing LOW
    pulses on the direction lines (one per "mickey"), with **no motion counters** in the hardware;
    Up/Down/Left/Right = line 9 bits 0/1/2/3, right/left/middle button = Fire1/Fire2/Fire3 = bits
    5/4/6, active low; AMX Art polls from the 300 Hz interrupt, capping the usable rate at about 300
    mickeys per second -- all from CPCWiki's "AMX Mouse" page. That page is blocked from this
    environment's network egress, so these facts were read as the page's text quoted in web-search
    results, not from the page itself; the bit layout was cross-checked against konCePCja's
    `src/amx_mouse.cpp`/`amx_mouse.h` (GPL-2.0-or-later, read directly), which agrees.
  - *Not documented, so this project's own engineering choice* -- the pulse width and spacing: one
    pulse per queued mickey, `4_000_000 / 300` T-states long (one 300 Hz polling interval), low for all
    but the last 5%, queue capped at 255 mickeys per axis. konCePCja models the same device differently
    (a mickey counter consumed once per deselect/reselect of line 9, i.e. software-paced rather than
    time-paced); that is an emulator-level approximation as well, and was not copied. No
    AMX-mouse-reading software was available to test against, only the polling probe described in
    `CLAUDE.md`.

### CPC instruction timing audit and Z80 undefined ED opcodes (2026-10-01)

- **Per-opcode NOP counts** (`crates/machines/cpc/tests/instruction_timing.rs`): Caprice32's `src/z80.cpp`
  (`cc_op`, `cc_cb`, `cc_ed`, `cc_xy`, `cc_xycb`, `cc_ex`; GPL-2.0-or-later), read directly -- the numbers
  only are embedded, no code. The tables' composition rules (a prefix's own cost plus the prefix-specific
  table, `cc_ex` added when a branch is taken, `DDCB` adding `cc_xy[0xCB]` and `cc_xycb`) follow how
  `z80_execute_instruction`/`z80_execute_pfx_*_instruction` apply them. `IN`/`OUT` are excluded because
  Caprice32 replaces the table value with a bus-slot dependent one (`z80_wait_states`); those instructions
  are covered separately, see the I/O-alignment entry below.
- **Cross-check, not vendored**: ep128emu (istvan-v/ep128emu, GPL-2.0-or-later, shallow-cloned into the
  scratchpad) -- its real Z80 core (`z80/z80.cpp`, `z80funcs2.cpp`) driven with the CPC464 timing formulas of
  `src/cpc464vm.cpp` (`memoryWait`, `memoryWaitM1`, `ioPortWait`, `updateCycle(s)`, `executeInterrupt`) in a
  small throwaway driver that stays out of this repository. It agreed with Caprice32 and with the corrected
  model on all 2830 non-prefix-split comparable cases; it times a `DD`/`FD` prefix before a non-indexed opcode
  as a separate 4 T step, so those cases were compared against Caprice32 only. CPCEC (`cpcec-z8.h`, CNGSoft,
  GPL) was read for the structure of `PUSH`/`RET cc`/`DJNZ` (`Z80_WAIT_IR1X(1)` before the bus accesses),
  block-instruction timing, and the stated IM0/IM1/NMI acceptance timing (13 T / 4 NOP, "equals LD A,(nn)");
  its interrupt-delay logic (`irq_delay`/`irq_steps`/`irq_timer`, `z80_irq_ack`) was read but not yet diffed.
- **Undefined `ED xx` opcodes and the NEG/RETN/IM mirrors** (`crates/core/z80/src/opcodes/ed.rs`): Sean Young,
  *The Undocumented Z80 Documented* (the decoder looks at only some opcode bits: `ED 4C/54/5C/64/6C/74/7C` =
  NEG, `ED 55/5D/65/6D/75/7D` = RETN, `ED 4E/66/6E` = IM 0, `ED 76` = IM 1, `ED 7E` = IM 2; every other
  undefined `ED xx`, `ED 77` and `ED 7F` included, is an 8-T-state NOP). This is the widely cited convention; it
  was not re-derived from a hardware capture here, and the existing `ed.rs` test that used `ED 00` as a
  "panics" sentinel already noted that real hardware treats it as an 8-cycle NOP.
- **I/O cycles on the Gate Array slot grid** (`CpcBus::contend_io`, `Bus::internal_cycles` leading cycle of
  `INI`/`IND`/`OUTI`/`OUTD` and their repeat forms): ep128emu's `ioPortWait` (`src/cpc464vm.cpp`) -- an I/O
  cycle is aligned so it ends on the last slot of the four-T-state group, i.e. `4 + ((7 - phase) & 3)` T.
  Found by running the same `IN`/`OUT` sequences through ep128emu's driver (see the cross-check entry above)
  and this emulator: ours answered 12 T for `OUT (C),r` where ep128emu answers 16. The expected totals
  (12 T `IN A,(n)`/`OUT (n),A`, 16 T `IN r,(C)`/`OUT (C),r`, 20 T `INI`/`IND`/`OUTI`/`OUTD`) are what
  Caprice32's `z80_wait_states` macro works out to, as read from its source, and ep128emu reproduces them
  exactly. Locked in by the `IN`/`OUT` and block-I/O cases of `instruction_timing.rs`; disabling `contend_io`
  makes 96 of them fail.
  The leading internal cycle on the `INI`/`IND`/`OUTI`/`OUTD` family is the instruction's own documented
  extra T-state on the second opcode fetch (16 T un-contended per iteration), not a CPC-specific rule.
- **TZX informational blocks** (`crates/formats/zx-tape/src/lib.rs`, `skip_length_prefixed`): block framing for
  `0x30` Text description, `0x31` Message, `0x32` Archive info, `0x33` Hardware type and `0x5A` Glue is taken
  from the TZX format specification and checked against the skip lengths in ep128emu's `tape.cpp` and
  Caprice32's `src/tape.cpp`. `0x34` (Emulation info) and `0x35` (Custom info) stay rejected on purpose: the
  two parsers do not handle them identically, and silently skipping a block with the wrong length would
  corrupt every block after it. Trigger: the real CPC tape `software/cpc/Oh Mummy.cdt` failed to parse at
  all (`0x30`/`0x32`) before this.
- **ep128emu CPC feature-delta review** (`TODO.md`, CPC backlog steps 8-10): read, not copied --
  `src/cpcmem.cpp` (`setRAMSize`, `setPaging`: 64/128/192/320/576K, bank-bit masking, lower/expansion ROM
  segments), `src/cpcdisk.cpp` (DSK/EXTENDED headers only, `r+b` open, weak-sector copy choice, short-sector
  fill), `src/fdc765.cpp` (`recalibrateSteps = 77`, rotation angle/overrun handling, FORMAT TRACK marked
  `FIXME`), `src/tape.cpp` (TZX blocks accepted/rejected, `0x35` read as a 10-byte identification string),
  `src/cpc_snap.cpp` (SNA v1/v2 load, v3 hardware state ignored) and its README's CPC section. Compared with
  Caprice32's `src/tape.cpp`, which skips `0x35` as a 16-byte identification string plus `u32` length and
  `0x34` as 8 bytes: the two disagree, nothing was changed on that basis, and the TZX specification itself
  could not be fetched (egress proxy) -- recorded as an open item. ep128emu (istvan-v, GPL-2.0-or-later) was
  consulted as a secondary source only; its own README calls CDT support and parts of the FDC incomplete.
- **Held `/INT`, acknowledge and `RMR` bit 4** (`gate_array.rs` `interrupt_line`/`acknowledge_interrupt`, `machine::Bus::
  interrupt_acknowledged`, `z80` `accept_interrupt`): three independent emulators agree that the Gate Array's interrupt
  request is a held level, not a pulse. Caprice32 `src/z80.cpp` (`z80_int_handler`: `z80.int_pending = 0;
  GateArray.sl_count &= 0x1f;`) and `src/cap32.cpp` (an `RMR` write with bit 4 sets `int_pending = 0` and
  `sl_count = 0`); CPCEC `cpcec.c` (`z80_irq_ack`: `irq_timer&=~32`, the request bit cleared only there and on an
  `RMR` write with bit 4) and `cpcec-z8.h` (`z80_irq` stays set until acknowledged); ep128emu `src/cpc464vm.cpp`
  (`executeInterrupt`: `gateArrayIRQCounter & 0x1F; clearInterrupt();`, and the same clear on an `RMR` bit-4 write;
  its Z80 core keeps `Z80_INTERRUPT_FLAG` until cleared). Before this, a request raised while the CPU had
  interrupts disabled was dropped, which no reference does. Behaviour only, no code copied.
- **`EI` inhibits interrupts for one instruction** (`z80` `ei_delay`): the standard documented Z80 behaviour (Zilog
  Z80 CPU User Manual, `EI`: "interrupts are not accepted until after the instruction following EI"); needed now
  that a waiting interrupt really is taken as soon as `EI` runs (`EI; RET` must return first). CPCEC's
  `Z80_AUXILIARY`/`z80_active=z80_iff.b.l` "consume EI delay" and Caprice32's `z80.EI_issued` do the same.
- **Refresh register `R`** (`z80` `Cpu::refresh`): increments in its low seven bits on every M1 cycle (opcode and
  prefix fetches -- a `DD CB d op` is two, `DD DD op` three -- the interrupt/NMI acknowledge, and each HALT
  re-execution), bit 7 only via `LD R,A`: Sean Young, *The Undocumented Z80 Documented*, section on the R
  register. The core never counted it before; the CPC firmware's tape sampling loop (`ROM 0x29D4`: `LD A,R`) and
  SpeedLock loaders read it.

### CPC tape loading: Port C motor writes, TZX pause edge, leading silence (2026-10-01)

Found by tracing why `software/cpc/Arkanoid.cdt` never got past its first screen (see `TODO.md` CPC item 7); each
of the three behaviours below was checked against two independent emulators' real sources (kept in the session
scratchpad, read not copied):

- **Cassette motor follows Port C bit 4, however it is written** (`cpc::bus::CpcBus::write_port_c`): the relay is
  driven by the 8255's PC4 output pin, so a data write to Port C and a bit-set/reset command on bit 4 are the same
  thing electrically. Caprice32 `src/cap32.cpp` (`z80_OUT_handler`, `case 2` "write to port C": `CPC.tape_motor = val
  & 0x10` when the upper half is an output, and the same assignment in the bit-set/reset branch) and CPCEC
  `cpcec.c` (`#define tape_enabled (pio_port_c&16)`, the latch whichever write changed it). This crate used to take
  the motor only from bit-set/reset commands -- an unsourced "compatibility" choice that ZEsarUX was credited with
  -- so a loader that starts the tape with `OUT (C),A` to `0xF6xx` (the ROM's own cassette start-up does it, and
  Arkanoid's SpeedLock stage does) waited forever for pulses from a stopped tape. A mode-set (control bit 7) still
  does not drop the motor here (Caprice32 agrees; CPCEC clears the latch and so stops it) -- unchanged, documented.
- **A pause starts with a terminating edge** (`zx_tape::push_pause`): TZX silence is low, but the last pulse before it
  still needs an edge to end it, and when that pulse was already low there is none, so a loader measuring it
  times out. Caprice32 `src/tape.cpp` (every data block's and the `0x20` block's pause: "start with a 1ms level
  opposite to the one last played", then the remaining pause) and CPCEC `src/cpcec-k7.h` (`tape_tails`: the first
  millisecond of a tail toggles the line, then `tape_status=0`). Used-bits handling was already identical in both
  (`(len - 1) * 8 + used`, Caprice32 `dwTapeDataCount`, CPCEC `tape_tzx14`). Three of Arkanoid's five SpeedLock
  data blocks end on a low pulse; without the edge the loader failed the last byte of each and wiped memory.
- **Leading silence** (`zx_tape::Tape::with_leading_silence`, `cpc::DEFAULT_TAPE_LEADER_MS` = 2000, plugin option
  `tape_leader` / `--tape-leader`): CPCEC's `tape_safetydelay()` (`cpcec-k7.h`: `(tape_type>=2)&&(tape_step=2000)`,
  "safety delay in milliseconds", applied as a hold before playback starts for TZX/CDT images and again after
  EOF). The firmware itself waits ~2.4 s after motor-on before sampling (`ROM 0x2A72`, 200 iterations of the
  `0x0682`-count loop at `0x2A89`, measured here as 2.39 s), so an image that opens straight on its first pilot
  -- Arkanoid's is 2.73 s long -- has it mostly played before the firmware listens (232 pilot pulses left, the
  header was missed). 2000 ms is CPCEC's value, an engineering choice here, not a hardware constant.
- **Fields longer than 312 lines** (`raster::FrameBuilder`, `SYNCED_MAX_FRAME_LINES`): a monitor locks to the VSYNC
  pulses it receives whatever the CRTC's field length, so a field is only closed for lack of a pulse after twice the
  PAL length; with no VSYNC at all it still free-runs at 312 lines. The 624-line limit is this project's own
  engineering choice (no source pins it down). Arkanoid's title screen uses `R4=38, R5=4`, i.e. 316 lines
  (`(38+1)*8+4`), which the old unconditional 312-line cut split into a 312-line field plus a 4-line stub.

