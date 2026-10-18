# CPC diagnostics: CRTC type and FDC lines differ from ZEsarUX (S7-hw known gap)

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
