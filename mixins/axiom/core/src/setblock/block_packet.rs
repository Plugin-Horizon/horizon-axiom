// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/SetBlockPacketListener.java,
// NetworkHelper.readLinkedHashMap / readLinkedHashSet / readBlockHitResult

//! SetBlock packet (`axiom:set_block`): place or break a set of blocks, with or without neighbour updates.
//! The client already sends final block states, so the facing, axis, half and waterlogged values are not
//! recomputed here. This matches Java, which writes the decoded `BlockState` as it is.

use std::collections::{HashMap, HashSet};

use crate::buf::Reader;

use super::host::{BlockPos, SetBlockHost};
use super::integration;
use super::{
    can_modify_world, can_use_axiom, uuid_string, SetBlockConfig, SetBlockError, FLAGS_WITHOUT_NEIGHBOR_UPDATES,
    FLAGS_WITH_NEIGHBOR_UPDATES, PERM_BUILD_PLACE, REASON_ANGEL, REASON_REPLACE_MODE,
    LARGE_CHANGE_LOG_THRESHOLD,
};

/// Java `InteractionHand`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Main,
    Off,
}

/// Java `BlockHitResult` as sent by the client.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockHit {
    pub pos: BlockPos,
    /// Index into Java's `Direction` order by 3D data value: 0 down, 1 up, 2 north, 3 south, 4 west, 5 east.
    pub direction: u8,
    /// Click offset inside the block (only the event side uses it).
    pub click: (f32, f32, f32),
    pub inside: bool,
    pub world_border: bool,
}

/// Java `Direction.from3DDataValue`: `BY_3D_DATA[abs(data % 6)]`.
pub fn direction_from_3d_data(data: i8) -> u8 {
    ((data as i32) % 6).abs() as u8
}

/// Decoded SetBlock packet.
#[derive(Clone, Debug)]
pub struct SetBlockPacket {
    /// Blocks in packet order. Duplicate positions keep their first slot and take the last state (LinkedHashMap).
    pub blocks: Vec<(BlockPos, u32)>,
    pub update_neighbors: bool,
    /// Only read when `update_neighbors` is set. Positions that must not receive neighbour updates.
    pub prevent_updates_at: HashSet<BlockPos>,
    pub reason: i32,
    pub breaking: bool,
    pub hit: BlockHit,
    pub hand: Hand,
    /// Client sequence id. Negative means no acknowledgement.
    pub sequence: i32,
    by_pos: HashMap<BlockPos, u32>,
}

impl SetBlockPacket {
    /// Java `blocks.get(pos)`.
    pub fn desired(&self, pos: BlockPos) -> Option<u32> {
        self.by_pos.get(&pos).copied()
    }
}

/// Java `NetworkHelper` collection count check: `count > limit` throws, a negative count reads as empty.
fn read_count(r: &mut Reader, limit: usize) -> Result<usize, SetBlockError> {
    let count = r.read_varint()?;
    if count as i64 > limit as i64 {
        return Err(SetBlockError::CollectionTooLarge { count: count as usize, max: limit });
    }
    Ok(count.max(0) as usize)
}

fn read_block_hit(r: &mut Reader) -> Result<BlockHit, SetBlockError> {
    let pos = r.read_block_pos()?;
    let direction = direction_from_3d_data(r.read_bytes(1)?[0] as i8);
    let click = (r.read_f32()?, r.read_f32()?, r.read_f32()?);
    let inside = r.read_bool()?;
    let world_border = r.read_bool()?;
    Ok(BlockHit { pos, direction, click, inside, world_border })
}

fn read_hand(r: &mut Reader) -> Result<Hand, SetBlockError> {
    match r.read_varint()? {
        0 => Ok(Hand::Main),
        1 => Ok(Hand::Off),
        other => Err(SetBlockError::UnknownHand(other)),
    }
}

/// Decode a SetBlock packet body. `wire` maps the player's block registry id to a block state.
pub fn parse_set_block(r: &mut Reader, limit: usize, wire: &dyn Fn(i32) -> Option<u32>) -> Result<SetBlockPacket, SetBlockError> {
    let count = read_count(r, limit)?;
    let mut blocks: Vec<(BlockPos, u32)> = Vec::with_capacity(count.min(1024));
    let mut by_pos: HashMap<BlockPos, u32> = HashMap::new();
    let mut slot: HashMap<BlockPos, usize> = HashMap::new();
    for _ in 0..count {
        let pos = r.read_block_pos()?;
        let id = r.read_varint()?;
        let state = wire(id).ok_or(SetBlockError::UnknownBlockId(id))?;
        by_pos.insert(pos, state);
        match slot.get(&pos) {
            Some(&i) => blocks[i].1 = state,
            None => {
                slot.insert(pos, blocks.len());
                blocks.push((pos, state));
            }
        }
    }

    let update_neighbors = r.read_bool()?;
    let mut prevent_updates_at = HashSet::new();
    if update_neighbors {
        let n = read_count(r, limit)?;
        for _ in 0..n {
            prevent_updates_at.insert(r.read_block_pos()?);
        }
    }

    let reason = r.read_varint()?;
    let breaking = r.read_bool()?;
    let hit = read_block_hit(r)?;
    let hand = read_hand(r)?;
    let sequence = r.read_varint()?;

    Ok(SetBlockPacket { blocks, update_neighbors, prevent_updates_at, reason, breaking, hit, hand, sequence, by_pos })
}

/// Java `SetBlockPacketListener.onReceive`.
pub fn handle_set_block(host: &mut dyn SetBlockHost, cfg: &SetBlockConfig, player: u128, r: &mut Reader) -> Result<(), SetBlockError> {
    if !can_use_axiom(host, player, PERM_BUILD_PLACE) {
        return Ok(());
    }
    let dim = host.player_dimension(player);
    if !can_modify_world(host, player, &dim) {
        return Ok(());
    }

    let pkt = {
        let wire = |id: i32| host.block_state_from_wire(player, id);
        parse_set_block(r, cfg.packet_collection_read_limit, &wire)?
    };

    if cfg.log_large_block_buffer_changes && pkt.blocks.len() > LARGE_CHANGE_LOG_THRESHOLD {
        host.log_info(&format!(
            "Player {} modified {} individual blocks with axiom",
            uuid_string(player),
            pkt.blocks.len()
        ));
    }

    if pkt.sequence >= 0 {
        host.ack_block_changes(player, pkt.sequence);
    }

    // Java creates the BlockPlaceContext here, so the clicked position is fixed before any block is written.
    let clicked = host.clicked_pos(player, pkt.hand, &dim, &pkt.hit);

    if pkt.reason & REASON_REPLACE_MODE == 0 && pkt.reason & REASON_ANGEL == 0 && !fire_bukkit_events(host, player, &pkt, &dim) {
        return Ok(());
    }

    if pkt.update_neighbors {
        if pkt.prevent_updates_at.is_empty() {
            for &(pos, state) in &pkt.blocks {
                if !can_break_or_place(host, player, &dim, pos, state) {
                    continue;
                }
                host.set_block(&dim, pos, state, FLAGS_WITH_NEIGHBOR_UPDATES);
            }
        } else {
            // Positions that are themselves prevented are written after the others, without updates.
            let mut delayed: Vec<(BlockPos, u32)> = Vec::new();
            for &(pos, state) in &pkt.blocks {
                if !can_break_or_place(host, player, &dim, pos, state) {
                    continue;
                }
                // Java: if any neighbour is prevented, no neighbour updates for this block.
                let neighbour_prevented = NEIGHBOUR_OFFSETS.iter().any(|&(dx, dy, dz)| {
                    pkt.prevent_updates_at.contains(&(pos.0 + dx, pos.1 + dy, pos.2 + dz))
                });
                let update = !neighbour_prevented;
                if pkt.prevent_updates_at.contains(&pos) {
                    delayed.push((pos, state));
                    if !update {
                        continue;
                    }
                }
                let flags = if update { FLAGS_WITH_NEIGHBOR_UPDATES } else { FLAGS_WITHOUT_NEIGHBOR_UPDATES };
                host.set_block(&dim, pos, state, flags);
            }
            for (pos, state) in delayed {
                set_without_updates(host, player, &dim, pos, state);
            }
        }
    } else {
        for &(pos, state) in &pkt.blocks {
            set_without_updates(host, player, &dim, pos, state);
        }
    }

    if !pkt.breaking {
        // Java: only the clicked block of the context, when the packet contains it.
        if let Some(desired) = pkt.desired(clicked) {
            if !host.is_block_loaded(&dim, clicked) {
                return Ok(());
            }
            let actual = host.block_state(&dim, clicked);
            if host.is_air(desired) || host.is_air(actual) {
                return Ok(());
            }
            if !host.same_block(desired, actual) {
                return Ok(());
            }
            if !integration::can_place_block(host, player, &dim, clicked) {
                return Ok(());
            }
            host.apply_item_to_block_entity(player, pkt.hand, &dim, clicked);
            if !host.is_bed_double_plant_or_door(actual) {
                host.set_placed_by(player, pkt.hand, &dim, clicked, actual);
            }
        }
    }
    Ok(())
}

/// The six neighbour offsets, in Java `Direction` order.
const NEIGHBOUR_OFFSETS: [(i32, i32, i32); 6] = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];

/// Java `SetBlockPacketListener.fireBukkitEvents`. Returns false when a listener cancelled.
fn fire_bukkit_events(host: &mut dyn SetBlockHost, player: u128, pkt: &SetBlockPacket, dim: &str) -> bool {
    if !host.fire_player_interact(player, pkt.breaking, &pkt.hit, pkt.hand, dim) {
        return false;
    }
    if !pkt.breaking {
        let placed: Vec<(BlockPos, u32)> = pkt
            .blocks
            .iter()
            .filter(|&&(pos, _)| {
                let existing = host.block_state(dim, pos);
                host.state_can_be_replaced(existing)
            })
            .copied()
            .collect();
        if !placed.is_empty() && !host.fire_block_place(player, pkt.hand, dim, pkt.hit.pos, &placed) {
            return false;
        }
    }
    true
}

/// Java `canBreakOrPlace`: the chunk must be loaded, then the protection check for air (break) or a block (place).
pub(super) fn can_break_or_place(host: &dyn SetBlockHost, player: u128, dim: &str, pos: BlockPos, state: u32) -> bool {
    if !host.is_chunk_loaded(dim, pos.0 >> 4, pos.2 >> 4) {
        return false;
    }
    if host.is_air(state) {
        integration::can_break_block(host, player, dim, pos)
    } else {
        integration::can_place_block(host, player, dim, pos)
    }
}

/// Java `SetBlockPacketListener.setWithoutUpdates`: write one block straight into its section, keep the
/// heightmaps, light, POI and block entities in sync, and send no neighbour updates.
pub(super) fn set_without_updates(host: &mut dyn SetBlockHost, player: u128, dim: &str, pos: BlockPos, state: u32) {
    if !can_break_or_place(host, player, dim, pos, state) {
        return;
    }
    let (cx, cy, cz) = (pos.0 >> 4, pos.1 >> 4, pos.2 >> 4);

    if !host.is_chunk_loaded(dim, cx, cz) {
        return;
    }
    host.mark_chunk_unsaved(dim, cx, cz);

    let (min_section, max_section) = host.section_y_range(dim);
    if cy < min_section || cy > max_section {
        return;
    }

    let had_only_air = host.section_has_only_air(dim, cx, cy, cz);
    let old = host.set_block_raw(dim, pos, state);
    if old != state {
        host.update_heightmaps(dim, pos, state);
        host.mark_block_changed(dim, pos);
        if host.light_properties_differ(old, state) {
            host.check_block_light(dim, pos);
        }
        if host.block_entity_must_be_removed(old, state) {
            host.remove_block_entity(dim, pos);
        }
        host.update_poi(dim, pos, old, state);
        if host.has_block_entity(state) {
            host.sync_block_entity(dim, pos, state);
        }
    }

    let now_only_air = host.section_has_only_air(dim, cx, cy, cz);
    if had_only_air != now_only_air {
        host.update_section_emptiness(dim, cx, cy, cz, now_only_air);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setblock::test_support::{FakeHost, Call};
    use crate::buf::Writer;

    const PLAYER: u128 = 0x1234;

    fn pack(x: i32, y: i32, z: i32) -> i64 {
        (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
    }

    fn hit_bytes(w: &mut Writer, pos: BlockPos, dir: i8) {
        w.write_i64(pack(pos.0, pos.1, pos.2));
        w.write_bytes(&[dir as u8]);
        for f in [0.5f32, 0.5, 0.5] {
            w.write_bytes(&f.to_be_bytes());
        }
        w.write_bool(false);
        w.write_bool(false);
    }

    /// Build a SetBlock packet body.
    fn packet(blocks: &[(BlockPos, i32)], update: bool, prevent: &[BlockPos], reason: i32, breaking: bool, hit: BlockPos, seq: i32) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_varint(blocks.len() as i32);
        for (pos, id) in blocks {
            w.write_i64(pack(pos.0, pos.1, pos.2));
            w.write_varint(*id);
        }
        w.write_bool(update);
        if update {
            w.write_varint(prevent.len() as i32);
            for p in prevent {
                w.write_i64(pack(p.0, p.1, p.2));
            }
        }
        w.write_varint(reason);
        w.write_bool(breaking);
        hit_bytes(&mut w, hit, 1);
        w.write_varint(0); // main hand
        w.write_varint(seq);
        w.into_bytes()
    }

    fn cfg() -> SetBlockConfig {
        SetBlockConfig { packet_collection_read_limit: 1024, log_large_block_buffer_changes: false }
    }

    #[test]
    fn duplicate_positions_keep_first_slot_and_last_state() {
        let body = packet(&[((0, 64, 0), 5), ((1, 64, 0), 6), ((0, 64, 0), 7)], false, &[], 0, false, (0, 63, 0), -1);
        let mut r = Reader::new(&body);
        let w = |id: i32| if id >= 0 { Some(id as u32) } else { None };
        let pkt = parse_set_block(&mut r, 1024, &w).unwrap();
        assert_eq!(pkt.blocks, vec![((0, 64, 0), 7), ((1, 64, 0), 6)]);
        assert_eq!(pkt.desired((0, 64, 0)), Some(7));
        assert_eq!(pkt.hit.direction, 1);
        assert_eq!(pkt.hand, Hand::Main);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn collection_limit_is_enforced() {
        let body = packet(&[((0, 64, 0), 1), ((1, 64, 0), 1)], false, &[], 0, false, (0, 63, 0), -1);
        let mut r = Reader::new(&body);
        let w = |id: i32| Some(id as u32);
        let err = parse_set_block(&mut r, 1, &w).unwrap_err();
        assert!(matches!(err, SetBlockError::CollectionTooLarge { count: 2, max: 1 }));
    }

    #[test]
    fn direction_normalisation_matches_java() {
        assert_eq!(direction_from_3d_data(0), 0);
        assert_eq!(direction_from_3d_data(5), 5);
        assert_eq!(direction_from_3d_data(6), 0);
        assert_eq!(direction_from_3d_data(-1), 1);
    }

    #[test]
    fn denied_without_build_place_permission() {
        let mut host = FakeHost::new("minecraft:overworld");
        let body = packet(&[((0, 64, 0), 3)], true, &[], 0, false, (0, 63, 0), 4);
        handle_set_block(&mut host, &cfg(), PLAYER, &mut Reader::new(&body)).unwrap();
        assert!(host.calls.is_empty());
    }

    #[test]
    fn neighbour_update_path_uses_flag_three_and_acks() {
        let mut host = FakeHost::new("minecraft:overworld");
        host.grant(PLAYER, PERM_BUILD_PLACE);
        let body = packet(&[((0, 64, 0), 3), ((1, 64, 0), 4)], true, &[], 0, false, (0, 63, 0), 9);
        handle_set_block(&mut host, &cfg(), PLAYER, &mut Reader::new(&body)).unwrap();
        assert!(host.calls.contains(&Call::Ack(9)));
        let sets: Vec<_> = host.calls.iter().filter_map(|c| match c {
            Call::SetBlock(p, s, f) => Some((*p, *s, *f)),
            _ => None,
        }).collect();
        assert_eq!(sets, vec![((0, 64, 0), 3, 3), ((1, 64, 0), 4, 3)]);
    }

    #[test]
    fn prevented_position_is_written_last_without_updates() {
        let mut host = FakeHost::new("minecraft:overworld");
        host.grant(PLAYER, PERM_BUILD_PLACE);
        let body = packet(&[((0, 64, 0), 3), ((5, 64, 5), 4)], true, &[(5, 64, 5)], 0, false, (0, 63, 0), -1);
        handle_set_block(&mut host, &cfg(), PLAYER, &mut Reader::new(&body)).unwrap();
        let sets: Vec<_> = host.calls.iter().filter_map(|c| match c {
            Call::SetBlock(p, s, f) => Some((*p, *s, *f)),
            _ => None,
        }).collect();
        // Java: (0,64,0) updates. (5,64,5) is prevented, so it is also written with updates first
        // (no neighbour is prevented), then written again without updates in the delayed pass.
        assert_eq!(sets, vec![((0, 64, 0), 3, 3), ((5, 64, 5), 4, 3)]);
        assert!(host.calls.contains(&Call::RawSet((5, 64, 5), 4)));
    }

    #[test]
    fn unknown_block_id_is_an_error() {
        let body = packet(&[((0, 64, 0), -1)], false, &[], 0, false, (0, 63, 0), -1);
        let mut host = FakeHost::new("minecraft:overworld");
        host.grant(PLAYER, PERM_BUILD_PLACE);
        let err = handle_set_block(&mut host, &cfg(), PLAYER, &mut Reader::new(&body)).unwrap_err();
        assert!(matches!(err, SetBlockError::UnknownBlockId(-1)));
    }
}
