# STATUS: horizon-axiom

Built and running on a Horizon server (26.3) with clients of every release from 1.20.5 to 26.3.

## Engine requirement

The glue calls `native_ack_block_changes(player, sequence)` (vanilla `ClientboundBlockChangedAckPacket`), which
upstream Horizon does not have yet (open as horizon-rust/horizon#14281; GAPS.md, "Block change acknowledgement"). Until it lands, the bundle builds only
against an engine that adds it.

## What works

- Every Axiom edit packet (`set_buffer`, `set_block`, blueprints, annotations, restrictions) for 26.3 clients.
- Clients of older releases through Horizon's protocol adapter: block ids in buffers are mapped from the client's
  release to 26.3 (`map_client_block_registry`, `ids.block_state.up`), and the client's own palette bit width is used.
- Section storage as vanilla writes it since 1.21.5 (no long count); older Axiom protocols keep the counted form
  (`LongArrays::for_protocol`). Checked against buffers written and read back by the vanilla 26.3 classes
  (`core/tests/fixtures/vanilla_26_3`).
- `void_air` marks the cells a buffer leaves alone, as in AxiomPaper.
- The original `plugins/AxiomPaper/config.yml` is read at server start (`register_config`); without it,
  `config/axiom-server.toml` is the fallback.

## Checking

    cd mixins/axiom/core
    cargo test
    cargo run --example check_original -- <server>/plugins/AxiomPaper/config.yml

## Open

See GAPS.md (block entities, neighbour updates, disallowed blocks, permission inheritance). Config changes apply at
the next server start; there is no reload of the YAML.
