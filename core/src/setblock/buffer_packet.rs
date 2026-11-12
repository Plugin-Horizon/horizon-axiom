// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/SetBlockBufferPacketListener.java,
// operations/SetBlockBufferOperation.java

//! SetBlockBuffer packet (`axiom:set_block_buffer`): whole chunk sections of blocks or biomes.
//! Block buffers are applied by [`SetBlockBufferOperation`], which spreads the work across ticks and loads far
//! chunks asynchronously. Biome buffers are applied at once.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::buf::{Reader, Writer};
use crate::buffer::{pack_pos, unpack_pos, BiomeBuffer, BlockBuffer, BufferError, CompressedBlockEntity, LongArrays};

use super::dispatch::DispatchBudget;
use super::host::{BlockPos, ChunkLoadState, SetBlockHost};
use super::integration;
use super::queue::{OperationQueue, PendingOperation};
use super::{
    can_modify_world, can_use_axiom, uuid_string, SetBlockConfig, SetBlockError, MAX_CHUNK_FUTURES, PERM_BUILD_SECTION,
    PERM_BUILD_NBT,
};

pub const BUFFER_TYPE_BLOCKS: u8 = 0;
pub const BUFFER_TYPE_BIOMES: u8 = 1;

/// Decoded SetBlockBuffer packet body (after the dimension, the discarded UUID and the type byte).
pub enum BufferPacket {
    Blocks { dim: String, buffer: BlockBuffer, dispatch_sends: i64 },
    Biomes { dim: String, buffer: BiomeBuffer, dispatch_sends: i64 },
}

/// Decode a SetBlockBuffer packet. `empty` is the "no block" state id and `global_bits` the palette bit count.
pub fn parse_buffer_packet(
    r: &mut Reader,
    empty: u32,
    global_bits: u8,
    framing: LongArrays,
) -> Result<BufferPacket, SetBlockError> {
    let dim = r.read_string(32767)?;
    r.read_uuid()?; // discarded, buffers are not associated with a UUID
    let kind = r.read_bytes(1)?[0];
    match kind {
        BUFFER_TYPE_BLOCKS => {
            let buffer = BlockBuffer::load(r, empty, global_bits, framing)?;
            let dispatch_sends = r.read_varint()? as i64;
            Ok(BufferPacket::Blocks { dim, buffer, dispatch_sends })
        }
        BUFFER_TYPE_BIOMES => {
            let buffer = BiomeBuffer::load(r)?;
            let dispatch_sends = r.read_varint()? as i64;
            Ok(BufferPacket::Biomes { dim, buffer, dispatch_sends })
        }
        other => Err(SetBlockError::UnknownBufferType(other)),
    }
}

/// Decode and apply a SetBlockBuffer packet. Java runs this on the server thread via `server.execute`.
/// A failing apply kicks the player (Java `catch (Throwable)`).
pub fn handle_buffer_packet(
    host: &mut dyn SetBlockHost,
    cfg: &SetBlockConfig,
    budget: &mut DispatchBudget,
    queue: &mut OperationQueue,
    player: u128,
    r: &mut Reader,
) -> Result<(), SetBlockError> {
    let client_protocol = host.client_protocol(player);
    let framing = client_protocol.map_or(LongArrays::Fixed, LongArrays::for_protocol);
    let packet = parse_buffer_packet(r, host.empty_state(), host.client_global_bits(player), framing)?;
    let result = match packet {
        BufferPacket::Blocks { dim, mut buffer, dispatch_sends } => {
            // A client on another release sends its own state ids (Java: the ViaVersion-mapped registry).
            if client_protocol.is_some() {
                let empty = host.empty_state();
                buffer.map_states(host.global_bits(), |v| {
                    i32::try_from(v)
                        .ok()
                        .and_then(|id| host.block_state_from_wire(player, id))
                        .unwrap_or(empty)
                });
            }
            apply_block_buffer(host, cfg, budget, queue, player, &dim, buffer, dispatch_sends)
        }
        BufferPacket::Biomes { dim, buffer, dispatch_sends } => {
            apply_biome_buffer(host, cfg, budget, player, &dim, &buffer, dispatch_sends)
        }
    };
    if let Err(e) = result {
        host.kick(player, &format!("An error occurred while processing block change: {e}"));
    }
    Ok(())
}

/// Java `applyBlockBuffer`, minus the `server.execute` hop.
#[allow(clippy::too_many_arguments)]
fn apply_block_buffer(
    host: &mut dyn SetBlockHost,
    cfg: &SetBlockConfig,
    budget: &mut DispatchBudget,
    queue: &mut OperationQueue,
    player: u128,
    world_key: &str,
    buffer: BlockBuffer,
    dispatch_sends: i64,
) -> Result<(), SetBlockError> {
    if cfg.log_large_block_buffer_changes {
        host.log_info(&format!(
            "Player {} modified {} chunk sections (blocks)",
            uuid_string(player),
            buffer.section_count()
        ));
    }

    if !budget.consume(host, player, buffer.section_count() as i64, dispatch_sends) {
        return Ok(());
    }
    if !can_use_axiom(host, player, PERM_BUILD_SECTION) {
        return Ok(());
    }
    let dim = host.player_dimension(player);
    if dim != world_key || !can_modify_world(host, player, &dim) {
        return Ok(());
    }

    let allow_nbt = host.has_axiom_permission(player, PERM_BUILD_NBT);
    queue.add(host, &dim, Box::new(SetBlockBufferOperation::new(player, buffer, allow_nbt)))
}

/// Java `applyBiomeBuffer`. Biome changes are written immediately, not queued.
fn apply_biome_buffer(
    host: &mut dyn SetBlockHost,
    cfg: &SetBlockConfig,
    budget: &mut DispatchBudget,
    player: u128,
    world_key: &str,
    biome: &BiomeBuffer,
    dispatch_sends: i64,
) -> Result<(), SetBlockError> {
    if cfg.log_large_block_buffer_changes {
        host.log_info(&format!(
            "Player {} modified {} chunk sections (biomes)",
            uuid_string(player),
            biome.section_count()
        ));
    }

    if !budget.consume(host, player, biome.section_count() as i64, dispatch_sends) {
        return Ok(());
    }
    if !can_use_axiom(host, player, PERM_BUILD_SECTION) {
        return Ok(());
    }
    let dim = host.player_dimension(player);
    if dim != world_key || !can_modify_world(host, player, &dim) {
        return Ok(());
    }

    let (min_section, max_section) = host.section_y_range(&dim);
    let mut changed: BTreeSet<(i32, i32)> = BTreeSet::new();

    biome.for_each_entry(|qx, qy, qz, key| {
        let cy = qy >> 2;
        if cy < min_section || cy > max_section {
            return;
        }
        if !host.biome_exists(key) {
            return;
        }
        let (cx, cz) = (qx >> 2, qz >> 2);
        if !host.is_chunk_loaded(&dim, cx, cz) {
            return;
        }
        if !integration::can_place_block(&*host, player, &dim, ((qx << 2) + 1, (qy << 2) + 1, (qz << 2) + 1)) {
            return;
        }
        host.set_biome_quart(&dim, qx, qy, qz, key);
        changed.insert((cx, cz));
    });

    let mut per_player: BTreeMap<u128, Vec<(i32, i32)>> = BTreeMap::new();
    for &(cx, cz) in &changed {
        host.mark_chunk_unsaved(&dim, cx, cz);
        for p in host.players_tracking_chunk(&dim, cx, cz) {
            per_player.entry(p).or_default().push((cx, cz));
        }
    }
    for (p, chunks) in per_player {
        host.send_chunk_biomes(p, &dim, &chunks);
    }
    Ok(())
}

/// Java `SetBlockBufferOperation`: writes a block buffer chunk by chunk.
pub struct SetBlockBufferOperation {
    executor: u128,
    buffer: BlockBuffer,
    allow_nbt: bool,
    /// Chunks still to start, keyed by Java `ChunkPos.asLong` (natural signed order): `(cx, cz, sections)`.
    pending: BTreeMap<i64, (i32, i32, Vec<(i32, i32, i32)>)>,
    /// Chunks in flight, in order: `(cx, cz, state, sections)`.
    futures: VecDeque<(i32, i32, Fut, Vec<(i32, i32, i32)>)>,
    game_master_warning: bool,
    finished: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fut {
    /// Loaded chunk outside the load distance, taken as is (Java `completedFuture`).
    Ready,
    /// Started with `request_chunk_load`.
    Loading,
}

/// Java `ChunkPos.asLong`: low 32 bits are x, high 32 bits are z.
fn chunk_key(cx: i32, cz: i32) -> i64 {
    (cx as u32 as i64) | ((cz as u32 as i64) << 32)
}

/// Block entity NBT as the network bytes `load_block_entity_nbt` expects. Fails for unknown dictionaries and
/// broken frames; the caller skips such entities.
fn block_entity_bytes(entity: &CompressedBlockEntity) -> Result<Vec<u8>, BufferError> {
    let nbt = entity.decompress()?;
    let mut w = Writer::new();
    w.write_nbt(&nbt)?;
    Ok(w.into_bytes())
}

impl SetBlockBufferOperation {
    pub fn new(executor: u128, buffer: BlockBuffer, allow_nbt: bool) -> Self {
        let mut pending: BTreeMap<i64, (i32, i32, Vec<(i32, i32, i32)>)> = BTreeMap::new();
        for (key, _) in buffer.sections() {
            let (cx, cy, cz) = unpack_pos(key);
            pending.entry(chunk_key(cx, cz)).or_insert_with(|| (cx, cz, Vec::new())).2.push((cx, cy, cz));
        }
        Self { executor, buffer, allow_nbt, pending, futures: VecDeque::new(), game_master_warning: false, finished: false }
    }

    /// Lets go of the holds of the chunks still loading, when the operation fails.
    fn release_loading(&mut self, host: &mut dyn SetBlockHost, dim: &str) {
        for (cx, cz, fut, _) in self.futures.drain(..) {
            if fut == Fut::Loading {
                host.release_chunk_load(dim, cx, cz);
            }
        }
    }

    /// Start up to `MAX_CHUNK_FUTURES` chunk loads, nearest rules as in Java.
    fn start_futures(&mut self, host: &mut dyn SetBlockHost, dim: &str) {
        let max_distance = host.max_chunk_load_distance(dim);
        let (px, _, pz) = host.player_block_pos(self.executor);
        let (psx, psz) = (px >> 4, pz >> 4);

        while self.futures.len() < MAX_CHUNK_FUTURES {
            let Some((&key, _)) = self.pending.iter().next() else { break };
            let (x, z, sections) = self.pending.remove(&key).expect("key was just seen");

            let distance = (psx - x).abs() + (psz - z).abs();
            if distance < max_distance {
                host.request_chunk_load(dim, x, z);
                self.futures.push_back((x, z, Fut::Loading, sections));
            } else if host.is_chunk_loaded(dim, x, z) {
                self.futures.push_back((x, z, Fut::Ready, sections));
            }
        }
    }

    /// Write every block of one chunk (Java: the body of the `chunkFutures` loop).
    fn write_chunk(&mut self, host: &mut dyn SetBlockHost, dim: &str, cx: i32, cz: i32, sections: Vec<(i32, i32, i32)>) {
        let (min_section, max_section) = host.section_y_range(dim);
        let mut chunk_changed = false;
        let mut light_changed = false;
        // Block entity data of this chunk, loaded in one bulk edit after its blocks are written.
        let mut entities: Vec<(BlockPos, Vec<u8>)> = Vec::new();

        for (scx, scy, scz) in sections {
            if scy < min_section || scy > max_section {
                continue;
            }
            let checker = integration::check_section(&*host, self.executor, dim, scx, scy, scz);
            if checker.none_allowed() {
                continue;
            }
            let bounds = checker.bounds();

            let had_only_air = host.section_has_only_air(dim, scx, scy, scz);
            let (bx0, by0, bz0) = (scx * 16, scy * 16, scz * 16);

            for y in bounds.min_y..=bounds.max_y {
                for z in bounds.min_z..=bounds.max_z {
                    for x in bounds.min_x..=bounds.max_x {
                        let pos: BlockPos = (bx0 + x, by0 + y, bz0 + z);
                        // `get` returns None for the buffer's empty state, which is skipped.
                        let Some(state) = self.buffer.get(pos.0, pos.1, pos.2) else { continue };
                        if had_only_air && host.is_air(state) {
                            continue;
                        }
                        if !checker.allowed(x, y, z) {
                            continue;
                        }

                        let old = host.set_block_raw(dim, pos, state);
                        if old != state {
                            chunk_changed = true;
                            host.update_heightmaps(dim, pos, state);
                            light_changed |= host.light_properties_differ(old, state);
                            if host.block_entity_must_be_removed(old, state) {
                                host.remove_block_entity(dim, pos);
                            }
                            host.update_poi(dim, pos, old, state);
                        }

                        if host.has_block_entity(state) {
                            let present = host.sync_block_entity(dim, pos, state);
                            if present && self.allow_nbt {
                                if host.is_game_master_block(dim, pos) && !host.can_use_game_master_blocks(self.executor) {
                                    self.game_master_warning = true;
                                } else {
                                    let offset = (x | (y << 4) | (z << 8)) as i16;
                                    let entity = self
                                        .buffer
                                        .block_entities(pack_pos(scx, scy, scz))
                                        .and_then(|map| map.get(&offset));
                                    if let Some(entity) = entity {
                                        match block_entity_bytes(entity) {
                                            Ok(nbt) => entities.push((pos, nbt)),
                                            Err(e) => host.log_info(&format!(
                                                "WARN: skipped block entity at {},{},{}: {e:?}",
                                                pos.0, pos.1, pos.2
                                            )),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let now_only_air = host.section_has_only_air(dim, scx, scy, scz);
            if had_only_air != now_only_air {
                host.update_section_emptiness(dim, scx, scy, scz, now_only_air);
            }
        }

        if !entities.is_empty() {
            host.load_block_entities(dim, &entities);
            chunk_changed = true;
        }
        if chunk_changed {
            host.send_chunk(dim, cx, cz);
            host.mark_chunk_unsaved(dim, cx, cz);
        }
        if light_changed {
            host.light_chunk(dim, cx, cz);
        }
    }
}

impl PendingOperation for SetBlockBufferOperation {
    fn is_finished(&self) -> bool {
        self.finished
    }

    fn executor(&self) -> u128 {
        self.executor
    }

    fn tick(&mut self, host: &mut dyn SetBlockHost, dim: &str) -> Result<(), SetBlockError> {
        if !self.pending.is_empty() {
            self.start_futures(host, dim);
        }

        // Process in order. A chunk that is still loading stops this tick (Java `return`).
        while let Some((cx, cz, fut, _)) = self.futures.front() {
            let (cx, cz, fut) = (*cx, *cz, *fut);
            if fut == Fut::Loading {
                match host.chunk_load_state(dim, cx, cz) {
                    ChunkLoadState::Pending => return Ok(()),
                    ChunkLoadState::Failed => {
                        self.release_loading(host, dim);
                        return Err(SetBlockError::ChunkLoadFailed { x: cx, z: cz });
                    }
                    ChunkLoadState::Loaded => {}
                }
            }
            let Some((_, _, _, sections)) = self.futures.pop_front() else { break };

            // A chunk that unloaded while queued is skipped. Java would hold the chunk object.
            if host.is_chunk_loaded(dim, cx, cz) {
                self.write_chunk(host, dim, cx, cz, sections);
            }
            if fut == Fut::Loading {
                host.release_chunk_load(dim, cx, cz);
            }
        }

        if !self.pending.is_empty() {
            return Ok(());
        }

        if self.game_master_warning {
            host.send_system_message(
                self.executor,
                "Unable to set data for Game Master block since you don't have op",
                true,
            );
        }
        self.finished = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setblock::permission::SectionPermissionChecker;
    use crate::setblock::test_support::{Call, FakeHost};

    /// Same empty state and palette bits as `FakeHost`.
    const EMPTY: u32 = 0;

    fn buffer_with(state: u32) -> BlockBuffer {
        let mut b = BlockBuffer::new(EMPTY, 15);
        b.set(1, 2, 3, state);
        b
    }

    #[test]
    fn block_buffer_op_writes_the_section_when_chunk_is_loaded() {
        let mut host = FakeHost::new("minecraft:overworld");
        let mut op = SetBlockBufferOperation::new(1, buffer_with(5), false);
        op.tick(&mut host, "minecraft:overworld").unwrap();
        assert!(op.is_finished());
        assert_eq!(host.block_state("minecraft:overworld", (1, 2, 3)), 5);
        assert!(host.calls.contains(&Call::SendChunk(0, 0)));
    }

    #[test]
    fn block_buffer_op_waits_while_a_chunk_loads() {
        let mut host = FakeHost::new("minecraft:overworld");
        host.chunk_states.insert(("minecraft:overworld".into(), 0, 0), ChunkLoadState::Pending);
        let mut op = SetBlockBufferOperation::new(1, buffer_with(5), false);
        op.tick(&mut host, "minecraft:overworld").unwrap();
        assert!(!op.is_finished());
        assert_eq!(host.block_state("minecraft:overworld", (1, 2, 3)), 0);
        assert!(host.calls.contains(&Call::RequestLoad(0, 0)));

        host.chunk_states.insert(("minecraft:overworld".into(), 0, 0), ChunkLoadState::Loaded);
        op.tick(&mut host, "minecraft:overworld").unwrap();
        assert!(op.is_finished());
        assert_eq!(host.block_state("minecraft:overworld", (1, 2, 3)), 5);
    }

    #[test]
    fn block_buffer_op_fails_when_a_chunk_fails_to_load() {
        let mut host = FakeHost::new("minecraft:overworld");
        host.chunk_states.insert(("minecraft:overworld".into(), 0, 0), ChunkLoadState::Failed);
        let mut op = SetBlockBufferOperation::new(1, buffer_with(5), false);
        assert!(matches!(op.tick(&mut host, "minecraft:overworld"), Err(SetBlockError::ChunkLoadFailed { x: 0, z: 0 })));
    }

    #[test]
    fn section_denied_by_region_is_skipped() {
        let mut host = FakeHost::new("minecraft:overworld");
        host.region = SectionPermissionChecker::None;
        let mut op = SetBlockBufferOperation::new(1, buffer_with(5), false);
        op.tick(&mut host, "minecraft:overworld").unwrap();
        assert!(op.is_finished());
        assert_eq!(host.block_state("minecraft:overworld", (1, 2, 3)), 0);
    }

    #[test]
    fn chunk_key_matches_java_chunkpos_as_long() {
        // Java ChunkPos.asLong puts x in the low word and z in the high word.
        assert_eq!(chunk_key(1, 0), 1);
        assert_eq!(chunk_key(0, 1), 1i64 << 32);
        assert_eq!(chunk_key(-1, 0), 0xFFFF_FFFF);
    }

    #[test]
    fn denied_region_checker_type_is_reachable() {
        let c = SectionPermissionChecker::None;
        assert!(c.none_allowed());
    }
}
