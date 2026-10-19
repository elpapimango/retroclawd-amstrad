# CPC timing: 80000 against 79872 T-states per frame (S6 known gap)

**Closed 2026-10-19.** `Cpc` reports an exact frame rate of 4 000 000 / 79 872 (50.08 Hz) through
`Machine::frame_rate` and the plugin's `MachineMetadata::frame_rate`: the frame the firmware programs (R0 = 63, R4 = 38, R9 = 7,
R5 = 0: 312 lines of 64 us). The host paces and budgets by it. A program that reprograms the CRTC changes the real picture rate
but not this pacing quantum.

Found 2026-10-18 by the CPC manifests' scenario S6.

ZEsarUX 13.0 (CPC464 and CPC6128 give the same): a breakpoint at the interrupt entry (0038h) over ZRCP, 24 consecutive
interrupts spaced 13303 to 13321 apart (the breakpoint fires after the instruction that ends them), 319497 and 319493 in all,
that is 13312 per interrupt on average (52 lines of 256 T-states), and six interrupts make a frame: 79872 = 312 x 256 (312 lines of
64 us at 4 MHz).

retroclawd (`retroclawd cpcNNN --headless --frames 100`): 8000563 / 8000627 / 8000683 cycles, i.e. 80000 per frame (4 MHz / 50 Hz).

The difference is 0.16 %: the real CRTC frame is 312 lines (a 50.08 Hz picture), the emulator paces at exactly 50 Hz. No emulator
code was changed to make the scenario pass. Next step: check the CRTC register values the firmware programs (R0 = 63 and R4 = 38,
R7 = 30, 8 lines per character row) and decide whether the frame length should be 79872.
