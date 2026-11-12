// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/SetBlockPacketListener.java,
// packet/impl/SetBlockBufferPacketListener.java, packet/impl/TickBlocksPacketListener.java,
// operations/SetBlockBufferOperation.java, operations/OperationQueue.java, operations/PendingOperation.java,
// integration/SectionPermissionChecker.java, integration/Integration.java, integration/Box.java,
// integration/BoxWithBoolean.java, listener/NoPhysicalTriggerListener.java, event/AxiomModifyWorldEvent.java

//! Set-block module: SetBlock, SetBlockBuffer and TickBlocks packets, the pending operation queue, section
//! permission bounds, the modify-world gate and the no-physical-trigger set.
//! The game glue implements [`SetBlockHost`] and calls the handlers from its packet and tick hooks.
//!
//! Not ported here: the CoreProtect logging (no CoreProtect equivalent in Horizon),
//! the ServerHeightmaps packet (not set-block work, belongs with the handshake code) and the Bukkit event classes.

pub mod block_packet;
pub mod buffer_packet;
pub mod dispatch;
pub mod host;
pub mod integration;
pub mod permission;
pub mod physical;
pub mod queue;
pub mod tick_blocks;

#[cfg(test)]
pub(crate) mod test_support;

use std::fmt;

use crate::buf::BufError;
use crate::buffer::BufferError;

pub use block_packet::{BlockHit, Hand, SetBlockPacket};
pub use host::{BlockPos, ChunkLoadState, SetBlockHost};
pub use permission::{BoxWithBoolean, LocalBox, SectionPermissionChecker};
pub use queue::{OperationQueue, PendingOperation};

/// `BlockPlaceContext` reasons: replace mode and angel placement skip the Bukkit events.
pub const REASON_REPLACE_MODE: i32 = 1;
pub const REASON_ANGEL: i32 = 128;

/// Block update flags, as in Java `Block`: `UPDATE_NEIGHBORS = 1`, `UPDATE_CLIENTS = 2`, `UPDATE_KNOWN_SHAPE = 16`,
/// `UPDATE_SKIP_ALL_SIDEEFFECTS = 816`. The glue maps them to the Horizon equivalents inside `set_block`.
pub const UPDATE_CLIENTS: u32 = 2;
pub const UPDATE_SKIP_ALL_SIDEEFFECTS: u32 = 816;
/// `setBlock(pos, state, 3)`: neighbours and clients.
pub const FLAGS_WITH_NEIGHBOR_UPDATES: u32 = 3;
/// `setBlock(pos, state, 18)`: clients and known shape, no neighbour updates.
pub const FLAGS_WITHOUT_NEIGHBOR_UPDATES: u32 = 18;

/// Java `SetBlockBufferOperation.MAX_CHUNK_FUTURES`.
pub const MAX_CHUNK_FUTURES: usize = 256;
/// Java `setBlock` broadcasts a "large change" log line above this many blocks.
pub const LARGE_CHANGE_LOG_THRESHOLD: usize = 64;

/// `AxiomPermission` nodes used by this module (Java `AxiomPermission.*.permission`).
pub const PERM_BUILD_PLACE: &str = "axiom.build.place";
pub const PERM_BUILD_SECTION: &str = "axiom.build.section";
pub const PERM_BUILD_NBT: &str = "axiom.build.nbt";
pub const PERM_BUILD_DANGEROUS_TICK: &str = "axiom.build_dangerous.tick";

/// Settings read by the handlers. The glue fills these from its config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetBlockConfig {
    /// Java `packetCollectionReadLimit`: maximum entries in one SetBlock collection.
    pub packet_collection_read_limit: usize,
    /// Java `logLargeBlockBufferChanges`.
    pub log_large_block_buffer_changes: bool,
}

#[derive(Debug)]
pub enum SetBlockError {
    Buf(BufError),
    Buffer(BufferError),
    CollectionTooLarge { count: usize, max: usize },
    UnknownBlockId(i32),
    UnknownHand(i32),
    UnknownBufferType(u8),
    UnknownTickType(u8),
    ChunkLoadFailed { x: i32, z: i32 },
}

impl From<BufError> for SetBlockError {
    fn from(e: BufError) -> Self {
        SetBlockError::Buf(e)
    }
}

impl From<BufferError> for SetBlockError {
    fn from(e: BufferError) -> Self {
        SetBlockError::Buffer(e)
    }
}

impl fmt::Display for SetBlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetBlockError::Buf(e) => write!(f, "malformed packet: {e:?}"),
            SetBlockError::Buffer(e) => write!(f, "malformed buffer: {e:?}"),
            SetBlockError::CollectionTooLarge { count, max } => {
                write!(f, "Collection size {count} is too large; max {max}")
            }
            SetBlockError::UnknownBlockId(id) => write!(f, "unknown block id {id}"),
            SetBlockError::UnknownHand(id) => write!(f, "unknown hand {id}"),
            SetBlockError::UnknownBufferType(t) => write!(f, "Unknown buffer type: {t}"),
            SetBlockError::UnknownTickType(t) => write!(f, "Unknown type: {t}"),
            SetBlockError::ChunkLoadFailed { x, z } => write!(f, "chunk {x},{z} failed to load"),
        }
    }
}

/// Java `AxiomPaper.canUseAxiom(player, perm)`: the player is active and holds the permission.
pub fn can_use_axiom(host: &dyn SetBlockHost, player: u128, node: &str) -> bool {
    host.is_axiom_active(player) && host.has_axiom_permission(player, node)
}

/// Java `AxiomPaper.canModifyWorld(player, world)` with the `AxiomModifyWorldEvent` callback.
/// The event's default is cancelled unless the target world is the one the player is in. The glue's listeners
/// may change that.
pub fn can_modify_world(host: &mut dyn SetBlockHost, player: u128, dim: &str) -> bool {
    if !host.world_allowed_by_config(dim) {
        return false;
    }
    let default_cancelled = host.player_dimension(player) != dim;
    !host.modify_world_event(player, dim, default_cancelled)
}

/// Java `UUID.toString()` format, used in log lines.
pub fn uuid_string(u: u128) -> String {
    let h = format!("{u:032x}");
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}
