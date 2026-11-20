# hmod port: axiom
Parity: 44% now / 51% with our PRs (75/170 items now, 87/170 with our PRs; the PR count is an estimate: 14 of the open items are named in GAPS_UPSTREAM.md, not checked one by one; NOT compiled or tested, written blind)

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
