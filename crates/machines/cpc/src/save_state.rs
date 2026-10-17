//! Internal save-state encode/decode for `Machine::save_state`/
//! `load_state` -- same bincode v2 serde-compat convention as `c64`/
//! `zx_spectrum`/`zx81`. Plain hardware state derives serde directly;
//! the FDC and cassette playback position use `nec765::FdcState`/`zx_tape::
//! TapePosition` so externally supplied disk/tape bytes stay external.

use crate::Cpc;

#[derive(serde::Serialize, serde::Deserialize)]
struct SaveState {
    model: crate::Model,
    cpu: z80::Cpu,
    ram: Vec<u8>,
    gate_array: crate::gate_array::GateArray,
    crtc: crate::crtc::Crtc,
    keyboard: crate::keyboard::KeyMatrix,
    ay: ay_3_8910::Ay3_8910,
    audio_cycle_accumulator: f64,
    ay_cycle_debt: u32,
    bus_tstate_phase: u8,
    ram_configuration: u8,
    ram_bank_select: u8,
    /// 64K Standard Memory Expansion banks beyond the base onboard
    /// second 64K -- `0` on every model without `has_128k_paging()`,
    /// same real-hardware-fact gate `CpcBus::set_additional_ram_banks`
    /// applies. Restored before the RAM-length check below so a save
    /// state captures its own expansion size, unlike externally
    /// supplied disk/tape media which must already be attached.
    additional_ram_banks: u8,
    upper_rom_select: u8,
    disk_motor: bool,
    cassette_motor: bool,
    tape_motor_mode: crate::TapeMotorMode,
    tape_position: Option<zx_tape::TapePosition>,
    fdc: Option<nec765::FdcState>,
    /// The disk serialised as a `.dsk` file, present only once the FDC has
    /// written to it -- a disk nobody wrote to is exactly the externally
    /// supplied image, so it stays external like ROM/tape bytes.
    modified_disk: Option<Vec<u8>>,
    /// The same for drive B.
    modified_disk_b: Option<Vec<u8>>,
    /// CPC Plus/GX4000 ASIC state -- `None` for every classic model. The
    /// cartridge image itself stays external, same `--rom` convention as
    /// every other machine here.
    asic: Option<crate::asic::Asic>,
}

pub(crate) fn encode(cpc: &Cpc) -> Vec<u8> {
    let state = SaveState {
        cpu: cpc.cpu,
        model: cpc.model(),
        ram: cpc.bus.ram().to_vec(),
        gate_array: cpc.bus.gate_array.clone(),
        crtc: cpc.bus.crtc.clone(),
        keyboard: cpc.bus.keyboard.clone(),
        ay: cpc.bus.ay.clone(),
        audio_cycle_accumulator: cpc.audio_cycle_accumulator,
        ay_cycle_debt: cpc.ay_cycle_debt,
        bus_tstate_phase: cpc.bus.bus_tstate_phase(),
        ram_configuration: cpc.bus.ram_configuration(),
        ram_bank_select: cpc.bus.ram_bank_select(),
        additional_ram_banks: cpc.bus.additional_ram_banks(),
        upper_rom_select: cpc.bus.upper_rom_select(),
        disk_motor: cpc.bus.disk_motor(),
        cassette_motor: cpc.bus.cassette_motor(),
        tape_motor_mode: cpc.bus.tape_motor_mode(),
        tape_position: cpc.bus.tape_position(),
        fdc: cpc.bus.fdc_state(),
        modified_disk: cpc.bus.modified_disk_image(),
        modified_disk_b: cpc.bus.modified_disk_image_of(1),
        asic: cpc.bus.asic_state(),
    };
    bincode::serde::encode_to_vec(&state, bincode::config::standard())
        .expect("CPC save state should always be encodable")
}

pub(crate) fn decode(cpc: &mut Cpc, bytes: &[u8]) -> Result<(), String> {
    let (state, _): (SaveState, usize) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard())
            .map_err(|e| format!("failed to decode CPC save state: {e}"))?;

    if state.model != cpc.model() {
        return Err(format!(
            "save state model mismatch: state is {}, machine is {}",
            state.model.name(),
            cpc.model().name()
        ));
    }

    cpc.bus.set_additional_ram_banks(state.additional_ram_banks);

    if state.ram.len() != cpc.bus.ram().len() {
        return Err(format!(
            "save state RAM size mismatch: expected {}, got {}",
            cpc.bus.ram().len(),
            state.ram.len()
        ));
    }

    cpc.cpu = state.cpu;
    cpc.bus.ram_mut().copy_from_slice(&state.ram);
    cpc.bus.gate_array = state.gate_array;
    cpc.bus.crtc = state.crtc;
    cpc.bus.keyboard = state.keyboard;
    cpc.bus.ay = state.ay;
    cpc.audio_cycle_accumulator = state.audio_cycle_accumulator;
    cpc.ay_cycle_debt = state.ay_cycle_debt;
    cpc.bus.set_bus_tstate_phase(state.bus_tstate_phase);
    cpc.bus.set_ram_configuration(state.ram_configuration);
    cpc.bus.set_ram_bank_select(state.ram_bank_select);
    cpc.bus.set_upper_rom_select(state.upper_rom_select);
    cpc.bus.set_disk_motor(state.disk_motor);
    cpc.bus.set_tape_motor_mode(state.tape_motor_mode);
    cpc.bus.restore_disk_image(state.modified_disk)?;
    cpc.bus.restore_disk_image_of(1, state.modified_disk_b)?;
    cpc.bus.restore_fdc_state(state.fdc);
    cpc.bus.set_cassette_motor(state.cassette_motor);
    cpc.bus.restore_tape_position(state.tape_position);
    cpc.bus.restore_asic_state(state.asic);
    cpc.resync_video();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use machine::{Bus, Machine};

    fn minimal_tzx_pause_block(ms: u16) -> zx_tape::Tape {
        let mut bytes = b"ZXTape!\x1A".to_vec();
        bytes.push(1); // major
        bytes.push(20); // minor
        bytes.push(0x20); // pause/stop block
        bytes.extend_from_slice(&ms.to_le_bytes());
        zx_tape::parse_tzx(&bytes).unwrap()
    }
    fn save_state_round_trips_cpu_and_memory() {
        let mut cpc = Cpc::new(vec![0; crate::ROM_IMAGE_SIZE_464], crate::Model::Cpc464).unwrap();
        cpc.bus.write(0x4000, 0x42);
        cpc.set_pc(0x1234);

        let saved = cpc.save_state();

        let mut restored =
            Cpc::new(vec![0; crate::ROM_IMAGE_SIZE_464], crate::Model::Cpc464).unwrap();
        restored.load_state(&saved).unwrap();

        assert_eq!(restored.pc(), 0x1234);
        assert_eq!(restored.read_memory(0x4000), 0x42);
    }

    fn save_state_rejects_a_different_cpc_model() {
        let source = Cpc::new(
            vec![0; crate::ROM_IMAGE_SIZE_664_6128],
            crate::Model::Cpc664,
        )
        .unwrap();
        let mut target = Cpc::new(
            vec![0; crate::ROM_IMAGE_SIZE_664_6128],
            crate::Model::Cpc6128,
        )
        .unwrap();

        assert!(target.load_state(&source.save_state()).is_err());
    }
    /// Merged (every check kept): `save_state_round_trips_cpu_and_memory`, `save_state_rejects_a_different_cpc_model`.
    #[test]
    fn save_state_basics() {
        save_state_round_trips_cpu_and_memory();
        save_state_rejects_a_different_cpc_model();
    }

    fn save_state_round_trips_asic_state_for_plus_models() {
        let mut cpc =
            Cpc::new(vec![0; crate::ROM_IMAGE_SIZE_464], crate::Model::Cpc464Plus).unwrap();
        for byte in [
            0xffu8, 0x77, 0xb3, 0x51, 0xa8, 0xd4, 0x62, 0x39, 0x9c, 0x46, 0x2b, 0x15, 0x8a, 0xcd,
            0xee,
        ] {
            cpc.bus.write_io(0xBC00, byte);
        }
        cpc.bus.write_io(0x7F00, 0xB8); // ASIC registers visible at &4000
        cpc.bus.write(0x6800, 77); // PRI register
        assert_eq!(cpc.bus.asic_state().unwrap().pri(), 77);

        let saved = cpc.save_state();
        let mut restored =
            Cpc::new(vec![0; crate::ROM_IMAGE_SIZE_464], crate::Model::Cpc464Plus).unwrap();
        restored.load_state(&saved).unwrap();

        let restored_asic = restored.bus.asic_state().unwrap();
        assert!(restored_asic.enabled());
        assert_eq!(restored_asic.pri(), 77);
    }

    fn save_state_round_trips_gx4000_asic_state_without_a_classic_asic_mismatch() {
        let source = Cpc::new(vec![0; crate::ROM_SIZE], crate::Model::Gx4000).unwrap();
        let mut target = Cpc::new(vec![0; crate::ROM_SIZE], crate::Model::Gx4000).unwrap();
        target.load_state(&source.save_state()).unwrap();
        assert!(!target.bus.asic_state().unwrap().enabled());
    }
    /// Merged (every check kept): `save_state_round_trips_asic_state_for_plus_models`, `save_state_round_trips_gx4000_asic_state_without_a_classic_asic_mismatch`.
    #[test]
    fn save_state_asic() {
        save_state_round_trips_asic_state_for_plus_models();
        save_state_round_trips_gx4000_asic_state_without_a_classic_asic_mismatch();
    }

    #[test]
    fn save_state_round_trips_6128_paging_motor_and_fdc_phase() {
        let mut cpc = Cpc::new(
            vec![0; crate::ROM_IMAGE_SIZE_664_6128],
            crate::Model::Cpc6128,
        )
        .unwrap();
        cpc.bus.gate_array.write(0x8D); // disable both ROM overlays
        cpc.bus.write_io(0x7F00, 0xC2); // all four secondary banks
        cpc.bus.write(0x0000, 0x42);
        cpc.bus.write_io(0xFA7E, 1);
        cpc.bus.write_io(0xFB7F, 0x0F); // Seek: no disk, so not ready
        cpc.bus.write_io(0xFB7F, 0x00);
        cpc.bus.write_io(0xFB7F, 0x05);
        for _ in 0..100 {
            cpc.step(); // let the (timed) seek finish
        }
        cpc.bus.write_io(0xFB7F, 0x08); // Sense Interrupt Status result pending

        let saved = cpc.save_state();
        let mut restored = Cpc::new(
            vec![0; crate::ROM_IMAGE_SIZE_664_6128],
            crate::Model::Cpc6128,
        )
        .unwrap();
        restored.load_state(&saved).unwrap();

        assert_eq!(restored.read_memory(0x0000), 0x42);
        assert!(restored.bus.disk_motor());
        assert_eq!(restored.bus.read_io(0xFB7E), 0xD0, "result phase survives");
        assert_eq!(restored.bus.read_io(0xFB7F), 0x68, "seek end + not ready");
        assert_eq!(restored.bus.read_io(0xFB7F), 0x00);
        assert_eq!(restored.bus.read_io(0xFB7E), 0x80);
    }

    fn blank_disk() -> dsk::Disk {
        let mut disk = dsk::Disk::new(1);
        let ids: Vec<[u8; 4]> = (1..=9).map(|r| [0, 0, 0xC0 + r, 2]).collect();
        assert!(disk.format_track(0, 0, &ids, 0x4E, 0xE5));
        disk.clear_modified();
        disk
    }

    fn write_sector_via_ports(cpc: &mut Cpc, fill: u8) {
        cpc.bus.write_io(0xFA7E, 1); // motor on
        for b in [0x45, 0, 0, 0, 0xC1, 2, 0xC1, 0x2A, 0xFF] {
            cpc.bus.write_io(0xFB7F, b); // Write Data, MFM
        }
        cpc.bus.read_io(0xFB7E); // the one not-ready poll
        for _ in 0..512 {
            cpc.bus.write_io(0xFB7F, fill);
        }
    }

    fn sector_c1(cpc: &Cpc) -> Vec<u8> {
        cpc.disk()
            .unwrap()
            .read_sector(0, 0, 0xC1)
            .unwrap()
            .data
            .clone()
    }

    fn save_state_carries_a_modified_disk_and_reverts_to_the_original_for_a_clean_one() {
        let rom = || vec![0; crate::ROM_IMAGE_SIZE_664_6128];
        let mut cpc = Cpc::new(rom(), crate::Model::Cpc6128).unwrap();
        cpc.load_disk(blank_disk()).unwrap();

        let clean = cpc.save_state();
        write_sector_via_ports(&mut cpc, 0x77);
        assert_eq!(sector_c1(&cpc), vec![0x77; 512]);
        let dirty = cpc.save_state();

        // Going back to a state from before the write puts the original back.
        cpc.load_state(&clean).unwrap();
        assert_eq!(
            sector_c1(&cpc),
            vec![0xE5; 512],
            "write undone by the older state"
        );
        assert!(!cpc.disk().unwrap().is_modified());

        // And forward again to the state that had the write.
        cpc.load_state(&dirty).unwrap();
        assert_eq!(sector_c1(&cpc), vec![0x77; 512]);
        assert!(cpc.disk().unwrap().is_modified());

        // A different machine with the *original* image inserted picks the
        // modified disk up from the state alone.
        let mut other = Cpc::new(rom(), crate::Model::Cpc6128).unwrap();
        other.load_disk(blank_disk()).unwrap();
        other.load_state(&dirty).unwrap();
        assert_eq!(sector_c1(&other), vec![0x77; 512]);
    }

    fn a_clean_disk_adds_nothing_to_the_save_state() {
        let rom = || vec![0; crate::ROM_IMAGE_SIZE_664_6128];
        let mut with_disk = Cpc::new(rom(), crate::Model::Cpc6128).unwrap();
        with_disk.load_disk(blank_disk()).unwrap();
        let without = Cpc::new(rom(), crate::Model::Cpc6128).unwrap();
        assert_eq!(
            with_disk.save_state().len(),
            without.save_state().len(),
            "an unwritten disk must stay external"
        );
    }
    /// Merged (every check kept): `save_state_carries_a_modified_disk_and_reverts_to_the_original_for_a_clean_one`, `a_clean_disk_adds_nothing_to_the_save_state`.
    #[test]
    fn save_state_disk() {
        save_state_carries_a_modified_disk_and_reverts_to_the_original_for_a_clean_one();
        a_clean_disk_adds_nothing_to_the_save_state();
    }

    #[test]
    fn save_state_round_trips_cassette_motor_and_tape_position() {
        let mut cpc = Cpc::new(vec![0; crate::ROM_IMAGE_SIZE_464], crate::Model::Cpc464).unwrap();
        cpc.load_tape(minimal_tzx_pause_block(1));
        cpc.set_tape_speed(2);
        cpc.bus.write_io(0xF700, 0x82); // PA/PC output mode
        cpc.bus.write_io(0xF700, 0x09); // bit-set Port C bit 4 -> motor on
        for _ in 0..150 {
            cpc.step();
        }

        let expected_cassette_in = cpc.bus.read_io(0xF500) & 0x80;
        let saved = cpc.save_state();
        let mut restored =
            Cpc::new(vec![0; crate::ROM_IMAGE_SIZE_464], crate::Model::Cpc464).unwrap();
        restored.load_tape(minimal_tzx_pause_block(1));
        restored.load_state(&saved).unwrap();

        assert!(restored.bus.cassette_motor());
        assert_eq!(restored.bus.read_io(0xF500) & 0x80, expected_cassette_in);
    }
}
