use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cpc::{Cpc, CpcKey, Model, TapeMotorMode};
use machine::{AUDIO_SAMPLE_RATE, Machine, Region};
use plugin_api::{
    ABI_MAJOR_V1, ABI_MINOR_V1, AddressSpaceInfo, BinaryBlob, BitmapExplorerDefaults,
    BitmapPaletteColor, ChipSnapshot, CodecError, CpuFamily, DebugCommand, DebugResponse,
    DebugValue, DisassemblyLine, DiskStatus, HostMouseButton, MACHINE_PROFILE_CONTRACT_VERSION,
    MachineCapability, MachineCommand, MachineCreateRequest, MachineCreateResponse,
    MachineInstanceCallRequest, MachineInstanceCallResponse, MachineInstanceHandle,
    MachineMetadata, MachineOptionInfo, MachineOptionKind, MachinePluginVTableV1, MachineProfile,
    MachineResponse, MediaKind, MediaSnapshot, NamedValue, PluginDescriptorV1, PluginKind,
    REQUIRED_MACHINE_DEBUG_CAPABILITIES, RcBuffer, RcCallResult, RcSlice, RcStatus, RegisterField,
    RegisterSnapshot, StatusCode, TapeStatus,
};

const PLUGIN_ID: &[u8] = b"retroclawd.cpc.machine";

fn bitmap_palette() -> Vec<BitmapPaletteColor> {
    [
        (0x80, 0x80, 0x80),
        (0x00, 0x00, 0x00),
        (0x00, 0xFF, 0x80),
        (0xFF, 0xFF, 0x80),
        (0x00, 0x00, 0x80),
        (0xFF, 0x00, 0x80),
        (0x00, 0x80, 0x80),
        (0xFF, 0x80, 0x80),
        (0x00, 0x00, 0x00),
        (0x00, 0x00, 0x00),
        (0xFF, 0xFF, 0x00),
        (0xFF, 0xFF, 0xFF),
        (0xFF, 0x00, 0x00),
        (0xFF, 0x00, 0xFF),
        (0xFF, 0x80, 0x00),
        (0xFF, 0x80, 0xFF),
        (0x00, 0x00, 0x00),
        (0x00, 0x00, 0x00),
        (0x00, 0xFF, 0x00),
        (0x00, 0xFF, 0xFF),
        (0x00, 0x00, 0x00),
        (0x00, 0x00, 0xFF),
        (0x00, 0x80, 0x00),
        (0x00, 0x80, 0xFF),
        (0x80, 0x00, 0x80),
        (0x80, 0xFF, 0x80),
        (0x80, 0xFF, 0x00),
        (0x80, 0xFF, 0xFF),
        (0x80, 0x00, 0x00),
        (0x80, 0x00, 0xFF),
        (0x80, 0x80, 0x00),
        (0x80, 0x80, 0xFF),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (r, g, b))| BitmapPaletteColor {
        name: format!("hardware {index:02}"),
        rgb: [r, g, b],
    })
    .collect()
}

fn bitmap_defaults() -> BitmapExplorerDefaults {
    BitmapExplorerDefaults {
        address: 0xC000,
        bytes_width: 80,
        bitplanes: 1,
        zoom: 2,
    }
}
const PLUGIN_VERSION: &[u8] = env!("CARGO_PKG_VERSION").as_bytes();

const fn static_slice(bytes: &'static [u8]) -> RcSlice {
    RcSlice {
        ptr: bytes.as_ptr(),
        len: bytes.len(),
    }
}

struct StaticPluginDescriptor(PluginDescriptorV1);

// SAFETY: descriptor and vtable refer only to immutable static data/function pointers.
unsafe impl Sync for StaticPluginDescriptor {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CpcKind {
    Cpc464,
    Cpc664,
    Cpc6128,
    Cpc464Plus,
    Cpc6128Plus,
    Gx4000,
}

impl CpcKind {
    fn all() -> [Self; 6] {
        [
            Self::Cpc464,
            Self::Cpc664,
            Self::Cpc6128,
            Self::Cpc464Plus,
            Self::Cpc6128Plus,
            Self::Gx4000,
        ]
    }

    fn from_machine_id(machine_id: &str) -> Option<Self> {
        if machine_id.eq_ignore_ascii_case("cpc464") {
            Some(Self::Cpc464)
        } else if machine_id.eq_ignore_ascii_case("cpc664") {
            Some(Self::Cpc664)
        } else if machine_id.eq_ignore_ascii_case("cpc6128") {
            Some(Self::Cpc6128)
        } else if machine_id.eq_ignore_ascii_case("cpc464plus") {
            Some(Self::Cpc464Plus)
        } else if machine_id.eq_ignore_ascii_case("cpc6128plus") {
            Some(Self::Cpc6128Plus)
        } else if machine_id.eq_ignore_ascii_case("gx4000") {
            Some(Self::Gx4000)
        } else {
            None
        }
    }

    fn machine_id(self) -> &'static str {
        match self {
            Self::Cpc464 => "cpc464",
            Self::Cpc664 => "cpc664",
            Self::Cpc6128 => "cpc6128",
            Self::Cpc464Plus => "cpc464plus",
            Self::Cpc6128Plus => "cpc6128plus",
            Self::Gx4000 => "gx4000",
        }
    }

    fn model(self) -> Model {
        match self {
            Self::Cpc464 => Model::Cpc464,
            Self::Cpc664 => Model::Cpc664,
            Self::Cpc6128 => Model::Cpc6128,
            Self::Cpc464Plus => Model::Cpc464Plus,
            Self::Cpc6128Plus => Model::Cpc6128Plus,
            Self::Gx4000 => Model::Gx4000,
        }
    }

    fn has_fdc(self) -> bool {
        matches!(self, Self::Cpc664 | Self::Cpc6128 | Self::Cpc6128Plus)
    }

    fn has_cassette(self) -> bool {
        self.model().has_cassette()
    }

    fn has_internal_rom(self) -> bool {
        self.model().has_internal_rom()
    }
}

static STATE: OnceLock<Mutex<PluginState>> = OnceLock::new();

static MACHINE_VTABLE_V1: MachinePluginVTableV1 = MachinePluginVTableV1 {
    size: MachinePluginVTableV1::expected_size(),
    list_profiles: plugin_list_profiles_v1,
    create_instance: plugin_create_instance_v1,
    destroy_instance: plugin_destroy_instance_v1,
    instance_call: plugin_instance_call_v1,
    free_buffer: plugin_free_buffer_v1,
    step: plugin_step_v1,
    halted: plugin_halted_v1,
    pc: plugin_pc_v1,
};

static PLUGIN_DESCRIPTOR_V1: StaticPluginDescriptor = StaticPluginDescriptor(PluginDescriptorV1 {
    abi_major: ABI_MAJOR_V1,
    abi_minor: ABI_MINOR_V1,
    kind: PluginKind::Machine,
    id: static_slice(PLUGIN_ID),
    version: static_slice(PLUGIN_VERSION),
    vtable: (&MACHINE_VTABLE_V1 as *const MachinePluginVTableV1).cast(),
});

struct PluginState {
    next_instance: MachineInstanceHandle,
    instances: HashMap<MachineInstanceHandle, InstanceRecord>,
}

struct InstanceRecord {
    machine: Cpc,
    kind: CpcKind,
    region: String,
    paused: bool,
    breakpoints: Vec<u16>,
    disk_attached: bool,
    disk_image_id: Option<String>,
}

impl PluginState {
    fn new() -> Self {
        Self {
            next_instance: 1,
            instances: HashMap::new(),
        }
    }

    fn allocate_instance(&mut self) -> MachineInstanceHandle {
        let instance = self.next_instance;
        self.next_instance = self.next_instance.wrapping_add(1).max(1);
        instance
    }

    fn record_mut(
        &mut self,
        instance: MachineInstanceHandle,
    ) -> Result<&mut InstanceRecord, String> {
        self.instances
            .get_mut(&instance)
            .ok_or_else(|| format!("unknown machine instance handle {instance}"))
    }
}

fn state() -> &'static Mutex<PluginState> {
    STATE.get_or_init(|| Mutex::new(PluginState::new()))
}

fn status_from_error(error: &str) -> StatusCode {
    if error.contains("unknown machine instance handle") {
        StatusCode::NotFound
    } else if error.contains("unsupported") {
        StatusCode::Unsupported
    } else {
        StatusCode::InvalidRequest
    }
}

fn decode_request<T: serde::de::DeserializeOwned>(req: RcSlice) -> Result<T, CodecError> {
    // SAFETY: caller owns request bytes for call duration.
    plugin_api::decode_message(unsafe { req.as_bytes() })
}

fn encode_ok<T: serde::Serialize>(value: &T) -> RcCallResult {
    match plugin_api::encode_message(value) {
        Ok(bytes) => RcCallResult::ok_bytes(bytes),
        Err(e) => e.into_call_result(),
    }
}

fn machine_profiles() -> Vec<MachineProfile> {
    CpcKind::all().into_iter().map(machine_profile).collect()
}

fn machine_profile(kind: CpcKind) -> MachineProfile {
    let (capabilities, media_kinds) = if kind.has_fdc() {
        (
            vec![
                MachineCapability::Video,
                MachineCapability::Audio,
                MachineCapability::Tape,
                MachineCapability::Disk,
                MachineCapability::Snapshot,
                MachineCapability::SaveState,
                MachineCapability::Debug,
            ],
            vec![
                MediaKind::Tape,
                MediaKind::Disk,
                MediaKind::Snapshot,
                MediaKind::Program,
            ],
        )
    } else if kind.has_cassette() {
        (
            vec![
                MachineCapability::Video,
                MachineCapability::Audio,
                MachineCapability::Tape,
                MachineCapability::Snapshot,
                MachineCapability::SaveState,
                MachineCapability::Debug,
            ],
            vec![MediaKind::Tape, MediaKind::Snapshot, MediaKind::Program],
        )
    } else {
        // GX4000: real hardware has neither a cassette port nor a disk
        // interface. Its cartridge is mandatory at creation time.
        (
            vec![
                MachineCapability::Video,
                MachineCapability::Audio,
                MachineCapability::Snapshot,
                MachineCapability::Cartridge,
                MachineCapability::SaveState,
                MachineCapability::Debug,
            ],
            vec![
                MediaKind::Snapshot,
                MediaKind::Cartridge,
                MediaKind::Program,
            ],
        )
    };

    MachineProfile {
        machine_id: kind.machine_id().to_string(),
        display_name: kind.machine_id().to_string(),
        cpu_family: CpuFamily::Z80,
        default_region: Some("pal".to_string()),
        contract_version: MACHINE_PROFILE_CONTRACT_VERSION,
        debug_capabilities: REQUIRED_MACHINE_DEBUG_CAPABILITIES.to_vec(),
        capabilities,
        media_kinds,
        options: cpc_options(kind),
        key_names: key_names(),
    }
}

fn cpc_options(kind: CpcKind) -> Vec<MachineOptionInfo> {
    let mut options = vec![MachineOptionInfo {
        key: "rom".to_string(),
        flag: "rom".to_string(),
        short: None,
        kind: MachineOptionKind::File,
        default: None,
        required: true,
        description: if kind.model() == Model::Gx4000 {
            "Cartridge image: raw 16K-aligned banks or a .cpr container".to_string()
        } else {
            "Combined firmware ROM image".to_string()
        },
    }];

    // No declared default: when absent the model's own default applies
    // (`cpc::Model::default_crtc_type`).
    options.push(MachineOptionInfo {
        key: "crtc_type".to_string(),
        flag: "crtc-type".to_string(),
        short: None,
        kind: MachineOptionKind::Int {
            min: Some(0),
            max: Some(4),
        },
        default: None,
        required: false,
        description: "CRTC silicon variant: 0 HD6845S/UM6845, 1 UM6845R, 2 MC6845, \
            3 AMS40489 (CPC+ ASIC), 4 AMS40226 (cost-down 6128); default depends on the model"
            .to_string(),
    });

    // Opt-in: the mouse occupies the joystick-0 port, shared with digital
    // joystick 1 (see `cpc::Cpc::set_amx_mouse`).
    options.push(MachineOptionInfo {
        key: "amx_mouse".to_string(),
        flag: "amx-mouse".to_string(),
        short: None,
        kind: MachineOptionKind::Bool,
        default: Some(DebugValue::Bool(false)),
        required: false,
        description: "Plug an AMX-style mouse into the joystick-0 port (host mouse motion/\
            buttons become direction pulses and fire lines on matrix line 9, shared with \
            joystick 1)"
            .to_string(),
    });

    if kind.has_cassette() {
        options.push(MachineOptionInfo {
            key: "tape_mode".to_string(),
            flag: "tape-mode".to_string(),
            short: None,
            kind: MachineOptionKind::Enum {
                choices: vec!["zesarux".to_string(), "caprice".to_string()],
            },
            default: Some(DebugValue::Text("caprice".to_string())),
            required: false,
            description: "Cassette motor-gate emulation: caprice (strict, default) or zesarux \
                (keep playing through motor-off)"
                .to_string(),
        });
    }

    if kind.has_cassette() {
        options.push(MachineOptionInfo {
            key: "tape_leader".to_string(),
            flag: "tape-leader".to_string(),
            short: None,
            kind: MachineOptionKind::Int {
                min: Some(0),
                max: Some(60_000),
            },
            default: Some(DebugValue::U64(u64::from(cpc::DEFAULT_TAPE_LEADER_MS))),
            required: false,
            description: "Milliseconds of silence put in front of an attached tape. The \
                firmware waits ~2.4 s after motor-on before sampling, so images that start \
                straight on the first pilot (Arkanoid, Killapede, ...) need a leader; CPCEC \
                uses 2000. 0 plays the image exactly as stored"
                .to_string(),
        });
    }

    if kind.has_internal_rom() {
        options.push(MachineOptionInfo {
            key: "rom_slot".to_string(),
            flag: "rom-slot".to_string(),
            short: None,
            kind: MachineOptionKind::FileSet {
                min_index: 1,
                max_index: 15,
            },
            default: None,
            required: false,
            description: "Expansion upper-ROM image for slot N (1-6 or 8-15; slot 0 is BASIC, \
                slot 7 is AMSDOS), repeatable as --rom-slot N=path"
                .to_string(),
        });
        options.push(MachineOptionInfo {
            key: "run".to_string(),
            flag: "run".to_string(),
            short: Some('r'),
            kind: MachineOptionKind::Bool,
            default: Some(DebugValue::Bool(false)),
            required: false,
            description: "Auto-type RUN\" (tape) or RUN\"DISC\" (disk) shortly after boot"
                .to_string(),
        });
    }

    if matches!(kind, CpcKind::Cpc6128 | CpcKind::Cpc6128Plus) {
        options.push(MachineOptionInfo {
            key: "ram_banks".to_string(),
            flag: "ram-banks".to_string(),
            short: None,
            kind: MachineOptionKind::Int {
                min: Some(0),
                max: Some(7),
            },
            default: Some(DebugValue::U64(0)),
            required: false,
            description: "Additional 64K Standard Memory Expansion banks (0-7)".to_string(),
        });
    }

    options
}

fn parse_region(region: Option<&str>) -> Result<Region, String> {
    match region.map(|s| s.to_ascii_lowercase()) {
        None => Ok(Region::Pal),
        Some(value) if value == "pal" => Ok(Region::Pal),
        Some(value) if value == "ntsc" => Ok(Region::Ntsc),
        Some(value) if value == "japan" => Ok(Region::Japan),
        Some(other) => Err(format!("unsupported region \"{other}\"")),
    }
}

fn region_name(region: Region) -> &'static str {
    match region {
        Region::Pal => "pal",
        Region::Ntsc => "ntsc",
        Region::Japan => "japan",
    }
}

fn blob<'a>(roms: &'a [BinaryBlob], key: &str) -> Option<&'a [u8]> {
    roms.iter()
        .find(|blob| blob.key.eq_ignore_ascii_case(key))
        .map(|blob| blob.bytes.as_slice())
}

fn option_value<'a>(options: &'a [NamedValue], key: &str) -> Option<&'a DebugValue> {
    options
        .iter()
        .find(|item| item.key.eq_ignore_ascii_case(key))
        .map(|item| &item.value)
}

fn option_u32(options: &[NamedValue], key: &str) -> Result<Option<u32>, String> {
    let Some(value) = option_value(options, key) else {
        return Ok(None);
    };

    match value {
        DebugValue::U64(v) => u32::try_from(*v)
            .map(Some)
            .map_err(|_| format!("option \"{key}\" out of range for u32")),
        _ => Err(format!("option \"{key}\" must be an unsigned integer")),
    }
}

/// `ram-bank:N` addresses physical 16K RAM block N (0-3 the base 64K, 4-7 the
/// 6128's second 64K, 8+ expansion banks), with the address an offset within
/// the block -- the same shape the ZX Spectrum plugin uses for its 128K banks.
fn ram_block_from_space(space: &str) -> Result<u8, String> {
    space
        .strip_prefix("ram-bank:")
        .ok_or_else(|| format!("unsupported memory space {space:?} (expected \"ram-bank:N\")"))?
        .parse::<u8>()
        .map_err(|_| format!("invalid RAM block memory space {space:?}"))
}

fn read_memory_in_space(
    machine: &Cpc,
    address: u16,
    len: u16,
    space: Option<&str>,
) -> Result<Vec<u8>, String> {
    let Some(space) = space else {
        return Ok((0..len)
            .map(|offset| machine.read_memory(address.wrapping_add(offset)))
            .collect());
    };
    let block = ram_block_from_space(space)?;
    machine.read_ram_block(block, address, len).ok_or_else(|| {
        format!(
            "{space}: block or offset out of range (this machine has {} 16K RAM blocks, offsets 0-0x3FFF)",
            machine.ram_block_count()
        )
    })
}

fn write_memory_in_space(
    machine: &mut Cpc,
    address: u16,
    bytes: &[u8],
    space: Option<&str>,
) -> Result<usize, String> {
    let Some(space) = space else {
        return Ok(machine.load(address, bytes));
    };
    let block = ram_block_from_space(space)?;
    let written = machine.write_ram_block(block, address, bytes);
    if written == 0 && !bytes.is_empty() {
        return Err(format!(
            "{space}: block or offset out of range (this machine has {} 16K RAM blocks, offsets 0-0x3FFF)",
            machine.ram_block_count()
        ));
    }
    Ok(written)
}

fn option_bool(options: &[NamedValue], key: &str) -> Result<Option<bool>, String> {
    let Some(value) = option_value(options, key) else {
        return Ok(None);
    };

    match value {
        DebugValue::Bool(v) => Ok(Some(*v)),
        _ => Err(format!("option \"{key}\" must be a bool")),
    }
}

fn mouse_button_from_host(button: HostMouseButton) -> Option<cpc::MouseButton> {
    match button {
        HostMouseButton::Left => Some(cpc::MouseButton::Left),
        HostMouseButton::Right => Some(cpc::MouseButton::Right),
        HostMouseButton::Middle => Some(cpc::MouseButton::Middle),
        HostMouseButton::Other(_) => None,
    }
}

fn option_tape_motor_mode(options: &[NamedValue]) -> Result<TapeMotorMode, String> {
    let Some(value) = option_value(options, "tape_mode") else {
        return Ok(TapeMotorMode::default());
    };
    let DebugValue::Text(value) = value else {
        return Err("option \"tape_mode\" must be text: zesarux or caprice".to_string());
    };
    match value.to_ascii_lowercase().as_str() {
        "zesarux" | "compat" | "compatibility" => Ok(TapeMotorMode::ZesaruxCompatibility),
        "caprice" | "strict" => Ok(TapeMotorMode::CapriceStrict),
        other => Err(format!(
            "option \"tape_mode\" has invalid value {other:?} (expected zesarux or caprice)"
        )),
    }
}

fn cpc_rom_bytes(kind: CpcKind, bytes: &[u8]) -> Result<Vec<u8>, String> {
    if kind.model() != Model::Gx4000 || !bytes.starts_with(b"RIFF") {
        return Ok(bytes.to_vec());
    }
    let (cartridge, repairs) =
        cpc_cartridge::parse_lenient(bytes).map_err(|e| format!("invalid .cpr cartridge: {e}"))?;
    for repair in repairs {
        eprintln!("cpc: the .cpr cartridge was repaired: {repair}");
    }
    let max_bank = cartridge
        .pages
        .iter()
        .map(|page| page.bank)
        .max()
        .ok_or_else(|| "CPC cartridge contains no pages".to_string())?;
    if max_bank as usize >= cpc::MAX_CARTRIDGE_BANKS {
        return Err(format!(
            "GX4000 cartridge page {} exceeds hardware bank limit {}",
            max_bank,
            cpc::MAX_CARTRIDGE_BANKS - 1
        ));
    }
    let mut flat = vec![0; (usize::from(max_bank) + 1) * cpc::ROM_SIZE];
    for page in cartridge.pages {
        let start = usize::from(page.bank) * cpc::ROM_SIZE;
        flat[start..start + cpc::ROM_SIZE].copy_from_slice(&page.data);
    }
    Ok(flat)
}

fn create_machine(req: MachineCreateRequest) -> Result<MachineCreateResponse, String> {
    let kind = CpcKind::from_machine_id(&req.machine_id)
        .ok_or_else(|| format!("unsupported machine id \"{}\"", req.machine_id))?;

    let rom =
        blob(&req.roms, "rom").ok_or_else(|| "missing required binary blob: rom".to_string())?;
    let rom_bytes = cpc_rom_bytes(kind, rom)?;
    if kind.has_internal_rom() {
        let expected = kind.model().rom_image_size();
        if rom_bytes.len() != expected {
            return Err(format!(
                "rom blob has wrong size: expected {expected} bytes, got {}",
                rom_bytes.len()
            ));
        }
    }
    // GX4000: no fixed size -- `Cpc::new` validates the normalized
    // cartridge's 1-8 bank range.

    let tape_motor_mode = option_tape_motor_mode(&req.options)?;
    let region = parse_region(req.region.as_deref())?;
    let mut machine = Cpc::new(rom_bytes, kind.model())?;
    machine.set_tape_motor_mode(tape_motor_mode);
    if let Some(ms) = option_u32(&req.options, "tape_leader")? {
        machine.set_tape_leader_ms(ms);
    }
    if let Some(index) = option_u32(&req.options, "crtc_type")? {
        let crtc_type = u8::try_from(index)
            .ok()
            .and_then(cpc::CrtcType::from_index)
            .ok_or_else(|| format!("option \"crtc_type\" out of range 0-4: {index}"))?;
        machine.set_crtc_type(crtc_type);
    }
    if let Some(enabled) = option_bool(&req.options, "amx_mouse")? {
        machine.set_amx_mouse(enabled);
    }
    if let Some(banks) = option_u32(&req.options, "ram_banks")? {
        let banks = u8::try_from(banks)
            .map_err(|_| format!("option \"ram_banks\" out of range: {banks}"))?;
        machine.set_ram_expansion_banks(banks)?;
    }
    for blob in &req.roms {
        if let Some(slot) = blob
            .key
            .strip_prefix("rom_slot")
            .and_then(|n| n.parse::<u8>().ok())
        {
            machine.load_expansion_rom(slot, blob.bytes.clone())?;
        }
    }

    let mut state = state()
        .lock()
        .map_err(|_| "plugin state mutex poisoned".to_string())?;
    let instance = state.allocate_instance();
    state.instances.insert(
        instance,
        InstanceRecord {
            machine,
            kind,
            region: region_name(region).to_string(),
            paused: false,
            breakpoints: Vec::new(),
            disk_attached: false,
            disk_image_id: None,
        },
    );

    Ok(MachineCreateResponse {
        instance,
        metadata: MachineMetadata {
            machine_id: kind.machine_id().to_string(),
            model: Some(kind.machine_id().to_string()),
            region: Some(region_name(region).to_string()),
            notes: vec!["external machine plugin v1".to_string()],
            bitmap_palette: bitmap_palette(),
            bitmap_defaults: Some(bitmap_defaults()),
        },
    })
}

fn media_extension(kind: MediaKind, name: Option<&str>) -> String {
    if let Some(filename) = name
        && let Some(ext) = filename.rsplit('.').next()
    {
        return ext.to_ascii_lowercase();
    }

    match kind {
        MediaKind::Tape => "cdt".to_string(),
        MediaKind::Disk => "dsk".to_string(),
        MediaKind::Program => "bin".to_string(),
        MediaKind::Snapshot => "sna".to_string(),
        MediaKind::Cartridge => "cpr".to_string(),
        MediaKind::RomSet => "rom".to_string(),
        MediaKind::Other(other) => other,
    }
}

fn attach_media(
    record: &mut InstanceRecord,
    kind: MediaKind,
    name: Option<String>,
    bytes: Vec<u8>,
    options: Vec<NamedValue>,
) -> Result<(), String> {
    match kind {
        MediaKind::Snapshot => {
            let extension = media_extension(MediaKind::Snapshot, name.as_deref());
            if extension != "sna" {
                return Err(format!(
                    "unsupported snapshot extension for cpc plugin: .{extension} (expected .sna)"
                ));
            }
            let snapshot = cpc_snapshot::parse(&bytes).map_err(|e| e.to_string())?;
            record.machine.load_snapshot(snapshot)
        }
        MediaKind::Tape if !record.kind.has_cassette() => {
            Err(format!("{} has no cassette port", record.kind.machine_id()))
        }
        MediaKind::Tape => {
            let extension = media_extension(MediaKind::Tape, name.as_deref());
            let tape = match extension.as_str() {
                "cdt" | "tzx" => zx_tape::parse_tzx(&bytes).map_err(|e| e.to_string())?,
                "voc" => zx_tape::parse_voc(&bytes).map_err(|e| e.to_string())?,
                "wav" => zx_tape::parse_wav(&bytes).map_err(|e| e.to_string())?,
                other => {
                    return Err(format!(
                        "unsupported tape extension for cpc plugin: .{other} (expected .cdt, .tzx, .voc or .wav)"
                    ));
                }
            };
            let speed = option_u32(&options, "tape_speed")
                .or_else(|_| option_u32(&options, "speed_multiplier"))?
                .unwrap_or(1);
            record.machine.load_tape(tape);
            record.machine.set_tape_speed(speed);
            Ok(())
        }
        MediaKind::Disk => {
            let extension = media_extension(MediaKind::Disk, name.as_deref());
            let disk = match extension.as_str() {
                "dsk" | "edsk" => dsk::parse(&bytes).map_err(|e| e.to_string())?,
                other => {
                    return Err(format!(
                        "unsupported disk extension for cpc plugin: .{other} (expected .dsk or .edsk)"
                    ));
                }
            };
            record.machine.load_disk(disk)?;
            record.disk_attached = true;
            record.disk_image_id = name;
            Ok(())
        }
        MediaKind::Cartridge => {
            if record.kind.model() != Model::Gx4000 {
                return Err("CPC cartridges require GX4000 machine creation".to_string());
            }
            let extension = media_extension(MediaKind::Cartridge, name.as_deref());
            if extension != "cpr" {
                return Err(format!(
                    "unsupported cartridge extension for cpc plugin: .{extension} (expected .cpr)"
                ));
            }
            cpc_cartridge::parse_lenient(&bytes).map_err(|e| e.to_string())?;
            Err("GX4000 cartridge must be supplied as the creation rom blob".to_string())
        }
        MediaKind::Program => {
            let org = option_u32(&options, "org")?.unwrap_or(0x4000);
            let org = u16::try_from(org)
                .map_err(|_| format!("program org out of range for u16: {org}"))?;
            record.machine.load(org, &bytes);
            Ok(())
        }
        other => Err(format!(
            "unsupported media kind for cpc plugin: {:?}",
            other
        )),
    }
}

fn media_snapshot(record: &InstanceRecord) -> MediaSnapshot {
    let playback = record.machine.tape_playback_state();
    let tape = Some(TapeStatus {
        attached: playback.is_some(),
        playing: playback.as_ref().is_some_and(|s| s.playing),
        finished: playback.as_ref().is_some_and(|s| s.finished),
        speed_multiplier: playback.as_ref().map_or(1, |s| s.speed_multiplier),
        position: playback.as_ref().map(|s| s.position as u64),
        total: playback.as_ref().map(|s| s.events as u64),
        motor_on: Some(record.machine.cassette_motor()),
        ear_bit: record.machine.tape_ear_bit(),
        cycles_until_next_edge: record.machine.tape_cycles_until_next_edge().map(u64::from),
        spin_down_cycles: None,
    });

    let disks = if record.kind.has_fdc() {
        vec![DiskStatus {
            attached: record.disk_attached,
            image_id: record.disk_image_id.clone(),
            drive: Some(0),
            head: None,
            track: None,
            sector: None,
            controller_phase: None,
        }]
    } else {
        Vec::new()
    };

    MediaSnapshot {
        tape,
        disks,
        cartridge: None,
    }
}

fn nv_u64(key: &str, value: u64) -> NamedValue {
    NamedValue {
        key: key.to_string(),
        value: DebugValue::U64(value),
    }
}

fn nv_text(key: &str, value: impl Into<String>) -> NamedValue {
    NamedValue {
        key: key.to_string(),
        value: DebugValue::Text(value.into()),
    }
}

fn nv_bool(key: &str, value: bool) -> NamedValue {
    NamedValue {
        key: key.to_string(),
        value: DebugValue::Bool(value),
    }
}

fn join_counts(counts: &[u64]) -> String {
    counts
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn chip_snapshots(record: &InstanceRecord) -> Vec<ChipSnapshot> {
    vec![overlay_chip_snapshot(record)]
}

/// The machine's generic `chip_id: "overlay"` snapshot -- shared by every
/// machine plugin's own `--show-overlay` GUI panel data (`memory_summary`/
/// `fdc_phase`/`disk_motor`/`tape_motor`/`tape_position`/`tape_total`/
/// `tape_edge_cycles` + `tape_waveform`/`tape_pulse_widths` blobs, the
/// common field vocabulary the host renders without per-machine branching)
/// *and* CPC's own richer telemetry (`--debug`'s diagnostic log, ZRCP's
/// `get-paging-state`): PPI/keyboard/PSG register state, upper-ROM/RAM-
/// configuration/gate-mode selects, per-region memory/IO read-write
/// counters, and tape/disk port-access counters. These used to be two
/// separate chips (`"overlay"` and a CPC-only `"cpc.overlay"`) -- merged
/// into one, since the host-side consumers of the CPC-specific data now
/// read whatever fields are actually present generically (see
/// `apps/retroclawd/src/plugin_bridge.rs`'s `overlay_chip_snapshot`) rather
/// than requiring a fixed, CPC-only field set on a separately-named chip.
fn overlay_chip_snapshot(record: &InstanceRecord) -> ChipSnapshot {
    let overlay = record.machine.overlay_snapshot();
    let mut fields = vec![
        nv_text(
            "memory_summary",
            format!(
                "ROM{} RAM{}",
                overlay.upper_rom_select, overlay.ram_configuration
            ),
        ),
        nv_u64("ram_bytes", overlay.ram_bytes as u64),
        nv_u64("rom_bytes", overlay.rom_bytes as u64),
        nv_text("map_0000", overlay.map_0000),
        nv_text("map_4000", overlay.map_4000),
        nv_text("map_8000", overlay.map_8000),
        nv_text("map_c000", overlay.map_c000),
        nv_u64("memory_reads_total", overlay.memory_reads_total),
        nv_u64("memory_writes_total", overlay.memory_writes_total),
        nv_text(
            "memory_read_blocks",
            join_counts(&overlay.memory_read_blocks),
        ),
        nv_text(
            "memory_write_blocks",
            join_counts(&overlay.memory_write_blocks),
        ),
        nv_u64("io_reads_total", overlay.io_reads_total),
        nv_u64("io_writes_total", overlay.io_writes_total),
        nv_u64("ppi_control", u64::from(overlay.ppi_control)),
        nv_u64("ppi_port_a", u64::from(overlay.ppi_port_a)),
        nv_u64("ppi_port_b", u64::from(overlay.ppi_port_b)),
        nv_u64("ppi_port_b_raw", u64::from(overlay.ppi_port_b_raw)),
        nv_u64("ppi_port_c", u64::from(overlay.ppi_port_c)),
        nv_u64(
            "keyboard_row_select",
            u64::from(overlay.keyboard_row_select),
        ),
        nv_u64(
            "psg_selected_register",
            u64::from(overlay.psg_selected_register),
        ),
        nv_u64("upper_rom_select", u64::from(overlay.upper_rom_select)),
        nv_u64("ram_configuration", u64::from(overlay.ram_configuration)),
        nv_u64("gate_mode", u64::from(overlay.gate_mode)),
        nv_u64("crtc_type", u64::from(record.machine.crtc_type().index())),
        nv_text(
            "crtc_chip",
            record.machine.crtc_type().chip_name().to_string(),
        ),
        nv_bool("amx_mouse", record.machine.amx_mouse_enabled()),
        nv_bool("disk_motor", overlay.disk_motor),
        nv_u64("tape_port_reads", overlay.tape_port_reads),
        nv_u64("tape_control_writes", overlay.tape_control_writes),
        nv_u64("tape_motor_toggles", overlay.tape_motor_toggles),
        nv_bool("tape_motor", record.machine.cassette_motor()),
        nv_u64("disk_msr_reads", overlay.disk_msr_reads),
        nv_u64("disk_data_reads", overlay.disk_data_reads),
        nv_u64("disk_data_writes", overlay.disk_data_writes),
        nv_u64("disk_motor_writes", overlay.disk_motor_writes),
    ];
    if let Some(cylinder) = overlay.fdc_cylinder {
        fields.push(nv_u64("fdc_cylinder", u64::from(cylinder)));
    }
    if let Some(phase) = overlay.fdc_phase {
        fields.push(nv_text("fdc_phase", phase));
    }
    if let Some(playback) = record.machine.tape_playback_state() {
        fields.push(nv_u64("tape_position", playback.position as u64));
        fields.push(nv_u64("tape_total", playback.events as u64));
    }
    if let Some(edge_cycles) = record.machine.tape_cycles_until_next_edge() {
        fields.push(nv_u64("tape_edge_cycles", u64::from(edge_cycles)));
    }
    let pulse_width_bytes = overlay
        .tape_pulse_widths
        .iter()
        .flat_map(|width| width.to_le_bytes())
        .collect();
    ChipSnapshot {
        chip_id: "overlay".to_string(),
        fields,
        blobs: vec![
            BinaryBlob {
                key: "tape_waveform".to_string(),
                bytes: overlay.tape_waveform,
            },
            BinaryBlob {
                key: "tape_pulse_widths".to_string(),
                bytes: pulse_width_bytes,
            },
        ],
    }
}

/// Every key name this machine's `KeyEvent` accepts -- built from the same
/// tables the event handler looks names up in, so the list cannot drift.
fn key_names() -> Vec<plugin_api::MachineKeyInfo> {
    let mut infos = plugin_api::key_infos(
        cpc_key_table()
            .iter()
            .map(|(name, _)| *name)
            .chain(cpc_combo_table().iter().map(|(name, _)| *name))
            .chain(cpc_analog_axis_table().iter().map(|(name, _)| *name)),
    );
    plugin_api::set_key_chars(&mut infos, CPC_CHARS);
    infos
}

/// The characters the keys type (unshifted from the firmware's key translation table at ROM `0x1D69`, shifted ones for
/// the combo keys below from the shift table that follows it), for the host's symbolic keyboard layout and for typing
/// (`--type`, paste).
const CPC_CHARS: &[(&str, &str)] = &[
    ("Space", " "),
    ("Comma", ","),
    ("Period", "."),
    ("Slash", "/"),
    ("Caret", "^"),
    ("Minus", "-"),
    ("At", "@"),
    ("Semicolon", ";"),
    ("Colon", ":"),
    ("LeftBracket", "["),
    ("RightBracket", "]"),
    ("Backslash", "\\"),
    ("Exclamation", "!"),
    ("DoubleQuote", "\""),
    ("Hash", "#"),
    ("Dollar", "$"),
    ("Percent", "%"),
    ("Ampersand", "&"),
    ("Apostrophe", "'"),
    ("LeftParen", "("),
    ("RightParen", ")"),
    ("Underscore", "_"),
    ("Equals", "="),
    ("Pound", "\u{a3}"),
    ("Pipe", "|"),
    ("LeftBrace", "{"),
    ("RightBrace", "}"),
    ("Backtick", "`"),
    ("Plus", "+"),
    ("Asterisk", "*"),
    ("Question", "?"),
    ("GreaterThan", ">"),
    ("LessThan", "<"),
];

/// Keys that are a symbol on a shifted key: pressing one presses Shift and the base key together, as typing it on the
/// real keyboard does (the pairs are the shift table of the CPC firmware).
fn cpc_combo_table() -> &'static [(&'static str, (CpcKey, CpcKey))] {
    use CpcKey::*;
    &[
        ("Exclamation", (Shift, Digit1)),
        ("DoubleQuote", (Shift, Digit2)),
        ("Hash", (Shift, Digit3)),
        ("Dollar", (Shift, Digit4)),
        ("Percent", (Shift, Digit5)),
        ("Ampersand", (Shift, Digit6)),
        ("Apostrophe", (Shift, Digit7)),
        ("LeftParen", (Shift, Digit8)),
        ("RightParen", (Shift, Digit9)),
        ("Underscore", (Shift, Digit0)),
        ("Equals", (Shift, Minus)),
        ("Pound", (Shift, Caret)),
        ("Pipe", (Shift, At)),
        ("LeftBrace", (Shift, LeftBracket)),
        ("RightBrace", (Shift, RightBracket)),
        ("Backtick", (Shift, Backslash)),
        ("Plus", (Shift, Semicolon)),
        ("Asterisk", (Shift, Colon)),
        ("Question", (Shift, Slash)),
        ("GreaterThan", (Shift, Period)),
        ("LessThan", (Shift, Comma)),
    ]
}

fn cpc_combo_from_name(name: &str) -> Option<(CpcKey, CpcKey)> {
    cpc_combo_table()
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| *value)
}

fn cpc_key_table() -> &'static [(&'static str, CpcKey)] {
    use CpcKey::*;
    &[
        ("KeyA", A),
        ("KeyB", B),
        ("KeyC", C),
        ("KeyD", D),
        ("KeyE", E),
        ("KeyF", F),
        ("KeyG", G),
        ("KeyH", H),
        ("KeyI", I),
        ("KeyJ", J),
        ("KeyK", K),
        ("KeyL", L),
        ("KeyM", M),
        ("KeyN", N),
        ("KeyO", O),
        ("KeyP", P),
        ("KeyQ", Q),
        ("KeyR", R),
        ("KeyS", S),
        ("KeyT", T),
        ("KeyU", U),
        ("KeyV", V),
        ("KeyW", W),
        ("KeyX", X),
        ("KeyY", Y),
        ("KeyZ", Z),
        ("Digit0", Digit0),
        ("Digit1", Digit1),
        ("Digit2", Digit2),
        ("Digit3", Digit3),
        ("Digit4", Digit4),
        ("Digit5", Digit5),
        ("Digit6", Digit6),
        ("Digit7", Digit7),
        ("Digit8", Digit8),
        ("Digit9", Digit9),
        ("Space", Space),
        ("Enter", Return),
        ("NumpadEnter", Enter),
        ("ShiftLeft", Shift),
        ("ShiftRight", Shift),
        ("ControlLeft", Control),
        ("ControlRight", Control),
        ("CapsLock", CapsLock),
        ("Tab", Tab),
        ("Escape", Escape),
        ("Delete", Delete),
        ("Backspace", Delete),
        ("ArrowUp", CursorUp),
        ("ArrowDown", CursorDown),
        ("ArrowLeft", CursorLeft),
        ("ArrowRight", CursorRight),
        ("Comma", Comma),
        ("Period", Period),
        ("Slash", Slash),
        ("Caret", Caret),
        ("Minus", Minus),
        ("At", At),
        ("Semicolon", Semicolon),
        ("Colon", Colon),
        ("LeftBracket", LeftBracket),
        ("RightBracket", RightBracket),
        ("Backslash", Backslash),
        ("F0", F0),
        ("F1", F1),
        ("F2", F2),
        ("F3", F3),
        ("F4", F4),
        ("F5", F5),
        ("F6", F6),
        ("F7", F7),
        ("F8", F8),
        ("F9", F9),
        ("FDot", FDot),
        // Explicit joystick wires, for the keymap config to bind to any host
        // key (joystick 1 on matrix line 9, joystick 2 on line 6).
        ("Joy1Up", Joy1Up),
        ("Joy1Down", Joy1Down),
        ("Joy1Left", Joy1Left),
        ("Joy1Right", Joy1Right),
        ("Joy1Fire1", Joy1Fire1),
        ("Joy1Fire2", Joy1Fire2),
        ("Joy1Fire3", Joy1Fire3),
        ("Joy2Up", Joy2Up),
        ("Joy2Down", Joy2Down),
        ("Joy2Left", Joy2Left),
        ("Joy2Right", Joy2Right),
        ("Joy2Fire1", Joy2Fire1),
        ("Joy2Fire2", Joy2Fire2),
        ("Joy2Fire3", Joy2Fire3),
    ]
}

fn cpc_key_from_name(name: &str) -> Option<CpcKey> {
    cpc_key_table()
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| *value)
}

/// Joystick 1 rides the arrow keys, alongside the cursor keys and
/// Space bar they already press (real joystick 1 is a separate device, but a
/// keyboard-only user has one set of keys for both conventions, so games
/// reading either respond -- the same "rides the same physical keys"
/// convention as the ZX Spectrum's Kempston-via-arrows). Restores behaviour
/// that lived in the host before key mapping moved into machine plugins and
/// was lost in that move.
fn cpc_joystick1_alias(name: &str) -> Option<CpcKey> {
    Some(match name {
        "ArrowUp" => CpcKey::Joy1Up,
        "ArrowDown" => CpcKey::Joy1Down,
        "ArrowLeft" => CpcKey::Joy1Left,
        "ArrowRight" => CpcKey::Joy1Right,
        // Not Space: the firmware's key table turns joystick 1 fire into the character `X`, so a Space that also pressed
        // fire typed "X " (found typing BASIC through `--type`). Bind `Joy1Fire1` to a host key with `[keys]` for games.
        _ => return None,
    })
}

/// CPC Plus/GX4000 analogue-joystick key names, distinct from the
/// classic digital `Joy1*` matrix keys `cpc_key_from_name` already maps
/// (real hardware keeps both ports simultaneously, see `cpc::asic`'s own
/// doc comment). `true`/`false`: positive (right/down, drives
/// `ANALOG_MAX`) or negative (left/up, drives `ANALOG_MIN`) direction.
fn cpc_analog_axis_table() -> &'static [(&'static str, (cpc::AnalogJoystick, cpc::AnalogAxis, bool))]
{
    use cpc::{AnalogAxis, AnalogJoystick};
    &[
        (
            "AnalogJoy1Left",
            (AnalogJoystick::First, AnalogAxis::X, false),
        ),
        (
            "AnalogJoy1Right",
            (AnalogJoystick::First, AnalogAxis::X, true),
        ),
        (
            "AnalogJoy1Up",
            (AnalogJoystick::First, AnalogAxis::Y, false),
        ),
        (
            "AnalogJoy1Down",
            (AnalogJoystick::First, AnalogAxis::Y, true),
        ),
        (
            "AnalogJoy2Left",
            (AnalogJoystick::Second, AnalogAxis::X, false),
        ),
        (
            "AnalogJoy2Right",
            (AnalogJoystick::Second, AnalogAxis::X, true),
        ),
        (
            "AnalogJoy2Up",
            (AnalogJoystick::Second, AnalogAxis::Y, false),
        ),
        (
            "AnalogJoy2Down",
            (AnalogJoystick::Second, AnalogAxis::Y, true),
        ),
    ]
}

fn cpc_analog_axis_from_name(name: &str) -> Option<(cpc::AnalogJoystick, cpc::AnalogAxis, bool)> {
    cpc_analog_axis_table()
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| *value)
}

fn inferred_width_bits(value: u64) -> u16 {
    if value <= u8::MAX as u64 {
        8
    } else if value <= u16::MAX as u64 {
        16
    } else if value <= u32::MAX as u64 {
        32
    } else {
        64
    }
}

fn register_snapshot(registers: Vec<(&'static str, u64)>) -> RegisterSnapshot {
    let registers = registers
        .into_iter()
        .map(|(name, value)| RegisterField {
            bank: "main".to_string(),
            name: name.to_string(),
            value,
            width_bits: inferred_width_bits(value),
        })
        .collect();

    RegisterSnapshot {
        cpu_family: CpuFamily::Z80,
        registers,
        flags: Vec::new(),
    }
}

fn handle_machine_command(
    record: &mut InstanceRecord,
    command: MachineCommand,
) -> Result<MachineResponse, String> {
    match command {
        MachineCommand::GetClockHz => Ok(MachineResponse::ClockHz(record.machine.clock_hz())),
        MachineCommand::GetFrameHz => Ok(MachineResponse::FrameHz(record.machine.frame_hz())),
        MachineCommand::RequestInterrupt => {
            record.machine.request_interrupt();
            Ok(MachineResponse::Ack)
        }
        MachineCommand::Step => {
            let cycles = record.machine.step();
            Ok(MachineResponse::Step(plugin_api::StepResult {
                cycles,
                pc: record.machine.pc(),
                halted: record.machine.halted(),
            }))
        }
        MachineCommand::IsHalted => Ok(MachineResponse::Halted(record.machine.halted())),
        MachineCommand::GetPc => Ok(MachineResponse::Pc(record.machine.pc())),
        MachineCommand::Reset => {
            record.machine.reset();
            Ok(MachineResponse::Ack)
        }
        MachineCommand::ReadMemory {
            address,
            len,
            space,
        } => Ok(MachineResponse::Memory(read_memory_in_space(
            &record.machine,
            address,
            len,
            space.as_deref(),
        )?)),
        MachineCommand::WriteMemory {
            address,
            bytes,
            space,
        } => {
            let written =
                write_memory_in_space(&mut record.machine, address, &bytes, space.as_deref())?;
            Ok(MachineResponse::BytesWritten(written as u32))
        }
        MachineCommand::LoadBytes { address, bytes } => {
            let written = record.machine.load(address, &bytes);
            Ok(MachineResponse::BytesWritten(written as u32))
        }
        MachineCommand::SetPc { address } => {
            record.machine.set_pc(address);
            Ok(MachineResponse::Ack)
        }
        MachineCommand::SaveState => Ok(MachineResponse::StateBlob(record.machine.save_state())),
        MachineCommand::SaveSnapshot { extension } if extension.eq_ignore_ascii_case("sna") => {
            let snapshot = record.machine.save_snapshot();
            let bytes = cpc_snapshot::write(&snapshot).map_err(|e| e.to_string())?;
            Ok(MachineResponse::SnapshotBlob(bytes))
        }
        MachineCommand::SaveSnapshot { extension } => Err(format!(
            "unsupported snapshot save extension \".{extension}\" for cpc plugin (expected .sna)"
        )),
        MachineCommand::LoadState { bytes } => {
            record.machine.load_state(&bytes)?;
            Ok(MachineResponse::Ack)
        }
        MachineCommand::AttachMedia {
            slot: _,
            kind,
            name,
            bytes,
            options,
        } => {
            attach_media(record, kind, name, bytes, options)?;
            Ok(MachineResponse::Ack)
        }
        MachineCommand::DetachMedia { slot } => match slot {
            plugin_api::MediaSlot::Tape => {
                record.machine.eject_tape();
                Ok(MachineResponse::Ack)
            }
            plugin_api::MediaSlot::Disk { drive: 0 } => {
                record.machine.eject_disk();
                record.disk_attached = false;
                record.disk_image_id = None;
                Ok(MachineResponse::Ack)
            }
            plugin_api::MediaSlot::Disk { drive } => {
                Err(format!("drive {drive} does not exist (only drive 0)"))
            }
            other => Err(format!(
                "nothing to eject from slot {other:?} on the cpc plugin (tape or disk drive 0)"
            )),
        },
        MachineCommand::ExportDisk {
            drive,
            only_if_modified,
        } => {
            if drive != 0 {
                return Err(format!("drive {drive} does not exist (only drive 0)"));
            }
            let image = if only_if_modified {
                record.machine.take_modified_disk_image()
            } else {
                record.machine.disk().map(dsk::Disk::to_bytes)
            };
            Ok(MachineResponse::DiskImage(image))
        }
        MachineCommand::MarkDiskSaved { drive } => {
            if drive != 0 {
                return Err(format!("drive {drive} does not exist (only drive 0)"));
            }
            record.machine.mark_disk_saved();
            Ok(MachineResponse::Ack)
        }
        MachineCommand::Freeze
        | MachineCommand::ExportCartridge
        | MachineCommand::ExportPrinter => {
            Err("unsupported: the cpc has no freezer or writable cartridge".to_string())
        }
        MachineCommand::ExportTape { format } => {
            let Some(tape) = record.machine.recorded_tape() else {
                return Ok(MachineResponse::TapeImage(None));
            };
            let image = match format.to_ascii_lowercase().as_str() {
                "cdt" | "tzx" => zx_tape::write_tzx(&tape),
                "voc" => zx_tape::write_voc(&tape, 44_100)
                    .map_err(|e| format!("cannot write the recording as .voc: {e}"))?,
                other => {
                    return Err(format!(
                        "unsupported tape format \"{other}\" (expected cdt, tzx or voc)"
                    ));
                }
            };
            Ok(MachineResponse::TapeImage(Some(image)))
        }
        MachineCommand::GetMediaSnapshot => {
            Ok(MachineResponse::MediaSnapshot(media_snapshot(record)))
        }
        MachineCommand::SetBorder { enabled: _ } => Ok(MachineResponse::Ack),
        MachineCommand::KeyEvent { key, pressed } => {
            if let Some((joystick, axis, positive)) = cpc_analog_axis_from_name(&key) {
                let value = if !pressed {
                    cpc::ANALOG_CENTER
                } else if positive {
                    cpc::ANALOG_MAX
                } else {
                    cpc::ANALOG_MIN
                };
                record.machine.set_analog_axis(joystick, axis, value);
                return Ok(MachineResponse::Ack);
            }
            if let Some((modifier, base)) = cpc_combo_from_name(&key) {
                for key in [modifier, base] {
                    if pressed {
                        record.machine.press_key(key);
                    } else {
                        record.machine.release_key(key);
                    }
                }
                return Ok(MachineResponse::Ack);
            }
            if let Some(mapped) = cpc_key_from_name(&key) {
                if pressed {
                    record.machine.press_key(mapped);
                } else {
                    record.machine.release_key(mapped);
                }
                if let Some(joystick) = cpc_joystick1_alias(&key) {
                    if pressed {
                        record.machine.press_key(joystick);
                    } else {
                        record.machine.release_key(joystick);
                    }
                }

                let row0 = record.machine.keyboard_row_state(0);
                let row2 = record.machine.keyboard_row_state(2);
                let tape = record.machine.tape_playback_state();
                tracing::info!(
                    target: "retroclawd::cpc_input",
                    host_key = %key,
                    mapped = ?mapped,
                    pressed,
                    row_select = record.machine.keyboard_row_select(),
                    psg_register = record.machine.psg_selected_register(),
                    row0 = format!("0x{row0:02X}"),
                    row2 = format!("0x{row2:02X}"),
                    tape_motor = record.machine.cassette_motor(),
                    tape_pos = tape.map_or(0, |s| s.position),
                    tape_events = tape.map_or(0, |s| s.events),
                    tape_playing = tape.is_some_and(|s| s.playing),
                    tape_finished = tape.is_some_and(|s| s.finished),
                    "cpc key event applied"
                );
            } else {
                tracing::debug!(
                    target: "retroclawd::cpc_input",
                    host_key = %key,
                    pressed,
                    "cpc key event ignored"
                );
            }
            Ok(MachineResponse::Ack)
        }
        MachineCommand::MouseMotion { dx, dy } => {
            record.machine.mouse_motion(dx, dy);
            Ok(MachineResponse::Ack)
        }
        MachineCommand::MouseButton { button, pressed } => {
            if let Some(button) = mouse_button_from_host(button) {
                record.machine.mouse_button(button, pressed);
            }
            Ok(MachineResponse::Ack)
        }
        // The AMX mouse has no wheel.
        MachineCommand::MouseWheel { .. } => Ok(MachineResponse::Ack),
        MachineCommand::WriteIo { port, value } => {
            record.machine.write_port(port, value);
            Ok(MachineResponse::Ack)
        }
        MachineCommand::SetPortWatches { watches } => {
            record.machine.set_port_watches(
                watches
                    .into_iter()
                    .map(|w| machine::PortWatch {
                        port: w.port,
                        mask: w.mask,
                        on_read: w.on_read,
                        on_write: w.on_write,
                    })
                    .collect(),
            );
            Ok(MachineResponse::Ack)
        }
        MachineCommand::TakePortHit => Ok(MachineResponse::PortHit(
            record
                .machine
                .take_port_hit()
                .map(|hit| plugin_api::PortHitInfo {
                    port: hit.port,
                    value: hit.value,
                    write: hit.write,
                }),
        )),
        MachineCommand::RenderFrame => {
            let (width, height, rgb) = record.machine.render_frame().ok_or_else(|| {
                format!("render_frame unavailable for {}", record.kind.machine_id())
            })?;
            Ok(MachineResponse::Frame { width, height, rgb })
        }
        MachineCommand::DrainAudio => Ok(MachineResponse::AudioPcmMono {
            sample_rate: AUDIO_SAMPLE_RATE,
            samples: record.machine.drain_audio(),
        }),
        MachineCommand::Debug(command) => {
            let response = handle_debug_command(record, command)?;
            Ok(MachineResponse::Debug(response))
        }
    }
}

fn handle_debug_command(
    record: &mut InstanceRecord,
    command: DebugCommand,
) -> Result<DebugResponse, String> {
    match command {
        DebugCommand::Pause => {
            record.paused = true;
            Ok(DebugResponse::Ack)
        }
        DebugCommand::Resume => {
            record.paused = false;
            Ok(DebugResponse::Ack)
        }
        DebugCommand::StepInstruction | DebugCommand::StepOver => {
            record.machine.step();
            Ok(DebugResponse::Ack)
        }
        DebugCommand::SetBreakpoint { address } => {
            if !record.breakpoints.contains(&address) {
                record.breakpoints.push(address);
                record.breakpoints.sort_unstable();
            }
            Ok(DebugResponse::Ack)
        }
        DebugCommand::ClearBreakpoint { address } => {
            record.breakpoints.retain(|bp| *bp != address);
            Ok(DebugResponse::Ack)
        }
        DebugCommand::ClearAllBreakpoints => {
            record.breakpoints.clear();
            Ok(DebugResponse::Ack)
        }
        DebugCommand::ListBreakpoints => Ok(DebugResponse::Breakpoints(record.breakpoints.clone())),
        DebugCommand::GetRegisters => Ok(DebugResponse::Registers(register_snapshot(
            record.machine.registers(),
        ))),
        DebugCommand::SetRegister { name, value } => {
            record.machine.set_register(&name, value)?;
            Ok(DebugResponse::Ack)
        }
        DebugCommand::GetAddressSpaces => {
            Ok(DebugResponse::AddressSpaces(vec![
                AddressSpaceInfo {
                    name: "cpu".to_string(),
                    addr_width_bits: 16,
                    bank_context: Vec::new(),
                },
                // `ram-bank:N` for N in 0..blocks: physical 16K RAM block N,
                // addressed by offset within the block.
                AddressSpaceInfo {
                    name: "ram-bank".to_string(),
                    addr_width_bits: 14,
                    bank_context: vec![nv_u64("blocks", record.machine.ram_block_count() as u64)],
                },
            ]))
        }
        DebugCommand::ReadMemory {
            address,
            len,
            space,
        } => Ok(DebugResponse::Memory(read_memory_in_space(
            &record.machine,
            address,
            len,
            space.as_deref(),
        )?)),
        DebugCommand::WriteMemory {
            address,
            bytes,
            space,
        } => {
            write_memory_in_space(&mut record.machine, address, &bytes, space.as_deref())?;
            Ok(DebugResponse::Ack)
        }
        DebugCommand::GetDisassembly { start, lines } => {
            let mut cursor = start.unwrap_or(record.machine.pc());
            let mut out = Vec::with_capacity(lines as usize);
            for _ in 0..lines {
                let raw: Vec<u8> = (0..4u16)
                    .map(|offset| record.machine.read_memory(cursor.wrapping_add(offset)))
                    .collect();
                let (text, consumed) = z80::disassemble::disassemble(&raw);
                let consumed = consumed.clamp(1, 4);
                out.push(DisassemblyLine {
                    address: cursor,
                    bytes: raw[..consumed].to_vec(),
                    text,
                });
                cursor = cursor.wrapping_add(consumed as u16);
            }
            Ok(DebugResponse::Disassembly(out))
        }
        DebugCommand::GetMediaSnapshot => Ok(DebugResponse::MediaSnapshot(media_snapshot(record))),
        DebugCommand::GetChipSnapshots => Ok(DebugResponse::ChipSnapshots(chip_snapshots(record))),
        DebugCommand::GetMachineMetadata => Ok(DebugResponse::MachineMetadata(MachineMetadata {
            machine_id: record.kind.machine_id().to_string(),
            model: Some(record.kind.machine_id().to_string()),
            region: Some(record.region.clone()),
            notes: vec!["external machine plugin v1".to_string()],
            bitmap_palette: bitmap_palette(),
            bitmap_defaults: Some(bitmap_defaults()),
        })),
    }
}

fn handle_instance_call(
    instance: MachineInstanceHandle,
    request: MachineInstanceCallRequest,
) -> Result<MachineInstanceCallResponse, String> {
    let mut state = state()
        .lock()
        .map_err(|_| "plugin state mutex poisoned".to_string())?;
    let record = state.record_mut(instance)?;
    let response = handle_machine_command(record, request.command)?;
    Ok(MachineInstanceCallResponse { response })
}

unsafe extern "C" fn plugin_free_buffer_v1(buffer: RcBuffer) {
    // SAFETY: caller guarantees this buffer originated from RcBuffer::from_vec.
    let _ = unsafe { buffer.into_vec() };
}

unsafe extern "C" fn plugin_list_profiles_v1(req: RcSlice) -> RcCallResult {
    plugin_api::guard::guarded_call("list_profiles", || unsafe {
        plugin_list_profiles_inner(req)
    })
}

unsafe fn plugin_list_profiles_inner(req: RcSlice) -> RcCallResult {
    let request: plugin_api::ListProfilesRequest = match decode_request(req) {
        Ok(req) => req,
        Err(e) => return e.into_call_result(),
    };
    let _ = request.include_experimental;

    let response = plugin_api::ListProfilesResponse {
        profiles: machine_profiles(),
    };
    encode_ok(&response)
}

unsafe extern "C" fn plugin_create_instance_v1(req: RcSlice) -> RcCallResult {
    plugin_api::guard::guarded_call("create_instance", || unsafe {
        plugin_create_instance_inner(req)
    })
}

unsafe fn plugin_create_instance_inner(req: RcSlice) -> RcCallResult {
    let request: MachineCreateRequest = match decode_request(req) {
        Ok(req) => req,
        Err(e) => return e.into_call_result(),
    };

    match create_machine(request) {
        Ok(response) => encode_ok(&response),
        Err(error) => RcCallResult::error_with_message(status_from_error(&error), &error),
    }
}

unsafe extern "C" fn plugin_destroy_instance_v1(instance: MachineInstanceHandle) -> RcStatus {
    plugin_api::guard::guarded(
        "destroy_instance",
        RcStatus::from_code(StatusCode::InternalError),
        || unsafe { plugin_destroy_instance_inner(instance) },
    )
}

unsafe fn plugin_destroy_instance_inner(instance: MachineInstanceHandle) -> RcStatus {
    let mut state = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return RcStatus::from_code(StatusCode::InternalError),
    };

    if state.instances.remove(&instance).is_some() {
        RcStatus::ok()
    } else {
        RcStatus::from_code(StatusCode::NotFound)
    }
}

unsafe extern "C" fn plugin_instance_call_v1(
    instance: MachineInstanceHandle,
    req: RcSlice,
) -> RcCallResult {
    plugin_api::guard::guarded_call("instance_call", || unsafe {
        plugin_instance_call_inner(instance, req)
    })
}

unsafe fn plugin_instance_call_inner(
    instance: MachineInstanceHandle,
    req: RcSlice,
) -> RcCallResult {
    let request: MachineInstanceCallRequest = match decode_request(req) {
        Ok(req) => req,
        Err(e) => return e.into_call_result(),
    };

    match handle_instance_call(instance, request) {
        Ok(response) => encode_ok(&response),
        Err(error) => RcCallResult::error_with_message(status_from_error(&error), &error),
    }
}

/// Fast path for `MachinePluginVTableV1::step` -- see that field's own doc
/// comment in `plugin-api`. No `MachineCommand`/bincode round trip at all.
extern "C" fn plugin_step_v1(instance: MachineInstanceHandle) -> u32 {
    if plugin_api::guard::has_crashed() {
        // A crashed machine idles like a halted CPU; 0 would spin the host.
        return 4;
    }
    plugin_api::guard::guarded("step", 4, || plugin_step_inner(instance))
}

fn plugin_step_inner(instance: MachineInstanceHandle) -> u32 {
    let Ok(mut state) = state().lock() else {
        return 0;
    };
    match state.instances.get_mut(&instance) {
        Some(record) => record.machine.step(),
        None => 0,
    }
}

/// Fast path for `MachinePluginVTableV1::halted`.
extern "C" fn plugin_halted_v1(instance: MachineInstanceHandle) -> u8 {
    if plugin_api::guard::has_crashed() {
        return 1;
    }
    plugin_api::guard::guarded("halted", 1, || plugin_halted_inner(instance))
}

fn plugin_halted_inner(instance: MachineInstanceHandle) -> u8 {
    let Ok(state) = state().lock() else {
        return 1;
    };
    match state.instances.get(&instance) {
        Some(record) => u8::from(record.machine.halted()),
        None => 1,
    }
}

/// Fast path for `MachinePluginVTableV1::pc`.
extern "C" fn plugin_pc_v1(instance: MachineInstanceHandle) -> u16 {
    plugin_api::guard::guarded("pc", 0, || plugin_pc_inner(instance))
}

fn plugin_pc_inner(instance: MachineInstanceHandle) -> u16 {
    let Ok(state) = state().lock() else {
        return 0;
    };
    match state.instances.get(&instance) {
        Some(record) => record.machine.pc(),
        None => 0,
    }
}

/// # Safety
///
/// Returns immutable descriptor storage owned by this plugin.
pub unsafe extern "C" fn entry_v1() -> *const PluginDescriptorV1 {
    &PLUGIN_DESCRIPTOR_V1.0
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_typed_character_belongs_to_a_listed_key() {
        let mut infos = key_names();
        assert!(plugin_api::set_key_chars(&mut infos, CPC_CHARS).is_empty());
        assert_eq!(
            infos
                .iter()
                .find(|k| k.name == "Comma")
                .map(|k| k.chars.as_str()),
            Some(",")
        );
    }

    #[test]
    fn key_names_list_everything_the_key_event_path_accepts() {
        let names = key_names();
        let listed: Vec<&str> = names.iter().map(|k| k.name.as_str()).collect();
        for expected in [
            "KeyA",
            "Joy1Fire1",
            "Joy2Up",
            "Tab",
            "CapsLock",
            "ControlLeft",
            "AnalogJoy1Left",
            "AnalogJoy2Down",
        ] {
            assert!(
                listed.contains(&expected),
                "{expected} missing from {listed:?}"
            );
        }
        // Unique (ignoring case/underscores), and each entry matches its own name.
        let mut seen = std::collections::HashSet::new();
        for key in &names {
            assert!(
                seen.insert(plugin_api::normalize_key_name(&key.name)),
                "{}",
                key.name
            );
            assert!(key.matches(&key.name));
        }
        // The free aliases work: `a` finds KeyA.
        assert!(names.iter().any(|k| k.name == "KeyA" && k.matches("a")));
    }

    use super::*;

    fn payload(result: RcCallResult) -> Vec<u8> {
        assert_eq!(result.status.code, StatusCode::Ok as u32);
        // SAFETY: tests consume payload exactly once.
        unsafe { result.payload.into_vec() }
    }

    fn create_request(machine_id: &str) -> MachineCreateRequest {
        let kind = CpcKind::from_machine_id(machine_id).expect("known machine id");
        let rom_len = if kind.has_internal_rom() {
            kind.model().rom_image_size()
        } else {
            cpc::ROM_SIZE // GX4000: one 16K cartridge bank is a valid minimum.
        };
        MachineCreateRequest {
            machine_id: machine_id.to_string(),
            region: Some("pal".to_string()),
            roms: vec![BinaryBlob {
                key: "rom".to_string(),
                bytes: vec![0; rom_len],
            }],
            options: Vec::new(),
        }
    }

    fn crtc_type_of(machine_id: &str, crtc_option: Option<u64>) -> Result<u8, String> {
        let mut request = create_request(machine_id);
        if let Some(value) = crtc_option {
            request.options.push(NamedValue {
                key: "crtc_type".to_string(),
                value: DebugValue::U64(value),
            });
        }
        let created = create_machine(request)?;
        let mut state = state().lock().unwrap();
        let record = state.instances.remove(&created.instance).unwrap();
        Ok(record.machine.crtc_type().index())
    }

    fn press(instance: MachineInstanceHandle, key: &str, pressed: bool) {
        let request = MachineInstanceCallRequest {
            command: MachineCommand::KeyEvent {
                key: key.to_string(),
                pressed,
            },
        };
        let bytes = plugin_api::encode_message(&request).expect("encode key event");
        // SAFETY: ABI function consumes request bytes only during call.
        let result = unsafe { plugin_instance_call_v1(instance, RcSlice::from_bytes(&bytes)) };
        payload(result);
    }

    fn keyboard_line(instance: MachineInstanceHandle, line: u8) -> u8 {
        let state = state().lock().unwrap();
        state.instances[&instance].machine.keyboard_row_state(line)
    }

    #[test]
    fn arrows_drive_joystick_1_alongside_the_cursor_keys_but_space_does_not_press_fire() {
        let created = create_machine(create_request("cpc464")).unwrap();
        let instance = created.instance;
        // ArrowLeft: cursor-left (line 1 bit 0) AND joystick 1 left (line 9 bit 2).
        press(instance, "ArrowLeft", true);
        assert_eq!(keyboard_line(instance, 1) & 0x01, 0);
        assert_eq!(keyboard_line(instance, 9) & 0x04, 0);
        press(instance, "ArrowLeft", false);
        assert_eq!(keyboard_line(instance, 1), 0xFF);
        assert_eq!(keyboard_line(instance, 9), 0xFF);
        // Space is only the space bar (line 5 bit 7): joystick 1 fire (line 9 bit 4) would make the firmware type an `X`.
        press(instance, "Space", true);
        assert_eq!(keyboard_line(instance, 5) & 0x80, 0);
        assert_eq!(keyboard_line(instance, 9), 0xFF);
        press(instance, "Space", false);
        assert_eq!(keyboard_line(instance, 5), 0xFF);
        state().lock().unwrap().instances.remove(&instance);
    }

    #[test]
    fn explicit_joystick_key_names_press_only_their_own_wires() {
        let created = create_machine(create_request("cpc6128")).unwrap();
        let instance = created.instance;
        press(instance, "Joy2Fire1", true);
        assert_eq!(keyboard_line(instance, 6), !0x10);
        assert_eq!(keyboard_line(instance, 9), 0xFF);
        press(instance, "Joy2Fire1", false);
        press(instance, "Joy1Fire2", true);
        assert_eq!(keyboard_line(instance, 9), !0x20);
        assert_eq!(keyboard_line(instance, 6), 0xFF);
        state().lock().unwrap().instances.remove(&instance);
    }

    fn send(instance: MachineInstanceHandle, command: MachineCommand) {
        let request = MachineInstanceCallRequest { command };
        let bytes = plugin_api::encode_message(&request).expect("encode command");
        // SAFETY: ABI function consumes request bytes only during call.
        let result = unsafe { plugin_instance_call_v1(instance, RcSlice::from_bytes(&bytes)) };
        payload(result);
    }

    /// `WriteIo` performs a real `OUT`: RAM configuration 2 maps the second
    /// 64K's block 5 at `&4000`, so what a flat read sees there changes.
    #[test]
    fn write_io_reaches_the_gate_array_through_the_plugin() {
        let instance = create_machine(create_request("cpc6128")).unwrap().instance;
        call_memory(
            instance,
            MachineCommand::WriteMemory {
                address: 0x4000,
                bytes: vec![0x11],
                space: None,
            },
        )
        .unwrap();
        call_memory(
            instance,
            MachineCommand::WriteMemory {
                address: 0,
                bytes: vec![0x22],
                space: Some("ram-bank:5".to_string()),
            },
        )
        .unwrap();
        let read = |instance| {
            call_memory(
                instance,
                MachineCommand::ReadMemory {
                    address: 0x4000,
                    len: 1,
                    space: None,
                },
            )
            .unwrap()
        };
        assert!(matches!(read(instance), MachineResponse::Memory(ref b) if b == &[0x11]));
        call_memory(
            instance,
            MachineCommand::WriteIo {
                port: 0x7F00,
                value: 0xC2,
            },
        )
        .unwrap();
        assert!(matches!(read(instance), MachineResponse::Memory(ref b) if b == &[0x22]));
        state().lock().unwrap().instances.remove(&instance);
    }

    #[test]
    fn amx_mouse_is_an_opt_in_option_on_every_model() {
        for kind in CpcKind::all() {
            let option = cpc_options(kind)
                .into_iter()
                .find(|option| option.key == "amx_mouse")
                .unwrap_or_else(|| panic!("{} declares amx_mouse", kind.machine_id()));
            assert!(matches!(option.kind, MachineOptionKind::Bool));
            assert!(matches!(option.default, Some(DebugValue::Bool(false))));
        }
    }

    #[test]
    fn host_mouse_events_drive_line_9_only_when_the_amx_mouse_is_enabled() {
        // Not enabled: motion and buttons change nothing.
        let off = create_machine(create_request("cpc6128")).unwrap().instance;
        send(off, MachineCommand::MouseMotion { dx: 5, dy: 0 });
        send(
            off,
            MachineCommand::MouseButton {
                button: HostMouseButton::Left,
                pressed: true,
            },
        );
        assert_eq!(keyboard_line(off, 9), 0xFF);
        state().lock().unwrap().instances.remove(&off);

        // Enabled through the creation option.
        let mut request = create_request("cpc6128");
        request.options.push(NamedValue {
            key: "amx_mouse".to_string(),
            value: DebugValue::Bool(true),
        });
        let on = create_machine(request).unwrap().instance;
        send(on, MachineCommand::MouseMotion { dx: -1, dy: 0 }); // left
        assert_eq!(keyboard_line(on, 9), !0x04, "left pulse is bit 2");
        send(
            on,
            MachineCommand::MouseButton {
                button: HostMouseButton::Right,
                pressed: true,
            },
        );
        assert_eq!(keyboard_line(on, 9), !(0x04 | 0x20), "right button = Fire1");
        send(
            on,
            MachineCommand::MouseButton {
                button: HostMouseButton::Right,
                pressed: false,
            },
        );
        send(
            on,
            MachineCommand::MouseButton {
                button: HostMouseButton::Other(7),
                pressed: true,
            },
        );
        assert_eq!(keyboard_line(on, 9), !0x04, "unmapped buttons are ignored");
        state().lock().unwrap().instances.remove(&on);
    }

    #[test]
    fn crtc_type_option_is_declared_for_every_model_without_a_forced_default() {
        for kind in CpcKind::all() {
            let option = cpc_options(kind)
                .into_iter()
                .find(|option| option.key == "crtc_type")
                .unwrap_or_else(|| panic!("{} declares crtc_type", kind.machine_id()));
            assert!(option.default.is_none(), "model default must apply");
            assert!(matches!(
                option.kind,
                MachineOptionKind::Int {
                    min: Some(0),
                    max: Some(4)
                }
            ));
        }
    }

    #[test]
    fn crtc_type_defaults_per_model_and_can_be_overridden() {
        assert_eq!(crtc_type_of("cpc464", None), Ok(0));
        assert_eq!(crtc_type_of("cpc664", None), Ok(0));
        assert_eq!(crtc_type_of("cpc6128", None), Ok(1));
        assert_eq!(crtc_type_of("cpc464plus", None), Ok(3));
        assert_eq!(crtc_type_of("gx4000", None), Ok(3));
        assert_eq!(crtc_type_of("cpc464", Some(2)), Ok(2));
        assert_eq!(crtc_type_of("cpc6128", Some(4)), Ok(4));
        assert!(
            crtc_type_of("cpc464", Some(5))
                .unwrap_err()
                .contains("out of range")
        );
    }

    #[test]
    fn descriptor_is_machine_plugin_v1() {
        let ptr = unsafe { entry_v1() };
        assert!(!ptr.is_null());
        // SAFETY: entry function returns pointer to static descriptor.
        let descriptor = unsafe { &*ptr };
        assert_eq!(descriptor.abi_major, ABI_MAJOR_V1);
        assert_eq!(descriptor.abi_minor, ABI_MINOR_V1);
        assert_eq!(descriptor.kind, PluginKind::Machine);
    }

    #[test]
    fn list_profiles_returns_all_cpc_models() {
        let req = plugin_api::ListProfilesRequest {
            include_experimental: false,
        };
        let req_bytes = plugin_api::encode_message(&req).expect("encode request");
        // SAFETY: ABI function consumes request bytes only during call.
        let result = unsafe { plugin_list_profiles_v1(RcSlice::from_bytes(&req_bytes)) };
        let bytes = payload(result);
        let response: plugin_api::ListProfilesResponse =
            plugin_api::decode_message(&bytes).expect("decode response");

        let ids: Vec<String> = response
            .profiles
            .into_iter()
            .map(|p| p.machine_id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "cpc464",
                "cpc664",
                "cpc6128",
                "cpc464plus",
                "cpc6128plus",
                "gx4000"
            ]
        );
    }

    #[test]
    fn create_instance_and_get_pc_round_trip() {
        let create_req =
            plugin_api::encode_message(&create_request("cpc464")).expect("encode create req");
        // SAFETY: ABI function consumes request bytes only during call.
        let create_result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&create_req)) };
        let create_payload = payload(create_result);
        let create: MachineCreateResponse =
            plugin_api::decode_message(&create_payload).expect("decode create response");

        let call_req = MachineInstanceCallRequest {
            command: MachineCommand::GetPc,
        };
        let call_req_bytes = plugin_api::encode_message(&call_req).expect("encode call request");
        // SAFETY: ABI function consumes request bytes only during call.
        let call_result = unsafe {
            plugin_instance_call_v1(create.instance, RcSlice::from_bytes(&call_req_bytes))
        };
        let call_payload = payload(call_result);
        let call: MachineInstanceCallResponse =
            plugin_api::decode_message(&call_payload).expect("decode call response");

        assert_eq!(call.response, MachineResponse::Pc(0));

        // SAFETY: instance handle came from create response above.
        let status = unsafe { plugin_destroy_instance_v1(create.instance) };
        assert!(status.is_ok());
    }

    #[test]
    fn attach_program_and_read_memory_round_trip() {
        let create_req =
            plugin_api::encode_message(&create_request("cpc464")).expect("encode create req");
        // SAFETY: ABI function consumes request bytes only during call.
        let create_result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&create_req)) };
        let create_payload = payload(create_result);
        let create: MachineCreateResponse =
            plugin_api::decode_message(&create_payload).expect("decode create response");

        let attach_req = MachineInstanceCallRequest {
            command: MachineCommand::AttachMedia {
                slot: plugin_api::MediaSlot::Other("program".to_string()),
                kind: MediaKind::Program,
                name: Some("program.bin".to_string()),
                bytes: vec![0x3E, 0x42],
                options: vec![NamedValue {
                    key: "org".to_string(),
                    value: DebugValue::U64(0x4000),
                }],
            },
        };
        let attach_bytes = plugin_api::encode_message(&attach_req).expect("encode attach request");
        // SAFETY: ABI function consumes request bytes only during call.
        let attach_result =
            unsafe { plugin_instance_call_v1(create.instance, RcSlice::from_bytes(&attach_bytes)) };
        let attach_payload = payload(attach_result);
        let attach_response: MachineInstanceCallResponse =
            plugin_api::decode_message(&attach_payload).expect("decode attach response");
        assert!(matches!(attach_response.response, MachineResponse::Ack));

        let read_req = MachineInstanceCallRequest {
            command: MachineCommand::ReadMemory {
                address: 0x4000,
                len: 2,
                space: None,
            },
        };
        let read_bytes = plugin_api::encode_message(&read_req).expect("encode read request");
        // SAFETY: ABI function consumes request bytes only during call.
        let read_result =
            unsafe { plugin_instance_call_v1(create.instance, RcSlice::from_bytes(&read_bytes)) };
        let read_payload = payload(read_result);
        let read_response: MachineInstanceCallResponse =
            plugin_api::decode_message(&read_payload).expect("decode read response");
        assert_eq!(
            read_response.response,
            MachineResponse::Memory(vec![0x3E, 0x42])
        );

        // SAFETY: instance handle came from create response above.
        let status = unsafe { plugin_destroy_instance_v1(create.instance) };
        assert!(status.is_ok());
    }

    /// The `"overlay"` chip carries both the small, cross-machine GUI-panel
    /// field vocabulary (`memory_summary`/`disk_motor`/`tape_motor`/
    /// `tape_waveform`) and CPC's own richer diagnostic telemetry
    /// (`tape_control_writes` among it) -- one chip, not the two
    /// (`"overlay"` + a CPC-only `"cpc.overlay"`) this project used to
    /// expose, see `overlay_chip_snapshot`'s own doc comment for why they
    /// were merged.
    #[test]
    fn debug_chip_snapshots_expose_overlay_telemetry() {
        let create_req =
            plugin_api::encode_message(&create_request("cpc464")).expect("encode create req");
        // SAFETY: ABI function consumes request bytes only during call.
        let create_result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&create_req)) };
        let create_payload = payload(create_result);
        let create: MachineCreateResponse =
            plugin_api::decode_message(&create_payload).expect("decode create response");

        let call_req = MachineInstanceCallRequest {
            command: MachineCommand::Debug(DebugCommand::GetChipSnapshots),
        };
        let call_req_bytes = plugin_api::encode_message(&call_req).expect("encode call request");
        // SAFETY: ABI function consumes request bytes only during call.
        let call_result = unsafe {
            plugin_instance_call_v1(create.instance, RcSlice::from_bytes(&call_req_bytes))
        };
        let call_payload = payload(call_result);
        let call: MachineInstanceCallResponse =
            plugin_api::decode_message(&call_payload).expect("decode call response");

        let MachineResponse::Debug(DebugResponse::ChipSnapshots(snapshots)) = call.response else {
            panic!("expected debug chip snapshots response");
        };
        assert_eq!(
            snapshots.len(),
            1,
            "cpc exposes exactly one overlay chip, not a separate cpc-only one"
        );
        let overlay = snapshots
            .iter()
            .find(|chip| chip.chip_id == "overlay")
            .expect("overlay snapshot");
        assert!(
            overlay
                .fields
                .iter()
                .any(|f| f.key == "tape_control_writes"),
            "overlay fields must include tape control counters"
        );
        assert!(
            overlay.fields.iter().any(|f| f.key == "memory_summary"),
            "overlay fields must include a memory_summary line"
        );
        assert!(
            overlay.fields.iter().any(|f| f.key == "disk_motor"),
            "CPC464 has a cassette motor field but no disk -- disk_motor is still always present \
             since every CPC model reports it, disk hardware just stays idle without an FDC"
        );
        assert!(
            overlay.fields.iter().any(|f| f.key == "tape_motor"),
            "overlay fields must include tape_motor"
        );
        assert!(
            overlay.blobs.iter().any(|b| b.key == "tape_waveform"),
            "overlay blobs must include recent tape waveform"
        );

        // SAFETY: instance handle came from create response above.
        let status = unsafe { plugin_destroy_instance_v1(create.instance) };
        assert!(status.is_ok());
    }

    fn call_memory(
        instance: MachineInstanceHandle,
        command: MachineCommand,
    ) -> Result<MachineResponse, String> {
        let request = MachineInstanceCallRequest { command };
        let bytes = plugin_api::encode_message(&request).expect("encode request");
        // SAFETY: ABI function consumes request bytes only during call.
        let result = unsafe { plugin_instance_call_v1(instance, RcSlice::from_bytes(&bytes)) };
        if result.status.code == StatusCode::Ok as u32 {
            Ok(plugin_api::decode_message(&payload(result)).expect("decode response"))
        } else {
            Err(format!("status {}", result.status.code))
        }
    }

    /// A `space` the plugin doesn't understand, or a block the machine
    /// doesn't have, must be a clear error -- never a silent flat access that
    /// would poke the wrong memory.
    #[test]
    fn unknown_or_out_of_range_spaces_are_clear_errors() {
        let instance = create_machine(create_request("cpc464")).unwrap().instance;
        for space in [
            "video",
            "ram-bank:",
            "ram-bank:x",
            "ram-bank:4",
            "ram-bank:200",
        ] {
            let write = call_memory(
                instance,
                MachineCommand::WriteMemory {
                    address: 0x10,
                    bytes: vec![0x42],
                    space: Some(space.to_string()),
                },
            );
            assert!(write.is_err(), "write to {space:?} should be rejected");
            let read = call_memory(
                instance,
                MachineCommand::ReadMemory {
                    address: 0x10,
                    len: 1,
                    space: Some(space.to_string()),
                },
            );
            assert!(read.is_err(), "read of {space:?} should be rejected");
        }
        // Offsets past the 16K block are rejected too.
        assert!(
            call_memory(
                instance,
                MachineCommand::WriteMemory {
                    address: 0x4000,
                    bytes: vec![1],
                    space: Some("ram-bank:0".to_string()),
                },
            )
            .is_err()
        );
        state().lock().unwrap().instances.remove(&instance);
    }

    /// `ram-bank:N` reaches physical RAM blocks on the 6128, including the
    /// ones the CPU doesn't currently have paged in, and expansion blocks.
    #[test]
    fn ram_bank_space_reads_and_writes_physical_blocks() {
        let mut request = create_request("cpc6128");
        request.options.push(NamedValue {
            key: "ram_banks".to_string(),
            value: DebugValue::U64(1),
        });
        let instance = create_machine(request).unwrap().instance;

        for (block, offset) in [(5u8, 0x0123u16), (11, 0x3FFE)] {
            let space = Some(format!("ram-bank:{block}"));
            let written = call_memory(
                instance,
                MachineCommand::WriteMemory {
                    address: offset,
                    bytes: vec![0xDE, 0xAD],
                    space: space.clone(),
                },
            )
            .expect("banked write");
            // (11, 0x3FFE) ends exactly at the block boundary: both bytes fit.
            assert!(matches!(written, MachineResponse::BytesWritten(2)));
            let read = call_memory(
                instance,
                MachineCommand::ReadMemory {
                    address: offset,
                    len: 2,
                    space,
                },
            )
            .expect("banked read");
            assert!(
                matches!(read, MachineResponse::Memory(ref b) if b == &[0xDE, 0xAD]),
                "block {block}: {read:?}"
            );
        }

        // Block 5 isn't mapped at &0123 on a freshly reset machine, so a flat
        // read there does not see the banked write.
        let flat = call_memory(
            instance,
            MachineCommand::ReadMemory {
                address: 0x4123,
                len: 2,
                space: None,
            },
        )
        .expect("flat read");
        assert!(matches!(flat, MachineResponse::Memory(ref b) if b != &[0xDE, 0xAD]));

        // The expansion board adds blocks 8-11.
        let blocks = state().lock().unwrap().instances[&instance]
            .machine
            .ram_block_count();
        assert_eq!(blocks, 12);
        state().lock().unwrap().instances.remove(&instance);
    }

    #[test]
    fn gx4000_instance_creates_and_renders_a_frame() {
        let create_req =
            plugin_api::encode_message(&create_request("gx4000")).expect("encode create req");
        // SAFETY: ABI function consumes request bytes only during call.
        let create_result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&create_req)) };
        let create_payload = payload(create_result);
        let create: MachineCreateResponse =
            plugin_api::decode_message(&create_payload).expect("decode create response");

        let call_req = MachineInstanceCallRequest {
            command: MachineCommand::RenderFrame,
        };
        let call_req_bytes = plugin_api::encode_message(&call_req).expect("encode call request");
        // SAFETY: ABI function consumes request bytes only during call.
        let call_result = unsafe {
            plugin_instance_call_v1(create.instance, RcSlice::from_bytes(&call_req_bytes))
        };
        let call_payload = payload(call_result);
        let call: MachineInstanceCallResponse =
            plugin_api::decode_message(&call_payload).expect("decode call response");

        match call.response {
            MachineResponse::Frame { width, height, .. } => {
                // The fixed 768x576 (4:3, scanline-doubled) monitor window
                // every CPC model renders.
                assert_eq!((width, height), (768, 576));
            }
            other => panic!("expected a rendered frame, got {other:?}"),
        }

        // SAFETY: instance handle came from create response above.
        let status = unsafe { plugin_destroy_instance_v1(create.instance) };
        assert!(status.is_ok());
    }

    #[test]
    fn gx4000_rejects_tape_media() {
        let create_req =
            plugin_api::encode_message(&create_request("gx4000")).expect("encode create req");
        // SAFETY: ABI function consumes request bytes only during call.
        let create_result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&create_req)) };
        let create: MachineCreateResponse =
            plugin_api::decode_message(&payload(create_result)).expect("decode create response");

        let mut state = state().lock().unwrap();
        let record = state.instances.get_mut(&create.instance).unwrap();
        let result = attach_media(record, MediaKind::Tape, None, Vec::new(), Vec::new());
        drop(state);
        assert!(result.is_err());

        // SAFETY: instance handle came from create response above.
        let status = unsafe { plugin_destroy_instance_v1(create.instance) };
        assert!(status.is_ok());
    }

    #[test]
    fn analog_joystick_key_event_reaches_the_asic() {
        let create_req =
            plugin_api::encode_message(&create_request("cpc6128plus")).expect("encode create req");
        // SAFETY: ABI function consumes request bytes only during call.
        let create_result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&create_req)) };
        let create: MachineCreateResponse =
            plugin_api::decode_message(&payload(create_result)).expect("decode create response");

        let key_req = MachineInstanceCallRequest {
            command: MachineCommand::KeyEvent {
                key: "AnalogJoy1Right".to_string(),
                pressed: true,
            },
        };
        let key_req_bytes = plugin_api::encode_message(&key_req).expect("encode key request");
        // SAFETY: ABI function consumes request bytes only during call.
        let key_result = unsafe {
            plugin_instance_call_v1(create.instance, RcSlice::from_bytes(&key_req_bytes))
        };
        let _ = payload(key_result); // must not error

        // SAFETY: instance handle came from create response above.
        let status = unsafe { plugin_destroy_instance_v1(create.instance) };
        assert!(status.is_ok());
    }
    #[test]
    fn gx4000_cpr_container_creates_machine_from_declared_bank() {
        let cartridge = cpc_cartridge::Cartridge {
            pages: vec![cpc_cartridge::CartridgePage {
                bank: 3,
                data: vec![0xA5; cpc_cartridge::PAGE_SIZE],
            }],
        };
        let request = MachineCreateRequest {
            machine_id: "gx4000".to_string(),
            region: Some("pal".to_string()),
            roms: vec![BinaryBlob {
                key: "rom".to_string(),
                bytes: cpc_cartridge::write(&cartridge).unwrap(),
            }],
            options: Vec::new(),
        };
        let bytes = plugin_api::encode_message(&request).unwrap();
        let result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&bytes)) };
        let create: MachineCreateResponse = plugin_api::decode_message(&payload(result)).unwrap();
        assert_eq!(create.metadata.machine_id, "gx4000");
        let status = unsafe { plugin_destroy_instance_v1(create.instance) };
        assert!(status.is_ok());
    }

    #[test]
    fn cpc_sna_snapshot_save_and_attach_round_trip_through_plugin() {
        let create_req = plugin_api::encode_message(&create_request("cpc464")).unwrap();
        let create_result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&create_req)) };
        let source: MachineCreateResponse =
            plugin_api::decode_message(&payload(create_result)).unwrap();

        let save_req = MachineInstanceCallRequest {
            command: MachineCommand::SaveSnapshot {
                extension: "sna".to_string(),
            },
        };
        let save_bytes = plugin_api::encode_message(&save_req).unwrap();
        let save_result =
            unsafe { plugin_instance_call_v1(source.instance, RcSlice::from_bytes(&save_bytes)) };
        let save_response: MachineInstanceCallResponse =
            plugin_api::decode_message(&payload(save_result)).unwrap();
        let MachineResponse::SnapshotBlob(snapshot) = save_response.response else {
            panic!("expected CPC SNA snapshot blob");
        };

        let target_req = plugin_api::encode_message(&create_request("cpc464")).unwrap();
        let target_result = unsafe { plugin_create_instance_v1(RcSlice::from_bytes(&target_req)) };
        let target: MachineCreateResponse =
            plugin_api::decode_message(&payload(target_result)).unwrap();
        let attach_req = MachineInstanceCallRequest {
            command: MachineCommand::AttachMedia {
                slot: plugin_api::MediaSlot::Snapshot,
                kind: MediaKind::Snapshot,
                name: Some("state.sna".to_string()),
                bytes: snapshot,
                options: Vec::new(),
            },
        };
        let attach_bytes = plugin_api::encode_message(&attach_req).unwrap();
        let attach_result =
            unsafe { plugin_instance_call_v1(target.instance, RcSlice::from_bytes(&attach_bytes)) };
        let attach_response: MachineInstanceCallResponse =
            plugin_api::decode_message(&payload(attach_result)).unwrap();
        assert!(matches!(attach_response.response, MachineResponse::Ack));
        assert!(unsafe { plugin_destroy_instance_v1(source.instance) }.is_ok());
        assert!(unsafe { plugin_destroy_instance_v1(target.instance) }.is_ok());
    }

    #[test]
    fn detach_media_ejects_the_disk_and_the_tape_and_refuses_other_slots() {
        let created = create_machine(create_request("cpc664")).unwrap();
        let mut record = state()
            .lock()
            .unwrap()
            .instances
            .remove(&created.instance)
            .unwrap();
        let attach_disk = MachineCommand::AttachMedia {
            slot: plugin_api::MediaSlot::Disk { drive: 0 },
            kind: MediaKind::Disk,
            name: Some("blank.dsk".to_string()),
            bytes: dsk::Disk::new(1).to_bytes(),
            options: Vec::new(),
        };
        let export = || MachineCommand::ExportDisk {
            drive: 0,
            only_if_modified: false,
        };

        handle_machine_command(&mut record, attach_disk.clone()).unwrap();
        assert!(matches!(
            handle_machine_command(&mut record, export()),
            Ok(MachineResponse::DiskImage(Some(_)))
        ));
        fn detach(
            record: &mut InstanceRecord,
            slot: plugin_api::MediaSlot,
        ) -> Result<MachineResponse, String> {
            handle_machine_command(record, MachineCommand::DetachMedia { slot })
        }
        assert!(matches!(
            detach(&mut record, plugin_api::MediaSlot::Disk { drive: 0 }),
            Ok(MachineResponse::Ack)
        ));
        assert!(!record.disk_attached);
        assert!(matches!(
            handle_machine_command(&mut record, export()),
            Ok(MachineResponse::DiskImage(None))
        ));
        // A swap is detach + attach: the new disk is in the drive.
        handle_machine_command(&mut record, attach_disk).unwrap();
        assert!(record.disk_attached);

        // The cassette: eject works with or without a tape in the deck.
        assert!(matches!(
            detach(&mut record, plugin_api::MediaSlot::Tape),
            Ok(MachineResponse::Ack)
        ));
        assert!(!record.machine.has_tape());
        // Anything else is a clear refusal.
        assert!(detach(&mut record, plugin_api::MediaSlot::Disk { drive: 1 }).is_err());
        assert!(detach(&mut record, plugin_api::MediaSlot::Cartridge { slot: 0 }).is_err());
    }

    /// `firmware.toml` (next to the family's `Cargo.toml`) tells `--firmware auto` which file fills which machine
    /// option. An entry naming an option this plugin does not declare as a file option can never apply: this is
    /// exactly how the C64/C128 manifest once matched nothing (`rom_kernal` against the declared `kernal`).
    #[test]
    fn firmware_toml_names_only_file_options_the_plugin_declares() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../firmware.toml");
        let text = std::fs::read_to_string(path).expect("the family's firmware.toml");
        let manifest: toml::Value = text.parse().expect("firmware.toml is valid TOML");
        let profiles = machine_profiles();
        let mut checked = 0;
        for entry in manifest["rom"].as_array().expect("[[rom]] entries") {
            let option = entry["option"].as_str().expect("option = \"..\"");
            for machine in entry["machines"].as_array().expect("machines = [..]") {
                let id = machine.as_str().expect("a machine id");
                // Another plugin of the family serves the machines this one does not.
                let Some(profile) = profiles.iter().find(|p| p.machine_id == id) else {
                    continue;
                };
                let declared: Vec<&str> = profile
                    .options
                    .iter()
                    .filter(|o| o.kind == plugin_api::MachineOptionKind::File)
                    .map(|o| o.key.as_str())
                    .collect();
                assert!(
                    declared.contains(&option),
                    "firmware.toml: \"{}\" names option \"{option}\" for {id}, which declares the file options {declared:?}",
                    entry["label"].as_str().unwrap_or("?")
                );
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "no firmware.toml entry applies to this plugin's machines"
        );
    }

    /// The CPC firmware's key translation tables: ten lines of eight bits, normal at ROM 0x1D69 and shifted (right after it) at
    /// 0x1DB9. They are what BASIC and the firmware turn a pressed matrix position into, i.e. the hardware's own
    /// key-to-character map; `Some` only when the ROM file is present.
    fn firmware_key_tables() -> Option<(Vec<u8>, Vec<u8>)> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../roms/cpc/cpc464.rom");
        let rom = std::fs::read(path).ok()?;
        Some((rom[0x1D69..0x1DB9].to_vec(), rom[0x1DB9..0x1E09].to_vec()))
    }

    #[test]
    fn every_character_a_key_claims_is_what_the_firmware_makes_of_its_matrix_position() {
        let Some((normal, shifted)) = firmware_key_tables() else {
            eprintln!("skipping: roms/cpc/cpc464.rom is not present");
            return;
        };
        let at = |(line, bit): (u8, u8)| usize::from(line) * 8 + usize::from(bit);
        let mut checked = 0;
        for (name, text) in CPC_CHARS {
            let wanted = text.chars().next().unwrap();
            // The firmware's byte for the character (the pound sign is 0xA3 there).
            let code = if wanted == '\u{a3}' {
                0xA3
            } else {
                wanted as u8
            };
            let got = if let Some((modifier, base)) = cpc_combo_from_name(name) {
                assert_eq!(
                    modifier,
                    CpcKey::Shift,
                    "{name} combines Shift with its key"
                );
                shifted[at(base.position())]
            } else {
                let key = cpc_key_from_name(name).unwrap_or_else(|| panic!("{name} is not a key"));
                normal[at(key.position())]
            };
            assert_eq!(
                got, code,
                "{name}: the firmware types {got:#04x}, the plugin says {wanted:?}"
            );
            checked += 1;
        }
        assert!(checked >= 30, "only {checked} characters were checked");
    }

    #[test]
    fn the_function_keys_are_the_ones_the_firmware_expands() {
        let Some((normal, _)) = firmware_key_tables() else {
            return;
        };
        // The firmware's expansion tokens: F0-F9 are 0x80-0x89, the `.` key 0x8A, small Enter 0x8B.
        for n in 0..10u8 {
            let key = cpc_key_from_name(&format!("F{n}")).unwrap();
            let (line, bit) = key.position();
            assert_eq!(
                normal[usize::from(line) * 8 + usize::from(bit)],
                0x80 + n,
                "F{n}"
            );
        }
        let (line, bit) = cpc_key_from_name("FDot").unwrap().position();
        assert_eq!(normal[usize::from(line) * 8 + usize::from(bit)], 0x8A);
    }

    #[test]
    fn a_shifted_symbol_presses_shift_and_its_key_together() {
        let created = create_machine(create_request("cpc464")).unwrap();
        let instance = created.instance;
        // The double quote is Shift (line 2, bit 5) with `2` (line 8, bit 1).
        press(instance, "DoubleQuote", true);
        assert_eq!(keyboard_line(instance, 2) & 0x20, 0, "Shift");
        assert_eq!(keyboard_line(instance, 8) & 0x02, 0, "the 2 key");
        press(instance, "DoubleQuote", false);
        assert_eq!(keyboard_line(instance, 2), 0xFF);
        assert_eq!(keyboard_line(instance, 8), 0xFF);
        // A plain punctuation key: the comma is line 4, bit 7 (it was swapped with the full stop once).
        press(instance, "Comma", true);
        assert_eq!(keyboard_line(instance, 4) & 0x80, 0);
        press(instance, "Comma", false);
        press(instance, "Period", true);
        assert_eq!(keyboard_line(instance, 3) & 0x80, 0);
    }
}
