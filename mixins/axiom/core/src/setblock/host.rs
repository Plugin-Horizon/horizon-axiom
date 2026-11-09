// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/SetBlockPacketListener.java,
// packet/impl/SetBlockBufferPacketListener.java, packet/impl/TickBlocksPacketListener.java,
// operations/SetBlockBufferOperation.java, integration/Integration.java, event/AxiomModifyWorldEvent.java (as callback)

//! World, player and integration access for the set-block module. The game glue implements [`SetBlockHost`].
//!
//! Conventions: positions are world block coordinates, chunk and section coordinates are `(cx, cz)` and
//! `(cx, cy, cz)`, dimensions are ids such as `"minecraft:overworld"`, players are `u128` UUIDs and block
//! states are global `u32` state ids. Methods that mirror a Java call document the Java call they replace.

use super::block_packet::{BlockHit, Hand};
use super::permission::SectionPermissionChecker;

/// Block position `(x, y, z)` in world coordinates.
pub type BlockPos = (i32, i32, i32);

/// Result of polling an asynchronous chunk load (the Java `CompletableFuture<LevelChunk>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkLoadState {
    Pending,
    Loaded,
    Failed,
}

pub trait SetBlockHost {
    // ---- players and permissions ----

    /// Dimension id the player is currently in.
    fn player_dimension(&self, player: u128) -> String;
    /// Current block position of the player (Java `player.getBlockX/Y/Z`).
    fn player_block_pos(&self, player: u128) -> BlockPos;
    /// Scoreboard name used in broadcast messages.
    fn scoreboard_name(&self, player: u128) -> String;
    /// Java `AxiomPaper.activeAxiomPlayers` membership.
    fn is_axiom_active(&self, player: u128) -> bool;
    /// Java `AxiomPaper.hasPermission(player, perm)`. Must return true for operators.
    /// `node` is an `axiom.*` permission string, see the constants in `mod.rs`.
    fn has_axiom_permission(&self, player: u128, node: &str) -> bool;
    /// Plain Bukkit-style `player.hasPermission(node)` (used for `axiomadmin.bypass_region_checks`).
    fn has_bukkit_permission(&self, player: u128, node: &str) -> bool;
    /// Java `player.canUseGameMasterBlocks()`.
    fn can_use_game_master_blocks(&self, player: u128) -> bool;
    /// Java `AxiomPaper.getAllowedDispatchSendsPerSecond(player)` (includes `axiomlimits.*` overrides).
    fn allowed_dispatch_sends_per_second(&self, player: u128) -> i64;
    /// Disconnect a player with a plain-text reason.
    fn kick(&mut self, player: u128, reason: &str);
    /// Chat message to one player. `red` is the Java `ChatFormatting.RED` style.
    fn send_system_message(&mut self, player: u128, text: &str, red: bool) {
        let _ = (player, text, red);
    }
    /// Chat broadcast to all players.
    fn broadcast_system_message(&mut self, text: &str) {
        let _ = text;
    }
    /// Server log line (only used for the optional large-change logging).
    fn log_info(&mut self, text: &str) {
        let _ = text;
    }

    // ---- world gate (AxiomPaper.canModifyWorld) ----

    /// Java `whitelist-world-regex` / `blacklist-world-regex` check on the world name.
    fn world_allowed_by_config(&self, dim: &str) -> bool;
    /// Fire `AxiomModifyWorldEvent(player, world)`. `cancelled` is the event's default (the player is
    /// not in that world). Returns the final cancelled state after all listeners ran.
    fn modify_world_event(&mut self, player: u128, dim: &str, cancelled: bool) -> bool;

    /// Java `AxiomPaper.getMaxChunkLoadDistance(world)` (already 0 for PlotSquared plot worlds).
    fn max_chunk_load_distance(&self, dim: &str) -> i32;

    // ---- block registry and block queries ----

    /// Java `registry.byIdOrThrow(id)` for the player's block registry. `None` if the id is unknown.
    fn block_state_from_wire(&self, player: u128, wire_id: i32) -> Option<u32>;
    /// Java `BlockState.isAir()`.
    fn is_air(&self, state: u32) -> bool;
    /// Global state id of the "no block" sentinel (Java `EMPTY_STATE`, void_air) used by block buffers.
    fn empty_state(&self) -> u32;
    /// Bit count of the global block state palette (9..=32), used to decode block buffers.
    fn global_bits(&self) -> u8;
    /// Java `a.getBlock() == b.getBlock()`.
    fn same_block(&self, a: u32, b: u32) -> bool;
    /// Java `BlockState.canBeReplaced()` for the state currently in the world.
    fn state_can_be_replaced(&self, state: u32) -> bool;
    /// Java `BlockState.hasBlockEntity()`.
    fn has_block_entity(&self, state: u32) -> bool;
    /// Java `blockState.getFluidState().isEmpty()`.
    fn fluid_is_empty(&self, state: u32) -> bool;
    /// Java `blockState.getBlock() instanceof LiquidBlock`.
    fn is_liquid_block(&self, state: u32) -> bool;
    /// Java `actualBlock instanceof BedBlock / DoublePlantBlock / DoorBlock`. Those skip `setPlacedBy`.
    fn is_bed_double_plant_or_door(&self, state: u32) -> bool;
    /// Java `LightEngine.hasDifferentLightProperties(old, new)`.
    fn light_properties_differ(&self, old: u32, new: u32) -> bool;
    /// Java `!old.is(block) && old.hasBlockEntity() && !new.shouldChangedStateKeepBlockEntity(old)`.
    fn block_entity_must_be_removed(&self, old: u32, new: u32) -> bool;
    /// Java `blockEntity instanceof GameMasterBlock` for the block entity at `pos` (false if none).
    fn is_game_master_block(&self, dim: &str, pos: BlockPos) -> bool;
    /// Java `Block.updateFromNeighbourShapes(state, level, pos)`.
    fn update_from_neighbour_shapes(&self, dim: &str, pos: BlockPos, state: u32) -> u32;

    // ---- chunks, sections and raw writes ----

    /// Java `level.isLoaded(pos)`.
    fn is_block_loaded(&self, dim: &str, pos: BlockPos) -> bool;
    /// Java `world.isChunkLoaded(cx, cz)` / `level.getChunkIfLoaded(cx, cz) != null`.
    fn is_chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool;
    /// Java `world.getChunkAtAsync(cx, cz)` (start loading; no-op if already loading or loaded).
    fn request_chunk_load(&mut self, dim: &str, cx: i32, cz: i32);
    /// Poll a chunk started with `request_chunk_load` (or any loaded chunk, which is `Loaded`).
    fn chunk_load_state(&self, dim: &str, cx: i32, cz: i32) -> ChunkLoadState;
    /// Java `level.getMinSectionY()` and `level.getMaxSectionY()`, both inclusive.
    fn section_y_range(&self, dim: &str) -> (i32, i32);
    /// Java `LevelChunkSection.hasOnlyAir()` for the section.
    fn section_has_only_air(&self, dim: &str, cx: i32, cy: i32, cz: i32) -> bool;
    /// Java `level.getBlockState(pos)`.
    fn block_state(&self, dim: &str, pos: BlockPos) -> u32;
    /// Java `LevelChunkSection.setBlockState(x, y, z, state, true)`: raw section write, no neighbour
    /// updates, no heightmap or light work. Returns the previous state.
    fn set_block_raw(&mut self, dim: &str, pos: BlockPos, state: u32) -> u32;
    /// Java `motionBlocking`, `motionBlockingNoLeaves`, `oceanFloor` and `worldSurface` `.update(...)` for one
    /// changed block. Called after `set_block_raw` when the state changed.
    fn update_heightmaps(&mut self, dim: &str, pos: BlockPos, new_state: u32);
    /// Java `chunk.markUnsaved()`.
    fn mark_chunk_unsaved(&mut self, dim: &str, cx: i32, cz: i32);
    /// Java `level.getChunkSource().blockChanged(pos)`.
    fn mark_block_changed(&mut self, dim: &str, pos: BlockPos);
    /// Java `level.getChunkSource().getLightEngine().checkBlock(pos)`.
    fn check_block_light(&mut self, dim: &str, pos: BlockPos);
    /// Java `LightEngine.updateSectionStatus(...)` plus `onSectionEmptinessChanged(...)`.
    fn update_section_emptiness(&mut self, dim: &str, cx: i32, cy: i32, cz: i32, has_only_air: bool);
    /// Java POI update: remove the old POI and add the new one when `PoiTypes.forState` differs.
    fn update_poi(&mut self, dim: &str, pos: BlockPos, old: u32, new: u32);
    /// Java `chunk.removeBlockEntity(pos)`.
    fn remove_block_entity(&mut self, dim: &str, pos: BlockPos);
    /// Java block-entity creation and update for a state that `has_block_entity`: drop an invalid
    /// block entity, create one if missing (`newBlockEntity` + `addAndRegisterBlockEntity`), or update
    /// the existing one (`setBlockState` + ticker refresh). Returns true if a block entity exists afterwards.
    fn sync_block_entity(&mut self, dim: &str, pos: BlockPos, state: u32) -> bool;
    /// Java `blockEntity.loadWithComponents(TagValueInput.create(... decompressed NBT))`.
    fn load_block_entity_nbt(&mut self, dim: &str, pos: BlockPos, nbt: &[u8]);
    /// Java `WorldExtension.sendChunk(cx, cz)`: send the chunk data to watching players.
    fn send_chunk(&mut self, dim: &str, cx: i32, cz: i32);
    /// Java `WorldExtension.lightChunk(cx, cz)`.
    fn light_chunk(&mut self, dim: &str, cx: i32, cz: i32);

    // ---- biomes ----

    /// Java `registry.get(biomeKey).isPresent()` for the biome registry.
    fn biome_exists(&self, key: &str) -> bool;
    /// Java `container.set(x & 3, y & 3, z & 3, biome)` in the section at quart coordinates `(qx, qy, qz)`.
    /// The chunk is known to be loaded.
    fn set_biome_quart(&mut self, dim: &str, qx: i32, qy: i32, qz: i32, key: &str);
    /// Players that currently track the chunk (Java `chunkMap.getPlayers(pos, false)`).
    fn players_tracking_chunk(&self, dim: &str, cx: i32, cz: i32) -> Vec<u128>;
    /// Java `ClientboundChunksBiomesPacket.forChunks(chunks)` sent to one player.
    fn send_chunk_biomes(&mut self, player: u128, dim: &str, chunks: &[(i32, i32)]);

    // ---- protection integrations (PlotSquared, WorldGuard, custom registrations) ----
    // The glue combines every registered integration. `section` results are merged with
    // `SectionPermissionChecker::combine`. The bypass permission is checked by this module.

    fn region_can_break(&self, player: u128, dim: &str, pos: BlockPos) -> bool;
    fn region_can_place(&self, player: u128, dim: &str, pos: BlockPos) -> bool;
    fn region_section(&self, player: u128, dim: &str, cx: i32, cy: i32, cz: i32) -> SectionPermissionChecker;

    // ---- placement packet side effects ----

    /// Java `player.connection.ackBlockChangesUpTo(seq)`.
    fn ack_block_changes(&mut self, player: u128, seq: i32);
    /// Java `PlayerInteractEvent` (left click when breaking, right click otherwise). Returns true if not cancelled.
    fn fire_player_interact(&mut self, player: u128, breaking: bool, hit: &BlockHit, hand: Hand, dim: &str) -> bool;
    /// Java `BlockPlaceEvent` (one entry) or `BlockMultiPlaceEvent` (more than one). `placed` holds only the
    /// positions whose existing block `state_can_be_replaced`, and is never empty. `against` is the hit block.
    /// Returns true if not cancelled.
    fn fire_block_place(&mut self, player: u128, hand: Hand, dim: &str, against: BlockPos, placed: &[(BlockPos, u32)]) -> bool;
    /// Java `BlockPlaceContext.getClickedPos()`. Must be evaluated before the packet's blocks are written
    /// (the Java context is created before the updates).
    fn clicked_pos(&self, player: u128, hand: Hand, dim: &str, hit: &BlockHit) -> BlockPos;
    /// Java `BlockItem.updateCustomBlockEntityTag` and `blockEntity.applyComponentsFromItemStack(inHand)`.
    fn apply_item_to_block_entity(&mut self, player: u128, hand: Hand, dim: &str, pos: BlockPos);
    /// Java `block.setPlacedBy(level, pos, state, player, inHand)`.
    fn set_placed_by(&mut self, player: u128, hand: Hand, dim: &str, pos: BlockPos, state: u32);

    // ---- writes with neighbour updates and tick side effects ----

    /// Java `level.setBlock(pos, state, flags)`. Returns nothing; Java ignores the result.
    fn set_block(&mut self, dim: &str, pos: BlockPos, state: u32, flags: u32);
    /// Java `fluidState.tick(level, pos, state)`.
    fn tick_fluid(&mut self, dim: &str, pos: BlockPos, state: u32);
    /// Java `blockState.tick(level, pos, level.getRandom())`.
    fn tick_block(&mut self, dim: &str, pos: BlockPos, state: u32);
}
