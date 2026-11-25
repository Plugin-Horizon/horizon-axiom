# hmod port: axiom
Parity: 47% now / 54% with our PRs (80/170 items now, 92/170 with our PRs; the PR count is an estimate: 14 of the open items are named in GAPS_UPSTREAM.md, not checked one by one; NOT compiled or tested, written blind)

Round 3 (2026-11-25, engine families, blind, not compiled)
- The `next-engine` feature is gone (anti-xray is normal code). Build checks: `region_can_break/place`, `region_section`, `can_place_block`/`can_break_block` of the core host traits take `&mut self` (a hook is fired inside `player_can_build_at`); `AxiomHost::build_allowed` is the player's flag and the protection mods' answer. Tick blocks: `update_from_neighbour_shapes` (`&mut self` now), `tick_block`, `tick_fluid` through the level calls. Block ids: exact (`block_state_network_id`), registry sorted by the exact ids, a startup comparison with the derived numbering (`BlockTable::check`), `EXACT_STATES` for id to state. Hooks: `player_move_check` (HookSets: bypass node and flying per tick), `entity_change_block_pre` and `vanilla_game_event` (no-physical-trigger players). Hanging: `set_hanging_turn`, `adjust_item_frame_rotation`, `item_frame_on_y_axis`; `vehicle`/`passengers` reverse-look the indexed uuids. `block_entity_type_of` uses `block_state_block_entity_type`.
- Open: the entity spawn tree is still refused; pressure plates / tripwires; detached tree facing; wildcard nodes.
Round 100% (2), written blind, not compiled
- Fixed: `/axiompaperdebug` has one usage per `AxiomDebugCommand` literal (`canUseAxiom`, `isMismatchedDataVersion`, `canModifyWorld`, `isClientListening <channel>`, `hasPermission <permission>`, `getRestrictions`, `canBreakBlockAtCurrentPosition [plotsquared|worldguard]`, `canPlaceBlockAtCurrentPosition [plotsquared|worldguard]`, `isPlotWorld`, `getCurrentEditablePlot`), so the subcommands tab-complete. The handler rebuilds the subcommand line from the literal path and the text argument (`debug_input`); the core's answers are unchanged.
- Fixed: `disallowed-blocks` (was read, never used). `restrictions/disallowed.rs` parses the entries (`minecraft:oak_stairs[waterlogged=true]`, a bare name for every state, a missing namespace means `minecraft`) and matches a state string; `block_state_from_wire` answers void air for a disallowed client state, as Java's client registry does. Tags (`#namespace:tag`) and entries Java's parser refuses are dropped.
- Fixed: plugin.yml children. A node also holds when one of its wildcard parents is held as a typed node, so `axiom.default` gives `axiom.build.place` through `axiom.build.*` (`with_parents`, used by `refresh_permissions`). Bukkit does this through `children`; before, only the leaves LuckPerms answered counted.
- Fixed: kick texts of the packet errors. A failing handler kicks with `Error while processing packet <channel>: <error>` (AxiomPaper's `WrapperPacketListener`; the error text is the core's, not Java's exception message). The zstd failure reads `Axiom: Zstd failed to decompress: <error>`. The block-change error keeps AxiomPaper's wording with the typo (`An error occured while processing block change: ...`), and a biome buffer that fails says `biome change`.
- Gaps closed as API-blocked and written to GAPS_UPSTREAM.md (axiom lines): block entity type of a state, data fixers for old-version data, neighbour shape update and scheduled tick, region/plot protection query, anti-xray flag per player, cancellable physical trigger, entity type lookup by id, server protocol version and older-client block maps.
- Unit tests (literal values): `with_parents` (parent grant reaches its children, a child grant does not grant its parent), `disallowed` (bare name, namespace, named properties, dropped entries, empty list, state splitting), and the lib-level disallowed check.

Checklist round 100 (walked from plugin.yml, config.yml, AxiomPaper.java, commands/, packet/impl/, listener/, restrictions/)
Commands
- [x] `/axiompaperdebug`: 10 subcommands, the 4 that take a region or plot literal (2 each) and their replies (text as Java's `Component.text`).
- [x] `/axiompaperdebug` sender check: op, `axiom.all`, `axiom.debug`, the debug UUID (kept as in the original, see Known gaps).
- [ ] Brigadier `requires`: Java hides the usages from a sender who may not run them; the port lists them to everyone and the handler answers. Needs a node-based or predicate-based visibility in the command API (not in the reference); not closable here.
- [x] `/axiompapermigrateconfig`: op (level 1) and console; messages as Java.
- [ ] `canBreak/canPlaceBlockAtCurrentPosition plotsquared|worldguard`: the answers are the generic ones, the region plugins are API-blocked (GAPS_UPSTREAM.md).
- [ ] `isPlotWorld` / `getCurrentEditablePlot`: always false / empty (PlotSquared API-blocked).
Config keys (config.yml, all 24 keys including `limits`; `allow-large-chunk-data-request` is read by no Java code, so it has no behaviour on either side)
- [x] Parsed with Java's defaults: the chunk, tunnel, world-filter, entity, marker, annotation, block and reach keys (`restrictions/config.rs`).
- [x] `disallowed-blocks` (this round).
- [x] `whitelist-world-regex` / `blacklist-world-regex` with Java's `matches` (full match, blank = off).
- [x] `allow-teleport-between-worlds`, `blueprint-sharing` (the blueprint channels are served only when on), `allow-annotations`, `send-markers`, `disable-entity-sanitization`.
- [x] `incompatible-data-version` / `unsupported-axiom-version` as kick / warn / ignore (unknown means kick).
- [ ] `whitelist-entities` / `blacklist-entities`: applied, but Java drops names the registry does not know, the port keeps them (GAPS_UPSTREAM.md, entity type lookup).
- [ ] `log-core-protect-changes`: parsed, nothing logged. Decision: CoreProtect is removed network-wide (CLAUDE.md).
- [x] `limits.<key>.block-buffer-rate-limit` (`example` skipped) and `axiomlimits.<key>` as typed nodes.
- [ ] `incompatible-data-version` with a client on another data version: `is_mismatched_data_version` is always false (no client data version in the port; API-blocked for the ViaVersion maps).
Messages
- [x] Handshake goodbyes and sends: outdated Axiom, server not updated, invalid handshake ID, missing `axiom.use`, data version, Axiom+ViaVersion, ViaVersion mismatch warning, `You don't have permission to use Axiom ...`.
- [x] Kicks: split buffer too large, unregistered packet, packet too large, too fast, malformed tunnel buffer, packet error (this round).
- [x] Tick block broadcast (`updated & ticked`, estimated time, `expect to be kicked`, `Done updating & ticking blocks`).
- [x] Config migration texts (`No migration necessary`, backup, migrated, the internal errors) and the clickable hint.
- [x] Blueprint feature text (`Axiom+ViaVersion: This feature isn't supported ...`).
- [x] Deprecated `axiom.*` notice: text kept, unreachable (wildcards cannot be asked, see Known gaps).
- [ ] Anti-xray warning: text kept, never sent (no anti-xray API, GAPS_UPSTREAM.md).
Permission nodes
- [x] Every leaf of plugin.yml declared, op default (level 1); `axiomadmin.bypass_region_checks` and `axiomlimits.*` at level 4.
- [x] plugin.yml children (this round).
- [ ] Explicit deny and wildcard queries (GAPS_UPSTREAM.md, tri-state query).
Listeners and channels (packet/impl, listener, AxiomPaper.onEnable)
- [x] Every serverbound channel is served when Java registers it (`served`: blueprint sharing, annotations).
- [x] The 20 listeners map to the core's handlers; the permission each one checks is the same node name (`axiom.*` as in plugin.yml).
- [ ] `NoPhysicalTriggerListener`: the cancel is API-blocked (GAPS_UPSTREAM.md).
- [ ] Tick-blocks neighbour shape updates and block ticks: API-blocked.
- [ ] Block entity type of a state, data fixers: API-blocked.
- [ ] Entity spawn cancel and hanging facing: API-blocked (existing GAPS_UPSTREAM.md lines).
- [ ] CoreProtect logging: not ported (decision, see above).
Heightmaps, markers, world properties, custom blocks, displays
- [x] Server heightmaps (round 2); markers listed with their custom data (round 2).
- [ ] Markers hidden by Axiom's gizmo flag still listed (flag in core state).

Round 2 (written blind, not compiled)
- Fixed: colours. `send_message` maps `MessageColor` to the named colours `red` / `yellow` through `TextStyle::color`; the
  join notice and `/axiompapermigrateconfig` lines use the colours of AxiomPaper (yellow, red, green). Plain `tell` is unchanged.
- Fixed: the config-migration hint is clickable: a green bold "CLICK HERE" with `RunCommand("/axiompapermigrateconfig")` and the
  command as hover text, then yellow " to migrate the config", as `AxiomPaper`'s join handler.
- Fixed: server heightmaps. Image files directly in `plugins/AxiomPaper/heightmaps` are read at the first join (Java: at enable), files over
  2,000,000 bytes are skipped with the same log line, and each is sent as `axiom:add_server_heightmap` (name, then bytes) after the
  blueprint manifest, as `sendTo`. Image test: the leading bytes of PNG, JPEG, GIF and BMP, not a decode, so a damaged image with a valid
  header is sent where Java skips it.
- Fixed: markers. `marker_snapshot`, `markers` and `marker_custom_data` list the level's `minecraft:marker` entities (`level.all_entities`,
  `entity.nbt`, vanilla's own entity keys cut from the root so the compound is the custom data as `MarkerData.getData` gives it) and encode
  them with `encode_marker`. Gap: markers hidden by Axiom's gizmo flag are still listed (the flag is in the core state, not in the host).
- Unit tests: `looks_like_image` (literal bytes) and the missing-folder case of `load_server_heightmaps`.
- Still open (see round 100%): region checks, explicit deny, wildcard nodes, entity spawn cancel, hanging facing, the API items in GAPS_UPSTREAM.md.

Round 100%
- Fixed: `block_state_from_wire` no longer refuses every state. `src/block_ids.rs` derives the vanilla numeric block state ids at runtime from the engine's block registry (`block_keys()` order, `block_states().definition(block)` properties sorted by name, first slowest; built lazily on the first `AxiomHost`, cached; only `minecraft:` blocks are numbered; no Mojang data table in the repo). Risk: the numbering is right only if the registry order and each property's value order match vanilla's (see GAPS_UPSTREAM.md); vanilla 26.3 should give 35723 states / 16 global bits. Ids convert to engine states through `block_state_id(string)` and back through the block's name and properties, cached per thread.
- Fixed: `set_block_raw` / `set_block` write through `LevelRef::set_block` (Java update bits mapped to `BlockUpdateFlags`); heightmaps, block entity creation and light are the engine's. `state_at`, `section_read`, `has_block_entity`, `state_can_be_replaced`, `fluid_is_empty`, `is_liquid_block`, `is_bed_double_plant_or_door`, `is_game_master_block`, `empty_state` (void_air) now answer from real data.
- Fixed: block entity NBT read (`block_entity_nbt`, `section_block_entities`) and write (`load_block_entities`, merged over the existing data); blueprint states (`state_to_nbt`, `state_from_nbt`, `default_state`), block entity / entity data fixers (`upgrade_*_nbt`), `data_version`.
- Open (blocked by API, listed in GAPS_UPSTREAM.md): region/plot checks (only `may_build`), block entity type of a state, block state data fixer, per-section bulk read, `update_from_neighbour_shapes` / tick-blocks, protocol names and older-client block maps, entity spawn cancel, hanging facing, markers, colours.
- Unsure (not compiled): `entity.data` / `replace_data` / `kind.id` signatures taken from the worldedit port; `block_states().replaceable` availability.

Round 1
- Fixed: src/lib.rs `entity_type_id`, `saved_entity`, `entity_info`: `EntityTypeKey` now mapped to its id string with `kind.id(host)` (was `as_str` / used as `String`).
- Fixed: src/lib.rs `Inbox` payload handler: `handle` returns `PayloadHandleError` (import added), context error wrapped in `PayloadHandleError::Failed`.
- Fixed: src/lib.rs `kick_player`: `PlayerRef::kick` is now `PlayerRef::disconnect` (returns `Result<bool, TextFormError>`).
- Fixed: src/lib.rs `permission_fallback`: `PlayerRef::has_permission` removed; operator-level fallback (level 2) kept via `permission_level`.
- Fixed: src/lib.rs `RestrictionsHost::permission_value`: `PlayerRef::permission_value` removed, always `None` (see gap).
- Permission nodes: none declared. String nodes (`axiom.*`, `axiomlimits.<key>`) are not migrated to typed nodes; see gap.
- Gaps: HMOD-GAP(permission-provider-answers): a typed node query (`permission_bool`) needs `&mut dyn ServerHost`, but every permission check in the core runs through `&self` host trait methods (`RestrictionsHost`, `EntityHost`, `RequestHost`, `AnnotationHost`, ...). Provider answers (LuckPerms) are lost. Only the operator-level fallback (level 2) remains, so non-operators get no axiom.* permission and Axiom is effectively operator-only. This guts the non-operator path; the fix needs the core traits to take `&mut self` (a refactor) or interior mutability of the host, which is not done here.
- Unsure:
  - `PayloadHandleError` is imported from the `mod_api` root (assumed re-exported there, as worldedit does).
  - `PlayerRef::disconnect` and `EntityTypeKey::id` are inferred from the generator's `player_<x>` / `entity_type_key_id` naming, not compiled.
  - The gap stubs keep the old operator-level-2 fallback in `permission_fallback`; that level was taken from the old comment.

Audit fixes
- Fixed: `permission_fallback` no longer answers `true` for `axiom.default` / `axiom.use` or any operator at level 2: every `axiom.*` leaf, `axiom.default`, `axiom.all`, `axiom.debug`, `axiomadmin.bypass_region_checks` and the `axiomlimits.<key>` nodes of the config are typed Boolean nodes (op-default level 1), declared in `permission_gather_nodes`, asked through `permission_bool` every 20 ticks and kept for the `&self` host traits; a changed answer calls `on_permissions_changed`.
- Fixed: `/axiompapermigrateconfig` now migrates `config.yml` (backup to `config.yml.bak`, shipped default with the set values, `src/config_migrate.rs`) and needs operator level 1; ops are told when the config is outdated.
- Fixed: chunk holds use the ticket controller `axiom:chunks` (`set_mod_forced`), old tickets are released once at start.
- Permission nodes: see above, declared in `Mod::permission_gather_nodes`.
- Gaps: HMOD-GAP(permission-explicit-deny): typed query cannot tell deny from unset; wildcard nodes cannot be queried; `block_state_from_wire` still refuses every state (set_block / set_buffer write nothing until the engine has numeric state ids; not runtime-tested here).
