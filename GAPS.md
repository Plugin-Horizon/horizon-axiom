# GAPS: what Horizon must add or change for Axiom to work fully

Checked against the Horizon source at `92fd45976` and the Java reference
(AxiomPaperPlugin, `src/main/java/com/moulberry/axiom/`). Blind: nothing here has been built or run against a server.
Items marked "not verified" were not read to the end.

Each bullet: what is missing, the Axiom feature that needs it, and the engine function or hook it would look like.

## Missing: the engine has no function for it

- **Set the time of day.** Needed by Axiom's `set_world_time` packet (`SetTimePacketListener`, `AxiomTimeChangeEvent`).
  Horizon has the readers `native_day_time` and `native_level_day_time`, and the cancellable `native_time_skip`, but no
  setter. Shape: `native_set_day_time(&mut self, dimension: &str, ticks: i64, cause: &str) -> bool`, going through the
  path `/time set` takes, so `native_time_skip` still hears it. The port's `WorldHost::set_day_time` logs once and changes nothing.

- **Write biomes.** Needed by `BiomeBuffer` and the biome branch of `set_buffer` (`SetBlockBufferPacketListener`).
  `native_biome` only reads; biomes are written in worldgen only (`set_biomes` in horizon-world). Shape:
  `native_set_biomes(&mut self, dimension: &str, cells: &[([i32; 3], &str)]) -> Result<u64, String>` in quart
  coordinates, which writes the cells and sends the chunk whole, as `edit_blocks` does for blocks. The port has no biome write yet
  (`set_biome` returns false).

- **Region and plot build check.** Needed by every Axiom edit and entity change: Axiom asks WorldGuard and PlotSquared
  whether the player may build there (`SectionPermissionChecker`, `integration/worldguard`, `integration/plotsquared`).
  Horizon has no hook for it: `native_block_place` covers placement only, `native_left_click_block` the start of a
  dig, and `native_edit_blocks` has no player and no cancel. Shape: `native_can_build(player: u128, dimension: &str,
  pos: [i32; 3]) -> bool`, answered by mods the way `native_permission_check` is, and asked by every bulk path with the
  acting player. The port stubs it as `false`, which denies every edit until a region provider answers.

- **Cancel a physical trigger (NoPhysicalTrigger).** Needed by `SetNoPhysicalTrigger` and `NoPhysicalTriggerListener`
  (pressure plates, tripwire, turtle eggs stepped on while a builder edits). `native_block_step_on` reports entities
  and cannot cancel; `native_trample` cancels but only for farmland. Shape: a cancellable
  `native_block_physical_trigger(&mut self, player: u128, dimension: &str, pos: [i32; 3], state: BlockStateId,
  cancel: &mut bool)` on the player's step, mirroring Bukkit's `PlayerInteractEvent.Action.PHYSICAL`.

- **Game rule and weather setters.** Needed by the world properties Axiom sends (`SetWorldPropertyListener`,
  `world_properties`). Readers exist (`native_level_game_rule`, `native_level_weather`); no setter was found. Shape:
  `native_set_game_rule(&mut self, dimension: &str, rule: &str, value: &str) -> bool` and
  `native_set_weather(&mut self, dimension: &str, raining: bool, thundering: bool) -> bool`.

- **Block entity NBT in bulk.** Needed by `set_buffer` with `allowNbt` (compressed block entities in
  `BlockBuffer`). `edit_blocks` refuses block NBT ("block NBT is not supported here"), and `native_set_block_tag`
  writes one block at a time, outside the run. Shape: a `nbt: Option<Compound>` per run entry in `NativeBlockEdit`,
  written in the same pass and given to `replace_block_entities`. Without it the port falls back to `set_block`
  plus `set_block_entity` per block, which is slow and not atomic.

- **Actor on bulk block changes (CoreProtect logging).** Needed for `log-core-protect-changes` (default true) and the
  CoreProtect integration. `native_block_changed` has no actor, and bulk edits skip per-block hooks (`block_edits.rs`:
  "no block_log entry", the chunk goes to `bulk_log`). Shape: `native_block_changes(&self, dimension: &str,
  actor: Option<u128>, changes: &[([i32; 3], u32, u32)])`, called after each bulk edit. Only needed if change logging
  is wanted; CLAUDE.md says CoreProtect is removed network-wide.

- **Cancel entity spawn, removal and manipulation.** Needed by the Bukkit events `AxiomSpawnEntityEvent`,
  `AxiomRemoveEntityEvent` and `AxiomManipulateEntityEvent`, which other plugins use to veto. `native_summon`,
  `native_remove_entity` and `native_entity_merge_nbt` have no cancel. Shape: a `cancel: &mut bool` on each, through a
  listen-and-cancel hook. Only needed when a region or anti-grief plugin must veto Axiom's entity work.

- **Writable mod data folder.** Needed by blueprint storage (`ServerBlueprintManager`, `BlueprintIo`), which keeps
  blueprints beside the server data, not in the world. Horizon has `native_world_dir` (inside the world: reset with
  it, backed up with it) and `native_data` (read-only, from the mod's resources). Shape:
  `native_mod_data_dir(&self, mod_id: &str) -> Option<PathBuf>`, writable and kept across world resets.

- **Custom blocks and items at runtime.** Needed by `AxiomCustomBlocksAPI` and the `register_custom_block_v2`,
  `register_custom_items` and `custom_blocks` channels. Horizon's `native_blocks` and `native_items` are startup
  lists, and the per-player protocol mapping has no meaning here. Large; not started. Only needed if Axiom's custom
  blocks are wanted.

## Partial: the engine has part of it, and it must change

- **Entity NBT reload and listing for display, marker and interaction kinds.** Needed by `ManipulateEntityPacketListener` and
  `SpawnEntityPacketListener` (NBT on spawn). `native_entity_merge_nbt` refuses any kind it cannot reload (only mobs,
  dropped items, armour stands and item frames), and the docs say displays, markers and interactions are kept without
  simulation (`native_entities.rs`). Shape: extend the reload set in `native_entity_merge_nbt` to those kinds.
  Not verified: that `native_summon` accepts `minecraft:marker` and `minecraft:interaction` by kind.
  Also missing: a call that lists the markers of a dimension for `WorldHost::marker_snapshot` (the port returns none).

- **Chunk loading for far edits, with release.** Needed by Axiom's `RequestChunksOperation` and
  `max-chunk-load-distance`, which load chunks a player's edit reaches. `native_force_chunk_in(owner, dimension, chunk,
  hold)` (`native_chunk_tickets.rs`, API 413) holds a chunk with a ticket and loads it; `native_edit_blocks` skips
  unloaded chunks. Missing: a release per request (the tickets are held until the owner lets go, and Axiom asks per
  chunk) and a "is loaded" answer that does not load. Shape: the port's `WorldHost::request_*` calls use
  `native_force_chunk_in` with an owner per player; the engine should also offer `native_chunk_loaded(dimension, chunk)
  -> bool`, which `native_chunk_snapshot(...).is_some()` only approximates.

- **Entity data by numeric id in any loaded level.** Needed by `RequestEntityDataPacketListener` and
  `MarkerNbtRequestPacketListener`. `native_entity_nbt` takes an `EntityRef`, and `native_entity_by_uuid` covers
  only npc levels. Shape: `native_entity_nbt_in(&mut self, dimension: &str, id: u32) -> Option<Compound>`, through
  `npc_in_level` as `native_merge_entity_nbt` does.

- **Light and send budgets per tick.** Needed by `max-chunk-relights-per-tick` and `max-chunk-sends-per-tick`.
  `edit_blocks` relights the chunk and its neighbours whole at the end of the tick and sends whole chunks, with no cap.
  Shape: a `max_chunks_per_tick: Option<u32>` on `NativeBlockEdit`, the rest deferred to the next tick.

- **Section export for chunk requests.** Needed by `RequestChunkDataPacketListener` and `RequestChunksOperation`, which
  send palette and biome per section. `native_chunk_snapshot` gives one block state or noise biome per position.
  Shape: `NativeChunkSnapshot::section(&self, section_y: i32) -> Option<(Vec<u32>, Vec<u16>, Vec<u16>)>`. This is a
  speed issue, not a blocker: per-position reads work, only slower.

- **Block entity and entity data fixers for old blueprints.** Needed by `DFUHelper`, `BlueprintIo` (block entities,
  entities, palettes with `DataVersion`). `native_upgrade_block_state` exists for block states; no public fixer for
  block entities or entities was found (`horizon_world::datafix` has the tables, not checked). Shape:
  `native_upgrade_block_entity(&self, id: &str, data_version: i32, tag: Tag) -> Tag` and `native_upgrade_entity(...)`.

## Not needed on Horizon (checked, present or not applicable)

- **Client version and ViaVersion.** Horizon serves only its own protocol (`NativeServerPlayer.protocol` says other
  versions are refused), so `ViaVersionHelper` and `UnknownVersionHelper` have nothing to do. The port answers
  `map_client_block_registry` with an error.
- **Payload channels and sizes.** Present: `native_custom_payload_from_client` (every non-`horizon:` channel, in order),
  `native_send_custom_payload_to_player` (any `namespace:path`, up to 1 MiB). The client-to-server limit is vanilla's
  32767, and Axiom's tunnel splits at 31000 (`tunnel-split-size`), so it fits.
- **Fly speed, game mode, teleport, flight, respawn.** Present: `native_set_flying_speed` (from a tick hook),
  `native_set_game_mode`, `native_teleport_player`, `native_set_flight`, `native_respawn_player`.
- **Permissions and commands.** Present: `native_has_permission`, `native_player_permission`,
  `native_permission_check`, `NativeCommand::permission`.
- **Heightmaps and light after an edit.** `edit_blocks` maintains heightmaps (test `heightmaps_follow_the_edit`).
- **Config.** Present: `register_configs`, `config_loading`, `config_reloading` (Server kind).
- **World folder.** Present: `native_world_dir`.

## Added by the glue wiring (2026-11-09, blind; the stubs in mixins/axiom/glue.rs)

- **Entity spawn with the client's uuid.** Needed by `SpawnEntityPacketListener` (`load_entity_tree`,
  `add_fresh_entity_with_passengers`): Axiom names the new entity with its own uuid, and the engine assigns its own.
  `native_summon(dimension, kind, pos, nbt) -> Option<u32>` takes no uuid and returns no uuid. Shape: `native_summon`
  takes the uuid (or honours `UUID` in the NBT) and returns it with the id. Until then spawn is a no-op.

- **Riding, passengers and hanging entities.** Needed by `ManipulateEntityPacketListener` (mounts, `eject_passengers`,
  `start_riding`, `stop_riding`, `vehicle`, `passengers`) and by hanging entities (`set_hanging_turn`,
  `adjust_item_frame_rotation`, `item_frame_on_y_axis`). No engine call for riding or for the orientation of a frame
  was found in `native_entities.rs` / `native_entity_moves.rs`. Shape: `native_mount(vehicle, passenger)`,
  `native_dismount(passenger)`, `native_passengers(entity)`, and `native_set_hanging_facing(entity, facing)`.

- **Block-state properties.** Needed by the `SetBlockHost` queries and writes: block entity type per state
  (`has_block_entity`, `block_entity_type_of`, `block_entity_accepts`), neighbour shape updates
  (`update_from_neighbour_shapes`), scheduled fluid and block ticks (`tick_fluid`, `tick_block`), point-of-interest
  updates (`update_poi`), placed-by (`set_placed_by`) and item components onto block entities
  (`apply_item_to_block_entity`). The port approximates liquids, beds, doors and game master blocks by block name.
  Shape: `native_block_state_props(state) -> { has_block_entity, block_entity_type, fluid }` and a per-write
  `native_block_placed(player, pos, state)` hook that runs the vanilla `setPlacedBy` and neighbour shape rules.

- **Block change acknowledgement.** Needed by `SetBlockPacketListener` (`ackBlockChangesUpTo`): the client waits for
  the server to acknowledge its predicted block changes by sequence number. No engine call sends that ack.
  Shape: `native_ack_block_changes(player, seq: i32)` writing the vanilla `ClientboundBlockChangedAckPacket`.

- **Zstd (resolved).** Blueprint block entities (`BlueprintHost::compress_block_entity` /
  `decompress_block_entity`) and the tunnel's `TUNNEL_PACKET_FLAG_ZSTD_COMPRESSED` use `buffer::zstd_compress` and
  `buffer::zstd_decompress` (pure Rust `ruzstd` 0.9.1, already in Horizon's lock with `twox-hash`). The glue calls them
  in `mixins/axiom/glue.rs`. Blueprint block entities stored by the earlier stub (as given, not compressed) do not decode
  any more. Java decodes the tunnel without a dictionary, so the same `zstd_decompress` reads those frames. Untested
  against a live Axiom client.

- **World whitelist and blacklist.** Needed by `world_allowed_by_config` and `can_modify_world` (`whitelist-world-regex`,
  `blacklist-world-regex` in the config). Fails closed: a configured filter refuses every world. Shape: the regex matcher below.

- **Regex matcher.** Needed by `RestrictionsHost::matches_regex` (Java `String.matches`, the whole text). The `regex` crate
  is not in Horizon's Cargo.lock, so no matcher is wired. Shape: a full-match helper in the engine or an approved crate.

- **Debug text of the restrictions.** Needed by `/axiompaperdebug` (`DebugHost::restrictions`). The per-player restrictions
  are held by the core state, which the host cannot read while the state is borrowed. Shape: a core accessor the glue can call
  before the host is built (a snapshot of the text per player), or a core change that passes the state in.

- **Entity uuid lookup.** Needed by every entity packet, by uuid. The engine's `native_entity(id)` is by numeric id;
  `native_entity_uuid(entity)` is by reference. The port scans `native_entities_in` on each entity packet and each
  request, which is O(entities in the level) per packet. Shape: `native_entity_by_uuid_in(dimension, uuid) -> Option<u32>`
  for every loaded level, backed by an index the engine keeps.
