> **Frozen branch (`horizon-plain`).** This is the version written for horizon-rust/horizon's native hooks / horizon-mixins. We stopped working on it on 2026-11-12: horizon-rust moved all mod API work to the fork [arismoko/horizon](https://github.com/arismoko/horizon) (see "Mod-related pull requests" in its CONTRIBUTING.md) and closed mod pull requests there. This branch is experimental and unsupported. The maintained version, for arismoko/horizon only, is on the default branch.

# horizon-axiom

Unofficial Rust port of the server side of [Axiom](https://axiom.moulberry.com/) (AxiomPaperPlugin, MIT) for
[Horizon](https://github.com/horizon-rust/horizon). Not affiliated with Moulberry; ported with their permission.

Axiom's licensing and whitelist are untouched: the server plugin contains no license check, and this port adds or
removes none. Commercial use of Axiom needs Axiom's Commercial License: https://axiomdocs.moulberry.com/other/commerciallicense.html

**Status:** built and running on Horizon 26.3, with clients from 1.20.5 to 26.3. Needs one engine function upstream
does not have yet; see STATUS.md and GAPS.md.

## Third-party data

`mixins/axiom/core/assets/block_entities_v1.dict` is the zstd dictionary for block entity NBT, copied unchanged from
AxiomPaperPlugin (`src/main/resources/zstd_dictionaries/block_entities_v1.dict`, MIT, Copyright (c) 2023 Moulberry Pty Ltd).
It is embedded with `include_bytes!`. The MIT notice for AxiomPaperPlugin is in [LICENSE](LICENSE).
Compression and decompression use `ruzstd` 0.9 (MIT, pure Rust).
