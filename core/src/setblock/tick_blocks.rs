// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/TickBlocksPacketListener.java

//! TickBlocks packet (`axiom:tick_blocks`): re-run neighbour shape updates and fluid or liquid ticks on a set of
//! positions or a box. Only checks `BUILD_DANGEROUS_TICK`, not the world gate, as in Java.

use std::time::Instant;

use crate::buf::Reader;
use crate::buffer::PositionSet;

use super::host::{BlockPos, SetBlockHost};
use super::{can_use_axiom, SetBlockError, PERM_BUILD_DANGEROUS_TICK, UPDATE_CLIENTS, UPDATE_SKIP_ALL_SIDEEFFECTS};

pub const TICK_TYPE_POSITION_SET: u8 = 0;
pub const TICK_TYPE_BOX: u8 = 1;
/// Java `count > 1048576` triggers the broadcast messages.
pub const MESSAGE_THRESHOLD: i64 = 1_048_576;

/// The area to tick.
pub enum TickArea {
    Positions(PositionSet),
    Box { min: BlockPos, max: BlockPos },
}

/// Decode a TickBlocks packet body (after the permission check, which Java does first).
pub fn parse_tick_blocks(r: &mut Reader) -> Result<(String, TickArea), SetBlockError> {
    let dim = r.read_string(32767)?;
    let area = match r.read_bytes(1)?[0] {
        TICK_TYPE_POSITION_SET => TickArea::Positions(PositionSet::read(r)?),
        TICK_TYPE_BOX => {
            let min = r.read_block_pos()?;
            let max = r.read_block_pos()?;
            TickArea::Box { min, max }
        }
        other => return Err(SetBlockError::UnknownTickType(other)),
    };
    Ok((dim, area))
}

/// Java `TickBlocksPacketListener.onReceive`.
pub fn handle_tick_blocks(host: &mut dyn SetBlockHost, player: u128, r: &mut Reader) -> Result<(), SetBlockError> {
    if !can_use_axiom(host, player, PERM_BUILD_DANGEROUS_TICK) {
        return Ok(());
    }
    let (dim, area) = parse_tick_blocks(r)?;
    if host.player_dimension(player) != dim {
        return Ok(());
    }

    let count: i64 = match &area {
        TickArea::Positions(set) => set.count() as i64,
        TickArea::Box { min, max } => {
            let sx = (max.0 - min.0).abs() as i64 + 1;
            let sy = (max.1 - min.1).abs() as i64 + 1;
            let sz = (max.2 - min.2).abs() as i64 + 1;
            sx * sy * sz
        }
    };
    let show_message = count > MESSAGE_THRESHOLD;
    let name = host.scoreboard_name(player);

    if show_message {
        host.broadcast_system_message(&format!("{name} updated & ticked {count} blocks using Axiom. The server may lag..."));
        let estimated = 1.max(count / 2_097_152);
        host.broadcast_system_message(&format!("Estimated Time (varies depending on server hardware): {estimated}s"));
        if estimated > 30 {
            host.broadcast_system_message("Estimated time is >30s, expect to be kicked from the server");
        }
    }

    let start = Instant::now();
    match &area {
        TickArea::Positions(set) => {
            set.for_each(|x, y, z| tick_position(host, &dim, (x, y, z)));
        }
        TickArea::Box { min, max } => {
            let (min_x, max_x) = (min.0.min(max.0), min.0.max(max.0));
            let (min_y, max_y) = (min.1.min(max.1), min.1.max(max.1));
            let (min_z, max_z) = (min.2.min(max.2), min.2.max(max.2));
            for x in min_x..=max_x {
                for y in min_y..=max_y {
                    for z in min_z..=max_z {
                        tick_position(host, &dim, (x, y, z));
                    }
                }
            }
        }
    }

    if show_message {
        let seconds = (start.elapsed().as_millis() as i64 + 500) / 1000;
        host.broadcast_system_message(&format!("Done updating & ticking blocks (took {seconds}s)"));
    }
    Ok(())
}

/// Java `consumer` lambda: tick one position.
fn tick_position(host: &mut dyn SetBlockHost, dim: &str, pos: BlockPos) {
    let state = host.block_state(dim, pos);
    if host.is_air(state) {
        return;
    }
    if !host.fluid_is_empty(state) {
        host.tick_fluid(dim, pos, state);
    }
    if host.is_liquid_block(state) {
        host.tick_block(dim, pos, state);
    } else {
        let updated = host.update_from_neighbour_shapes(dim, pos, state);
        if updated != state {
            host.set_block(dim, pos, updated, UPDATE_CLIENTS | UPDATE_SKIP_ALL_SIDEEFFECTS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::Writer;
    use crate::setblock::test_support::{Call, FakeHost};
    use crate::setblock::PERM_BUILD_DANGEROUS_TICK;

    fn pack(x: i32, y: i32, z: i32) -> i64 {
        (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
    }

    #[test]
    fn box_tick_updates_only_non_air_blocks() {
        let dim = "minecraft:overworld";
        let mut w = Writer::new();
        w.write_string(dim);
        w.write_bytes(&[TICK_TYPE_BOX]);
        w.write_i64(pack(0, 64, 0));
        w.write_i64(pack(1, 64, 0));
        let body = w.into_bytes();

        let mut host = FakeHost::new(dim);
        host.grant(9, PERM_BUILD_DANGEROUS_TICK);
        host.set(dim, (1, 64, 0), 4);
        handle_tick_blocks(&mut host, 9, &mut crate::buf::Reader::new(&body)).unwrap();

        // (0,64,0) is air and skipped. (1,64,0) gets the neighbour-shape result (4 ^ 1 = 5).
        assert_eq!(
            host.calls,
            vec![Call::SetBlock((1, 64, 0), 5, UPDATE_CLIENTS | UPDATE_SKIP_ALL_SIDEEFFECTS)]
        );
        assert!(host.messages.is_empty());
    }

    #[test]
    fn needs_dangerous_tick_permission() {
        let dim = "minecraft:overworld";
        let mut w = Writer::new();
        w.write_string(dim);
        w.write_bytes(&[TICK_TYPE_BOX]);
        w.write_i64(pack(0, 64, 0));
        w.write_i64(pack(0, 64, 0));
        let body = w.into_bytes();
        let mut host = FakeHost::new(dim);
        host.set(dim, (0, 64, 0), 4);
        handle_tick_blocks(&mut host, 9, &mut crate::buf::Reader::new(&body)).unwrap();
        assert!(host.calls.is_empty());
    }
}
