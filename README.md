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

What does not work yet on arismoko/horizon (each is marked `TODO(arismoko-api: ...)` in `src/lib.rs`):

- `disallowed-blocks`: block-state entries match, tag entries (`#namespace:tag`) do not (no block tag table in the port).
- `/axiompaperdebug` tab-completes its subcommands, but the usages are listed to every player (Brigadier hides them from senders without the permission).
- Block states use the vanilla numbering derived at runtime from the engine's own block registry (`block_keys` in registry order, each block's `definition`, properties sorted by name, first one slowest); no game data table ships with the mod. A mod's blocks are unmapped. Block entity data is read and written; there is no block state data fixer, no block entity type of a state, and older clients are refused (no per-release block mapping).
- Sections are read cell by cell; `update_from_neighbour_shapes` and the tick-blocks packet do nothing (no API).
- No cancel hook for Axiom's entity spawns; hanging-entity facing is not read or set.
- Markers are listed from the level's entities with their custom data (vanilla's own entity keys cut); a marker hidden by Axiom's gizmo flag is still sent, as the flag lives in the core state.
- Region and plot checks (WorldGuard-style) are not seen.
- The game protocol version and the mod data folder are approximated.
- Permissions are typed nodes. A node also holds when a typed ancestor holds, as plugin.yml's `children` do (`axiom.default` grants `axiom.build.place` through `axiom.build.*`); a wildcard ancestor cannot be asked itself, so only its typed ancestors count. The nodes are every `axiom.*` leaf, `axiom.default`, `axiom.all`, `axiom.debug`, `axiomadmin.bypass_region_checks`, `axiomlimits.<key>`), op-default at level 1 as in `plugin.yml`. A typed query cannot tell an explicit deny from "not set", so explicit denies of a node are not sent to the client, and the wildcard nodes (`axiom.entity.*`, the deprecated `axiom.*`) cannot be queried: a wildcard grant reaches the leaf through the permission handler, and the deprecated-wildcard notice is not sent. `axiomadmin.bypass_region_checks` and `axiomlimits.<key>` default to level 4 (Bukkit: nobody / unset).
- Answers are taken every 20 ticks, so a changed grant takes up to a second to apply.

## Licence

MIT. Based on AxiomPaperPlugin (Copyright (c) 2023 Moulberry Pty Ltd, MIT); see `LICENSE`.
