# ROM images (Amstrad)

`../firmware.toml` lists the SHA-1 of the dumps each machine option accepts, for `retroclawd --firmware auto` (firmware fetched from a RomM server by hash). It holds hashes only; no ROM is needed for that.

ROM images are copyrighted and are **not** part of this repository. Put your own dumps in a local `roms/` folder
(git-ignored) or let `retroclawd --firmware auto` fetch them by hash. The application never bundles
ROMs; supply your own dumps. The application checks the size of every ROM and refuses a mismatch (a
Spectrum ROM is not silently loaded into a ZX81).

All CPC models take one combined dump with `--rom`; no splitting is needed.

| Machine | File here | Size |
|---|---|---|
| `cpc464`, `cpc464plus` | `cpc/cpc464.rom` | 32K (lower OS + BASIC) |
| `cpc664` | `cpc/cpc664.rom` | 48K (lower OS + BASIC + AMSDOS in upper-ROM slot 7) |
| `cpc6128`, `cpc6128plus` | `cpc/cpc6128.rom` | 48K (lower OS + BASIC + AMSDOS in upper-ROM slot 7) |
| `gx4000` | a `.cpr` cartridge or raw 16K banks (`--rom`) | -- |

CPC+ machines take the same dumps as their classic counterparts here. Extra upper ROMs: `--rom-slot N=path`
(slots 1-15, repeatable).

```
retroclawd cpc464  --rom machines/amstrad/roms/cpc/cpc464.rom
retroclawd cpc6128 --rom machines/amstrad/roms/cpc/cpc6128.rom --disk game.dsk
retroclawd cpc6128 --rom machines/amstrad/roms/cpc/cpc6128.rom --headless --max-cycles 2000000 --dump-screen cpc6128.png
```

The models boot to their `Ready` screens; CPC664/6128 accept `--disk image.dsk` (standard or Extended DSK).
