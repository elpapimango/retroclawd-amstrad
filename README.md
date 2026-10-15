# retroclawd-amstrad

Amstrad CPC (464, 664, 6128, 464+, 6128+, GX4000) emulation for [retroclawd](https://github.com/elpapimango/retroclawd), split out of the main repository.
The machine logic lives here; the retroclawd application links none of it. It loads the
shared libraries this workspace builds (`cpc_machine_plugin`) at run time.

## Machines

`cpc464`, `cpc664`, `cpc6128`, `cpc464plus`, `cpc6128plus`, `gx4000` -- Z80 at 4 MHz, Gate Array and CRTC (types 0-4,
`--crtc-type`) with per-scanline video capture, the three video modes, AY-3-8910 sound, the i8255 PPI, the uPD765A
floppy (`.dsk`/`.edsk`, 664/6128), cassette (`.cdt`/`.tzx`/`.voc`, with recording), the CPC+ ASIC (sprites, palette,
DMA, `.cpr` cartridges), 128K paging and up to seven 64K expansion banks (`--ram-banks`), joysticks and an AMX mouse, SNA snapshots,
`.state` save states and debugger disassembly.

The ROM files and how to call each machine: `docs/firmware.md`.

## Build

```
cargo build --release --workspace
```

produces, in `target/release/`, one library per machine
(`lib<id>_machine_plugin.so`, `<id>_machine_plugin.dll`, `lib<id>_machine_plugin.dylib`).
Put them in the `plugins/` folder next to the `retroclawd` executable (or point
`RETROCLAWD_MACHINE_PLUGIN_DIRS` at the folder) and run `retroclawd <machine> ...`.

## Shared code

Everything that is not specific to this family -- `machine`, `plugin-api`, the `z80`/`mos6502` cores and the
chips/formats several families use -- comes from
[`retroclawd-crates`](https://github.com/elpapimango/retroclawd-crates) as a git dependency pinned to the commit
`577447c075` (the `git = ..., rev = ...` entries in each crate's `Cargo.toml`). To move to a newer commit, run
`tools/bump_pin.sh` in the application repository (it rewrites the pins, runs the tests and records the result), or
by hand: change the rev in every manifest, run `cargo update`, and check the plugin ABI policy in
`retroclawd-crates/docs/plugin-abi.md` (a plugin must be built with the same ABI major, and a protocol version the
application speaks).

The shared crates come from the public repository `retroclawd-crates`, pinned by commit (`rev = ...` in the
`Cargo.toml` files); `tools/bump_pin.sh` in the host repository moves the pin.

## Layout

The directory layout is the same as in the original repository, because the tests find ROMs and
software relative to `CARGO_MANIFEST_DIR/../../../`.

- `crates/machines/cpc`
- `crates/plugins/cpc-machine-plugin`
- `crates/plugin-dlls/cpc`
- `crates/formats/cpc-cartridge`
- `crates/formats/cpc-snapshot`
- `crates/chips/i8255`

ROM images and software are never committed: `roms/` and `software/` are git-ignored folders for your
own copies. Tests that need them skip when they are absent.

## Tests

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The ROM-gated boot tests read the files under `roms/` and skip without them.

## License

Free software under the **GNU General Public License v3.0 or later** (see `LICENSE`). No ROM image or commercial
software is part of this repository or its history; supply your own (see `docs/firmware.md`). Third-party sources
that were read for reference are credited in `CREDITS.md`.

## Contributing / issues

Not accepted right now -- this is a personal work-in-progress project. Pull requests and issues will be closed
unreviewed.
