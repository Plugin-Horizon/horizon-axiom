# GAPS: what Horizon must add or change for Axiom to work fully

Checked against the Horizon source at `92fd45976` and the Java reference
(AxiomPaperPlugin, `src/main/java/com/moulberry/axiom/`). Blind: nothing here has been run against a server. The core is tested (`cargo test`); the glue was type-checked (`cargo check`) in a staged copy of upstream/main 216eca2f5 (the pinned revision in horizon-mixin.toml is older), with nothing run.
Items marked "not verified" were not read to the end.

Each bullet: what is missing, the Axiom feature that needs it, and the engine function or hook it would look like.

## Missing: the engine has no function for it

- **Actor on bulk block changes (CoreProtect logging).** Needed for `log-core-protect-changes` (default true) and the
  CoreProtect integration. `native_block_changed` has no actor, and bulk edits skip per-block hooks (`block_edits.rs`:
  "no block_log entry", the chunk goes to `bulk_log`). Shape: `native_block_changes(&self, dimension: &str,
  actor: Option<u128>, changes: &[([i32; 3], u32, u32)])`, called after each bulk edit. Only needed if change logging
  is wanted; CLAUDE.md says CoreProtect is removed network-wide.

- **Cancel entity spawn, removal and manipulation.** Needed by the Bukkit events `AxiomSpawnEntityEvent`,
  `AxiomRemoveEntityEvent` and `AxiomManipulateEntityEvent`, which other plugins use to veto. `native_summon`,
  `native_remove_entity` and `native_entity_merge_nbt` have no cancel. Shape: a `cancel: &mut bool` on each, through a
  listen-and-cancel hook. Only needed when a region or anti-grief plugin must veto Axiom's entity work.

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
  Marker listing is wired: `native_level_markers` (`native_npcs.rs`) feeds `WorldHost::marker_snapshot`,
  `AnnotationHost::markers` and `marker_custom_data`. Open: only the level in place. A marker's uuid comes from
  `native_entity(id)`, which maps ids in the level in place only, so markers of another loaded level are not listed.
  Those traits took `&mut self` for this (core `annotation/mod.rs`, `world/mod.rs`).

- **Entity data by numeric id in any loaded level.** Needed by `RequestEntityDataPacketListener` and
  `MarkerNbtRequestPacketListener`. `native_entity_nbt` takes an `EntityRef`, and `native_entity_by_uuid` covers
  only npc levels. Shape: `native_entity_nbt_in(&mut self, dimension: &str, id: u32) -> Option<Compound>`, through
  `npc_in_level` as `native_merge_entity_nbt` does.

- **Light and send budgets per tick.** Needed by `max-chunk-relights-per-tick` and `max-chunk-sends-per-tick`.
  `edit_blocks` relights the chunk and its neighbours whole at the end of the tick and sends whole chunks, with no cap.
  Shape: a `max_chunks_per_tick: Option<u32>` on `NativeBlockEdit`, the rest deferred to the next tick.
  Wired already (no cap): `relight_chunks` and `light_chunk` call `native_relight_chunks`, `send_chunk_with_light`
  calls `native_resend_chunk` per player, `players_tracking_chunk` calls `native_chunk_trackers`. Not wired on
  purpose: `check_block_light`, since `native_set_block` (`Level.setBlock`) lights each write already.

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

- **Riding, passengers and hanging entities.** Riding in `ManipulateEntity` is wired: `start_riding` calls `native_mount`,
  `stop_riding` `native_dismount`, `eject_passengers` `native_eject_passengers` (`native_travel.rs`). Still open:
  `vehicle`, `passengers` and `is_passenger` have no engine getter, so the core's `remove_passenger` never reaches
  `stop_riding` (it asks `vehicle` first) and the seat checks see nobody riding. Shape: `native_vehicle_of(id) -> Option<u32>`
  and `native_riders_of(id) -> Vec<u32>`. `native_mount` seats only what the engine counts as riders and vehicles (players,
  mobs, boats, minecarts), so a passenger of another kind is refused and logged. Hanging entities: `set_hanging_turn`,
  `adjust_item_frame_rotation` and `item_frame_on_y_axis` are still stubs. Shape: `native_set_hanging_facing(entity, facing)`
  and a facing query.
- **Block-state properties.** Needed by the `SetBlockHost` queries and writes: block entity type per state
  (`has_block_entity`, `block_entity_type_of`, `block_entity_accepts`), neighbour shape updates
  (`update_from_neighbour_shapes`), scheduled fluid and block ticks (`tick_fluid`, `tick_block`), point-of-interest
  updates (`update_poi`), placed-by (`set_placed_by`) and item components onto block entities
  (`apply_item_to_block_entity`). The port approximates liquids, beds, doors and game master blocks by block name.
  Shape: `native_block_state_props(state) -> { has_block_entity, block_entity_type, fluid }` and a per-write
  `native_block_placed(player, pos, state)` hook that runs the vanilla `setPlacedBy` and neighbour shape rules.

- **Block change acknowledgement.** Needed by `SetBlockPacketListener` (`ackBlockChangesUpTo`): the client waits for
  the server to acknowledge its predicted block changes by sequence number. No engine call sends that ack.
  Shape: `native_ack_block_changes(&mut self, player: u128, sequence: i32)` writing the vanilla
  `ClientboundBlockChangedAckPacket`. **Call site:** `AxiomHost::ack_block_changes` in `mixins/axiom/glue.rs`. Upstream Horizon
  does not have the function yet, so the glue builds only against an engine that adds it. Every Axiom edit needs it.

- **Zstd (resolved).** Blueprint block entities (`BlueprintHost::compress_block_entity` /
  `decompress_block_entity`) and the tunnel's `TUNNEL_PACKET_FLAG_ZSTD_COMPRESSED` use `buffer::zstd_compress` and
  `buffer::zstd_decompress` (pure Rust `ruzstd` 0.9.1, already in Horizon's lock with `twox-hash`). The glue calls them
  in `mixins/axiom/glue.rs`. Blueprint block entities stored by the earlier stub (as given, not compressed) do not decode
  any more. Java decodes the tunnel without a dictionary, so the same `zstd_decompress` reads those frames. Untested
  against a live Axiom client.

- **Entity uuid lookup.** Needed by every entity packet, by uuid. `native_entity_by_uuid(uuid)` now exists
  (`native_npcs.rs`, every loaded level, `-> Option<(dimension, id, kind)>`), but it takes `&mut self`, and the core's
  entity lookups (`has_entity`, `is_removed`, `entity_info`, `position`, ...) take `&self`. So the port still scans
  `native_entities_in` per call (`index_dimension`, O(entities in the level)). Open: a `&self` variant of
  `native_entity_by_uuid`, or a core change to `&mut self` on those lookups.

## Resolved (engine functions now used by the glue)

- **Set the time of day.** `native_level_set_day_time` (`native_level_world.rs`), through `native_level`. It sets the
  overworld clock whatever the dimension, as the engine does.
- **Game rule and weather setters.** `native_level_set_game_rule` (`advance_time` for time frozen, `advance_weather`
  for weather paused) and `native_level_set_weather` (`native_level_world.rs`). `weather_paused` reads `advance_weather`
  through `native_level_game_rule`.
- **Write biomes.** `native_set_biomes` (`native_biome_colors/native_biome_edit.rs`), one call per cell, since the core
  writes biomes cell by cell. Each call resends its columns, so a large biome buffer is slow. Batching needs a core
  change. An unknown biome id is refused by the engine and logged once; `biome_exists` only checks for a namespace.
- **Region and plot build check.** `native_may_build` (`native_world_edit.rs`) for `can_place_block`, `can_break_block`,
  `region_can_place`, `region_can_break`, and `region_section` (one check per block of the section). The bypass
  permission stays in core. Still open: the debug `can_break_block` / `can_place_block` take no position
  and stay stubs.
- **Cancel a physical trigger.** `native_player_physical_trigger` (`native_hooks.rs`), with the `physical-trigger`
  injection in `horizon-mixin.toml` that sets `cancel` for players with no-physical-trigger on.
- **Chunk loading for far edits, with release.** `native_chunk_load_request`, `native_chunk_load_state`,
  `native_chunk_load_release` and `native_chunk_is_loaded` (`native_chunk_tickets.rs`). The glue counts holds per chunk
  and asks for the ticket once. Core now calls `release_chunk` (requests) and `release_chunk_load` (buffers) when a
  chunk is done or the operation ends. Known gap: a request dropped when its player leaves releases only what it
  had in flight; a buffer dropped without finishing keeps its holds until restart.
- **Markers, chunk trackers, resend and relight, permission tri-state.** `native_level_markers`,
  `native_chunk_trackers`, `native_resend_chunk`, `native_relight_chunks` and `native_player_permission_value` are in
  the glue (`permission_value`, the chunk and marker hosts). Core: `markers`, `marker_custom_data` and
  `marker_snapshot` take `&mut self` now, a signature change the engine forced. Core `cargo test --offline`: 301 pass.
- **Block entity NBT in bulk.** `set_buffer` with `allowNbt` loads each chunk's block entities in one
  `native_edit_blocks` call (`NativeBlockEdit.block_entities`, SNBT from `horizon_client::snbt::to_snbt`), after that
  chunk's blocks are written. The core's `SetBlockHost::load_block_entities` replaces the per-block call. The block
  states still go through `set_block_raw`, not through the edit. Untested: upstream refuses the whole edit if one
  entry does not parse.
- **Writable mod data folder (blueprints).** Blueprints load from `native_data_dir("axiom")/blueprints`
  (`horizon-mods/.horizon-mod-data/axiom/`), outside the world, so a world reset keeps them. Annotations stay in the
  world folder on purpose: they belong to their world. Blueprints saved under `<world>/axiom/blueprints` by an older
  build are not moved.
- **Entity spawn with the client's uuid.** `entity::build_detached_tree` (core) builds the tree with a UUID per entity.
  The glue keeps it as a pending tree until `add_fresh_entity_with_passengers`, which summons each entity with
  `native_summon_with_uuid` at the pose `place_tree` gave it, snaps and turns it, then mounts the passengers with
  `native_mount`. A failed summon removes what was summoned of that tree. A refused mount is logged and the passenger
  stays beside its vehicle. Untested against a client.
- **Regex matcher.** Core `regex.rs`: a Pike VM (no backtracking, no crate) with Java `String.matches` semantics. It
  covers the syntax in its module comment; anything else (possessive quantifiers, backreferences, lookaround, flags,
  `\p`, nested classes, `&&`) is a syntax error. `matches_regex` and `world_filters_pass` (glue) fail closed.
- **World whitelist and blacklist.** Wired through the regex matcher. An invalid pattern refuses every world. The filters
  match the Paper world name for the three vanilla dimensions (`world`, `world_nether`, `world_the_end`), as Java matches
  `World#getName`; other dimensions use their id. Open: a Paper config means the same worlds here only if Horizon's
  world folders have Paper's names.
- **Debug text of the restrictions.** Core `Restrictions::java_text` (Java `toString`; enum names derived from the
  variant names, to check against the Java constants) and `RestrictionsState::restrictions_text`. The glue takes the
  sender's text before it builds the host. Other players get none.

Core `cargo test --offline`: 343 pass (adds the regex, detached-tree and restrictions-text tests).

## Bundle notes (2026-11-11)

- **Bundle with LuckPerms (Axiom item 7).** `axiom.*` nodes resolve through `native_has_permission`, so the LuckPerms
  port must be in the same engine build. The bundle list is composed outside this repo.
- **Region protection (Axiom item 8).** `native_may_build` (`native_world_edit.rs`) denies only when a mod's
  `can_build` hook cancels. Axiom registers no `can_build` hook, so with no region mod loaded every edit is allowed.
- **Original config file.** `plugins/AxiomPaper/config.yml` is read at server start (`register_config`), via the
  vendored `yaml.rs` (`horizon-yaml`, unchanged). Changes to it take effect at the next start, as Paper's do. Without
  the file, `config/axiom-server.toml` is the fallback.

