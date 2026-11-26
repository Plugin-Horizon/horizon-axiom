# horizon-axiom

## Port status

Compared with AxiomPaperPlugin (commit 69c1b0a) (estimate): **47 %** of the original's commands, permissions, config keys, messages and features work today; **54 %** once our open Horizon PRs are merged; **71 %** with the engine changes we are writing; **47 %** with the newest Horizon SDK. Estimated count: ~80/~170 items. Status: not yet compiled after the 2026-11-20 round.

Unofficial Rust port of the server side of [Axiom](https://axiom.moulberry.com/) (AxiomPaperPlugin, MIT) for
[Horizon](https://github.com/arismoko/horizon). Not affiliated with Moulberry; ported with their permission.

Axiom's licensing and whitelist are untouched: the server plugin contains no license check, and this port adds or
removes none. Commercial use of Axiom needs Axiom's Commercial License: https://axiomdocs.moulberry.com/other/commerciallicense.html

> **Built for and working only with [arismoko/horizon](https://github.com/arismoko/horizon)** (the Horizon fork that carries the mod API). [horizon-rust/horizon](https://github.com/horizon-rust/horizon) is experimental for mods and not supported: the earlier version for its native hooks / horizon-mixins is frozen on the `horizon-plain` branch (work stopped on 2026-11-12).

## Which Horizon

This is a native trait mod: `horizon-mod.toml`, a `Cargo.toml` that depends on `horizon-mod-api`, `src/lib.rs` with the hooks, and the plugin logic in the `src/horizon_axiom_core/` module (plain Rust, no engine needed for its tests).

- Pin: `[native] horizon = "54090ee36c4e2c7b43b66db864220c577b94b7e0"` in `horizon-mod.toml` (game `26.3`). That is a development build of arismoko/horizon `main` plus mod API additions that are not all merged upstream yet, so a stock `main` may lack some host methods this mod calls until they land.
- It compiled and loaded on that build and was smoke-tested there; it has not seen a long run with real players. Treat it as experimental.
- Feature descriptions below were written for the earlier horizon-rust version; where they name engine calls (`native_*`) or old file paths, read them as descriptions only. The Known gaps list is current.

## Third-party data

`data/axiom/assets/block_entities_v1.dict` is the zstd dictionary for block entity NBT, copied unchanged from
AxiomPaperPlugin (`src/main/resources/zstd_dictionaries/block_entities_v1.dict`, MIT, Copyright (c) 2023 Moulberry Pty Ltd).
It is embedded with `include_bytes!`. The MIT notice for AxiomPaperPlugin is in [LICENSE](LICENSE).
Compression and decompression use `ruzstd` 0.9 (MIT, pure Rust).

## Build

Build it as a native trait mod with arismoko's launcher (see the "native mods" how-to in the arismoko/horizon docs):

```sh
horizon-launcher native-build <path to this folder> --server-output <dir> --version <tag> --trust-native-code
```

`--trust-native-code` is your consent to compile and run this code inside the server. The logic tests run without the engine: `cargo test`.

## Known gaps

What does not work yet (round 3: the engine families are in the SDK, the `next-engine` feature is gone):

- `disallowed-blocks`: block-state entries and tag entries (`#namespace:tag`, via the engine's block tags) both match.
- Block states are numbered with the game's own ids (`block_state_network_id`): the registry order comes from the exact ids, every state is compared with the derived table at the first host (logged) and the game's ids answer both directions. A mod's blocks are unmapped. Block entity data is read and written; `block_state_block_entity_type` names a state's block entity type. Older clients are still refused (no per-release block mapping).
- Build checks (`player_can_build_at`, asked for each placement and break with `place`/`break`; a section is asked in its eight octants), the tick-blocks packet (`update_from_neighbour_shapes`, `block_tick`, `fluid_tick`), the anti-xray warning (`player_anti_xray_active`), the bypass of the movement checks (`player_move_check`: arcball camera, noclip while flying) and the no-physical-trigger cancel for game events, farmland and turtle eggs (`vanilla_game_event`, `entity_change_block_pre`) are wired. The facing and the frame rotation of hanging entities that are manipulated are read and set (`entity_hanging_direction`, `entity_item_frame_rotation`); a vehicle and its passengers are read from the indexed level.
- Entity spawns: the detached tree is summoned from its NBT (`entity::spawn_nbt`: root data with `UUID`, `Pos`, `Rotation`, passengers nested, each with the UUID the core assigned; `host.summon`); a mod's veto (`entity_join_level`) makes it answer None and the spawn is dropped; the glue is not compiled yet.
- Pressure plates and tripwires stay as they are while only no-trigger players stand on them (`pressure_plate_change_detail`, `tripwire_trigger`); the facing and item rotation of an unspawned hanging entity is turned in its saved data (`hanging_nbt_*`).
- Still missing: wildcard permission nodes (the engine cannot test a wildcard).
- Markers are listed from the level's entities with their custom data (vanilla's own entity keys cut); a marker hidden by Axiom's gizmo flag is still sent, as the flag lives in the core state.
- The game protocol version and the mod data folder are approximated.
- Round 2: game protocol and version names come from the host; explicit permission denies are read with `permission_query_tristate` and sent to the client.
- Permissions are typed nodes. A node also holds when a typed ancestor holds, as plugin.yml's `children` do (`axiom.default` grants `axiom.build.place` through `axiom.build.*`); a wildcard ancestor cannot be asked itself, so only its typed ancestors count. The nodes are every `axiom.*` leaf, `axiom.default`, `axiom.all`, `axiom.debug`, `axiomadmin.bypass_region_checks`, `axiomlimits.<key>`), op-default at level 1 as in `plugin.yml`. A typed query cannot tell an explicit deny from "not set", so explicit denies of a node are not sent to the client, and the wildcard nodes (`axiom.entity.*`, the deprecated `axiom.*`) cannot be queried: a wildcard grant reaches the leaf through the permission handler, and the deprecated-wildcard notice is not sent. `axiomadmin.bypass_region_checks` and `axiomlimits.<key>` default to level 4 (Bukkit: nobody / unset).
- Answers are taken every 20 ticks, so a changed grant takes up to a second to apply.

## Licence

MIT. Based on AxiomPaperPlugin (Copyright (c) 2023 Moulberry Pty Ltd, MIT); see `LICENSE`.
