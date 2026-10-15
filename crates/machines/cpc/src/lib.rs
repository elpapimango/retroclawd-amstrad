mod amx_mouse;
mod asic;
mod bus;
mod crtc;
mod gate_array;
mod keyboard;
mod plus_video;
mod raster;
mod save_state;
mod tape_recorder;
mod video;

/// Supported CPC models. Classic: the 464 has cassette hardware and 64K
/// RAM; the 664 adds the built-in uPD765A floppy subsystem; the 6128
/// adds the same floppy subsystem plus a second 64K RAM block and its
/// paging PAL. Plus/GX4000: `Cpc464Plus`/`Cpc6128Plus` add the real CPC+
/// ASIC (4096-colour palette, hardware sprites, screen split/soft
/// scroll, PRI, vectored IM2 interrupts, DMA sound -- see `crate::asic`)
/// on top of the same classic memory shape their non-Plus counterparts
/// use (6128Plus keeps a real built-in floppy drive, same as 6128;
/// 464Plus has none, same as 464); `Gx4000` is the cartridge-only games
/// console derived from the same Plus hardware -- no internal firmware
/// ROM, no floppy, no cassette port at all (real hardware facts, see
/// `CREDITS.md`), booting solely from a mandatory cartridge image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Model {
    Cpc464,
    Cpc664,
    Cpc6128,
    Cpc464Plus,
    Cpc6128Plus,
    Gx4000,
}

/// CPC cassette transport policy.
///
/// `CapriceStrict` (default) matches Caprice32's real hardware-oriented
/// gate (`z80.cpp`'s `if ((CPC.tape_motor) && (CPC.tape_play_button))`):
/// motor-off pauses transport immediately and EAR returns idle-high, then
/// resumes from the exact same tape position once motor turns back on.
/// This is also the only grounded choice for CDT/.tap *file* playback:
/// ZEsarUX's own CPC source has no real-time EAR-bit pulse path for
/// file-based tape at all (`cpc_in_ppi`'s Port B case only mixes in a
/// live sampled bit for `realtape_*`, i.e. a real cassette deck wired in
/// over an audio cable, or live audio-in recording -- confirmed by
/// reading `src/machines/cpc.c` directly). File-based `.cdt`/`.tap`
/// loading in ZEsarUX instead runs entirely through `tap_load_detect`/
/// `tap_load()` in `src/cores/core_cpc.c`, a ROM-tape-routine trap that
/// pokes each block into memory in one shot -- the same shape as this
/// project's own (Spectrum-only so far) `--fast-tape-load`, not real-time
/// pulse playback at all. So ZEsarUX's "ignore motor-off, keep playing"
/// comment (`cpc_cassette_motor_control`, `TODO_machines` line ~893) is
/// real, but it describes real-cable/audio-in tape, not this crate's
/// `.cdt`-file feature -- citing it to justify continuing playback here
/// was a mistake in an earlier revision of this file, caught after games
/// with custom loaders that use motor-off as a genuine pause signal
/// (Jet Set Willy, Arkanoid) desynced against a tape that kept running
/// underneath them and silently drifted out of frame, corrupting the
/// next block's checksum and dropping back to the BASIC `Ready` prompt --
/// looking exactly like the machine "rebooted" to a user watching it.
/// `ZesaruxCompatibility` is kept as an explicit opt-in for the rare case
/// a loader relies on brief motor glitches not stopping playback (a real,
/// if less common, property of some mechanical decks' own inertia), but
/// is no longer the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum TapeMotorMode {
    #[default]
    CapriceStrict,
    ZesaruxCompatibility,
}

impl Model {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cpc464 => "cpc464",
            Self::Cpc664 => "cpc664",
            Self::Cpc6128 => "cpc6128",
            Self::Cpc464Plus => "cpc464plus",
            Self::Cpc6128Plus => "cpc6128plus",
            Self::Gx4000 => "gx4000",
        }
    }

    /// Required exact size of the classic-shaped "rom" blob (internal
    /// firmware for every model except `Gx4000`, which instead validates
    /// its cartridge image separately -- see `Cpc::new`).
    pub const fn rom_image_size(self) -> usize {
        match self {
            Self::Cpc464 | Self::Cpc464Plus => ROM_IMAGE_SIZE_464,
            Self::Cpc664 | Self::Cpc6128 | Self::Cpc6128Plus => ROM_IMAGE_SIZE_664_6128,
            Self::Gx4000 => 0,
        }
    }

    const fn ram_size(self) -> usize {
        match self {
            Self::Cpc464 | Self::Cpc664 | Self::Cpc464Plus | Self::Gx4000 => 0x10000,
            Self::Cpc6128 | Self::Cpc6128Plus => 0x20000,
        }
    }

    /// Real hardware fact: 6128Plus keeps the same 8-configuration 128K
    /// paging PAL its non-Plus 6128 counterpart has (an ASIC-unrelated
    /// mechanism); 464/664/464Plus/GX4000 all have flat 64K RAM.
    pub(crate) const fn has_128k_paging(self) -> bool {
        matches!(self, Self::Cpc6128 | Self::Cpc6128Plus)
    }

    /// Real hardware fact: 6128Plus keeps the built-in floppy drive its
    /// non-Plus 6128 counterpart has; 464Plus, like 464, has none;
    /// GX4000 has no disk interface at all (console form factor).
    const fn has_fdc(self) -> bool {
        matches!(self, Self::Cpc664 | Self::Cpc6128 | Self::Cpc6128Plus)
    }

    /// Whether this model has the real CPC+ ASIC (4096-colour palette,
    /// hardware sprites, screen split/soft scroll, PRI, vectored IM2
    /// interrupts, DMA sound -- see `crate::asic`).
    pub const fn has_asic(self) -> bool {
        matches!(self, Self::Cpc464Plus | Self::Cpc6128Plus | Self::Gx4000)
    }

    /// Real hardware fact: GX4000 is cartridge-only, with no cassette
    /// port at all -- every other model (classic or Plus) has one.
    pub const fn has_cassette(self) -> bool {
        !matches!(self, Self::Gx4000)
    }

    /// Whether this model has an internal firmware ROM at all (every
    /// model except GX4000, which boots solely from its mandatory
    /// cartridge -- real hardware fact, see `CREDITS.md`).
    pub const fn has_internal_rom(self) -> bool {
        !matches!(self, Self::Gx4000)
    }

    /// The CRTC type this model is built with by default: the CPC464/664
    /// with a type 0 chip, the 6128 with a UM6845R (type 1), and the Plus
    /// family with the AMS40489 ASIC (type 3) -- konCePCja's model-to-type
    /// table. Real machines varied by production batch (a 464 may carry a
    /// type 1, a 6128 a type 2 or, late, a type 4), so this is a
    /// convention, overridable with `Cpc::set_crtc_type`/`--crtc-type`.
    pub const fn default_crtc_type(self) -> CrtcType {
        match self {
            Self::Cpc464 | Self::Cpc664 => CrtcType::Type0,
            Self::Cpc6128 => CrtcType::Type1,
            Self::Cpc464Plus | Self::Cpc6128Plus | Self::Gx4000 => CrtcType::Type3,
        }
    }
}

pub use amx_mouse::MouseButton;
pub use bus::{
    ANALOG_CENTER, ANALOG_MAX, ANALOG_MIN, AnalogAxis, AnalogJoystick, Cpc, DEFAULT_TAPE_LEADER_MS,
    MAX_CARTRIDGE_BANKS, OverlaySnapshot, ROM_IMAGE_SIZE_464, ROM_IMAGE_SIZE_664_6128, ROM_SIZE,
};
pub use crtc::CrtcType;
pub use keyboard::CpcKey;
