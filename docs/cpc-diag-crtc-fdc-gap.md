# CPC diagnostics: CRTC type and FDC lines differ from ZEsarUX (S7-hw, closed 2026-10-19)

**Closed 2026-10-19 by a second reference.** Caprice32 4.6 (`retroclawd-tools/scripts/capture_caprice_screen.py`, AMSDOS in slot
7, the 664 at 64K) shows `CRTC 00` and `FDC DETECTED` on both machines: the FDC line confirms ours and shows ZEsarUX's "NONE"
is its own detection failing. Caprice32's CRTC answers as a type 0, and the diagnostics report the type the emulated CRTC
behaves as, so S7-hw now runs ours with `--crtc-type 0` and compares with Caprice32's picture (`golden_crop`: its 768x540 frame
sits 6 lines higher than ours). The crop is the two values only; a run with `--crtc-type 1` fails it (2.99 against 1). The
defaults stay 0 for the 664 and 1 for the 6128 (real boards varied). No emulator code changed.

The original finding:

Found 2026-10-18 by the `S7-hw` scenario of `cpc664.manifest.toml` and `cpc6128.manifest.toml`.

amstrad-diagnostics v1.3 (`RUN"DIAG` from `AmstradDiag.dsk`) prints the CRTC type it detects and whether it finds a floppy
controller. Everything else on its first screen (title, model, vendor, refresh rate, RAM size, the test list, LOWER RAM PASSED and
the untested rest) is identical in ZEsarUX 13.0 and retroclawd (scenarios S7 and S7-results pass).

| | CRTC | FDC |
|---|---|---|
| ZEsarUX 13.0, CPC664 and CPC6128, the diagnostics disk in drive A | 04 | NONE |
| retroclawd CPC664 | 00 | DETECTED |
| retroclawd CPC6128 | 01 | DETECTED |

Reading: the program was loaded from that disk, so a controller exists and "NONE" is the reference's detection failing. CRTC type
4 is the value the diagnostics give the CPC Plus ASIC, not a 664 or 6128. Which of type 0, 1 or 2 a real 664 or 6128 reports depends
on the board revision (UM6845, HD6845S, UM6845R, MC6845); the project's choice (0 for the 664, 1 for the 6128) was not checked
against a source here. No emulator code was changed.
