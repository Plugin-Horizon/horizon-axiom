// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/RequestChunkDataPacketListener.java,
// packet/impl/RequestEntityDataPacketListener.java, operations/RequestChunksOperation.java.
//
// Chunk and entity data requests. A request is either answered at once or becomes a `RequestChunksOperation`
// that the glue ticks until the needed chunks have loaded. The glue implements `RequestHost`.
// The MarkerNbtRequestPacketListener is marker-specific and is left to the annotation module.

mod codec;

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

use crate::buf::{BufError, Nbt, Reader};

pub use codec::{
    pack_block_pos, pack_chunk_pos, unpack_block_pos, unpack_chunk_pos, BlockStates,
    CompressedBlockEntity, MAX_PACKET_SIZE, MIN_POSITION_LONG, SECTION_VOLUME,
};

/// Incoming channel for chunk requests.
pub const REQUEST_CHUNK_CHANNEL: &str = "axiom:request_chunk_data";
/// Incoming channel for entity data requests.
pub const REQUEST_ENTITY_CHANNEL: &str = "axiom:request_entity_data";
/// Outgoing channel for chunk data responses.
pub const RESPONSE_CHUNK_CHANNEL: &str = "axiom:response_chunk_data";
/// Outgoing channel for entity data responses.
pub const RESPONSE_ENTITY_CHANNEL: &str = "axiom:response_entity_data";

/// Permission node of `AxiomPermission.CHUNK_REQUEST`.
pub const PERMISSION_CHUNK_REQUEST: &str = "axiom.chunk.request";
/// Permission node of `AxiomPermission.CHUNK_REQUESTBLOCKENTITY`.
pub const PERMISSION_CHUNK_REQUEST_BLOCK_ENTITY: &str = "axiom.chunk.request_block_entity";
/// Permission node of `AxiomPermission.ENTITY_REQUESTDATA`.
pub const PERMISSION_ENTITY_REQUEST_DATA: &str = "axiom.entity.request_data";

/// Maximum number of chunk loads in flight per operation (Java `MAX_CHUNK_FUTURES`).
pub const MAX_CHUNK_FUTURES: usize = 256;
/// Entity type of players. Players are never sent (Java `entity instanceof Player` and `canEntityBeManipulated`).
pub const PLAYER_ENTITY_TYPE: &str = "minecraft:player";
/// Max length of a resource key string on the wire (Identifier limit).
const MAX_IDENTIFIER_LEN: usize = 32767;

/// What the host can tell about one section.
pub enum SectionRead {
    /// Chunk not loaded, or the section index is outside the chunk's section range. Skipped silently.
    Missing,
    /// The section holds only air. Sent as "no container".
    Empty,
    /// Block states of the section, vanilla index order `(y << 8) | (z << 4) | x`.
    Blocks(Box<BlockStates>),
}

/// Basic facts about an entity, looked up before any NBT is saved.
pub struct EntityInfo {
    /// Namespaced entity type id, for example `minecraft:armor_stand`.
    pub entity_type: String,
    /// Block position of the entity (floored coordinates).
    pub block_pos: (i32, i32, i32),
}

/// Everything the request code needs from the game. Players are `u128` UUIDs, dimensions are ids such as
/// `minecraft:overworld`, block states are global `u32` ids.
pub trait RequestHost {
    /// Player has an active Axiom session (handshake done).
    fn is_axiom_active(&self, player: u128) -> bool;
    /// Permission check by node. Operators pass, as `AxiomPlugin.hasPermission` does.
    fn has_permission(&self, player: u128, node: &str) -> bool;
    /// Player runs a protocol version that does not match the server data, so requests get empty replies.
    fn is_mismatched_data_version(&self, player: u128) -> bool;
    /// Player is online. False means the operation is dropped without a reply.
    fn is_player_online(&self, player: u128) -> bool;
    /// Dimension the player is in, or `None` when offline.
    fn player_dimension(&self, player: u128) -> Option<String>;
    /// Block position of the player.
    fn player_block_pos(&self, player: u128) -> (i32, i32, i32);
    /// Axiom may modify this world for this player (world regexes, `AxiomModifyWorldEvent` veto).
    fn can_modify_world(&self, player: u128, dimension: &str) -> bool;
    /// Protection check for one block (Integration.canPlaceBlock).
    fn can_place_block(&self, player: u128, dimension: &str, x: i32, y: i32, z: i32) -> bool;
    /// Chunk distance (Manhattan, in chunks) within which chunks are loaded. Zero or less means
    /// "render-distance only". Plot worlds report 0.
    fn max_chunk_load_distance(&self, dimension: &str) -> i32;
    /// Max entries in an incoming id list (`packetCollectionReadLimit`).
    fn packet_collection_read_limit(&self) -> usize;
    /// Config: entity type may be listed, not on the player blacklist (`canEntityBeManipulated`).
    fn can_entity_be_manipulated(&self, entity_type: &str) -> bool;
    /// Height range of the dimension as `(min_y, height)`.
    fn build_height(&self, dimension: &str) -> (i32, i32);
    /// Bit width of the global block state id map (`ceil(log2(number of block states))`).
    fn block_state_global_bits(&self) -> u8;
    /// Starts loading a chunk if needed. Idempotent, may complete later.
    fn request_chunk(&mut self, dimension: &str, chunk_x: i32, chunk_z: i32);
    /// Lets go of the hold `request_chunk` took, once the chunk is read or the request ends (also when the player
    /// leaves with chunks in flight). Called once per chunk requested.
    fn release_chunk(&mut self, dimension: &str, chunk_x: i32, chunk_z: i32);
    /// The chunk is loaded and at full status.
    fn is_chunk_ready(&self, dimension: &str, chunk_x: i32, chunk_z: i32) -> bool;
    /// State of one section (`sy` is the section y, not the block y).
    fn section_states(
        &self,
        dimension: &str,
        chunk_x: i32,
        section_y: i32,
        chunk_z: i32,
    ) -> SectionRead;
    /// Block entity NBT at a block position. `None` if the chunk is not loaded or there is no block entity.
    fn block_entity_nbt(&mut self, dimension: &str, x: i32, y: i32, z: i32) -> Option<Nbt>;
    /// All block entities of one section as `(x, y, z, nbt)`. Empty if the chunk is not loaded.
    fn section_block_entities(
        &mut self,
        dimension: &str,
        chunk_x: i32,
        section_y: i32,
        chunk_z: i32,
    ) -> Vec<(i32, i32, i32, Nbt)>;
    /// Entity by UUID. `None` if the entity is not in the dimension.
    fn entity_info(&self, dimension: &str, uuid: u128) -> Option<EntityInfo>;
    /// Full save of the entity (`Entity.save`). `None` if the entity cannot be saved.
    fn entity_save_nbt(&mut self, dimension: &str, uuid: u128) -> Option<Nbt>;
    /// Sends one custom payload to a player.
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>);
}

/// Errors for malformed requests. The Java side throws in these cases, so the glue should log and drop.
#[derive(Debug)]
pub enum RequestError {
    /// Payload could not be decoded.
    Buf(BufError),
    /// A list count was above the configured limit.
    CollectionTooLarge { count: usize, limit: usize },
    /// A count on the wire was negative.
    NegativeCount(i32),
}

impl From<BufError> for RequestError {
    fn from(err: BufError) -> Self {
        RequestError::Buf(err)
    }
}

fn can_use(host: &impl RequestHost, player: u128, node: &str) -> bool {
    host.is_axiom_active(player) && host.has_permission(player, node)
}

fn read_count(reader: &mut Reader) -> Result<usize, RequestError> {
    let count = reader.read_varint()?;
    usize::try_from(count).map_err(|_| RequestError::NegativeCount(count))
}

/// Skips `count` packed positions (8 bytes each), as Java's `skipBytes`.
fn skip_longs(reader: &mut Reader, count: usize) -> Result<(), RequestError> {
    reader.read_bytes(count * 8)?;
    Ok(())
}

fn send_chunk_packets(host: &mut impl RequestHost, player: u128, packets: Vec<Vec<u8>>) {
    for packet in packets {
        host.send_payload(player, RESPONSE_CHUNK_CHANNEL, packet);
    }
}

/// Empty chunk response: id, no block entities, no sections, finished.
fn send_empty_chunk_response(host: &mut impl RequestHost, player: u128, id: i64) {
    let packets = codec::chunk_response_packets(id, &BTreeMap::new(), &BTreeMap::new(), 0);
    send_chunk_packets(host, player, packets);
}

/// Handles one `axiom:request_chunk_data` payload (the bytes after the channel).
///
/// Returns `Some(operation)` when some chunks must load first. The glue pushes it onto the queue of the
/// player's dimension and calls [`RequestChunksOperation::tick`] each server tick until it is finished.
/// Returns `None` when the response was already sent.
pub fn handle_chunk_request(
    host: &mut impl RequestHost,
    player: u128,
    payload: &[u8],
) -> Result<Option<RequestChunksOperation>, RequestError> {
    let mut reader = Reader::new(payload);
    let id = reader.read_i64()?;

    if !can_use(host, player, PERMISSION_CHUNK_REQUEST) || host.is_mismatched_data_version(player) {
        // Always answer, so the client does not wait forever.
        send_empty_chunk_response(host, player, id);
        return Ok(None);
    }
    let Some(dimension) = host.player_dimension(player) else {
        return Ok(None);
    };
    if !host.can_modify_world(player, &dimension) {
        send_empty_chunk_response(host, player, id);
        return Ok(None);
    }

    // Only the dimension the player is in can be requested.
    let requested_dimension = reader.read_string(MAX_IDENTIFIER_LEN)?;
    if requested_dimension != dimension {
        send_empty_chunk_response(host, player, id);
        return Ok(None);
    }

    let allow_block_entities = host.has_permission(player, PERMISSION_CHUNK_REQUEST_BLOCK_ENTITY);
    let send_block_entities_in_chunks = reader.read_bool()? && allow_block_entities;
    let max_distance = host.max_chunk_load_distance(&dimension);

    if !allow_block_entities && max_distance <= 0 {
        send_empty_chunk_response(host, player, id);
        return Ok(None);
    }

    let (player_x, _, player_z) = host.player_block_pos(player);
    let player_section_x = player_x >> 4;
    let player_section_z = player_z >> 4;

    let mut sending_block_entities: BTreeMap<i64, CompressedBlockEntity> = BTreeMap::new();
    let mut sending_sections: BTreeMap<i64, Option<Box<BlockStates>>> = BTreeMap::new();
    let mut pending_chunks: BTreeSet<i64> = BTreeSet::new();
    let mut block_entities_pending: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut sections_pending: BTreeMap<i64, Vec<i32>> = BTreeMap::new();

    let block_entity_count = read_count(&mut reader)?;
    if !allow_block_entities {
        skip_longs(&mut reader, block_entity_count)?;
    } else {
        let (min_y, height) = host.build_height(&dimension);
        for _ in 0..block_entity_count {
            let packed = reader.read_i64()?;
            let (x, y, z) = unpack_block_pos(packed);
            if y < min_y || y >= min_y + height {
                continue;
            }
            let chunk_x = x >> 4;
            let chunk_z = z >> 4;
            let distance = (player_section_x - chunk_x).abs() + (player_section_z - chunk_z).abs();
            let can_load = distance < max_distance;

            if !can_load {
                if let Some(nbt) = host.block_entity_nbt(&dimension, x, y, z) {
                    if let Some(compressed) = codec::compress_block_entity(&nbt) {
                        sending_block_entities.insert(packed, compressed);
                    }
                }
            } else {
                let chunk_key = pack_chunk_pos(chunk_x, chunk_z);
                block_entities_pending
                    .entry(chunk_key)
                    .or_default()
                    .push(packed);
                pending_chunks.insert(chunk_key);
            }
        }
    }

    let chunk_count = read_count(&mut reader)?;
    if max_distance <= 0 {
        skip_longs(&mut reader, chunk_count)?;
    } else {
        for _ in 0..chunk_count {
            let packed = reader.read_i64()?;
            let (section_x, section_y, section_z) = unpack_block_pos(packed);
            let distance =
                (player_section_x - section_x).abs() + (player_section_z - section_z).abs();
            let can_load = distance < max_distance;

            if !can_load {
                match host.section_states(&dimension, section_x, section_y, section_z) {
                    SectionRead::Missing => {}
                    SectionRead::Empty => {
                        sending_sections.insert(packed, None);
                    }
                    SectionRead::Blocks(states) => {
                        sending_sections.insert(packed, Some(states));
                        if send_block_entities_in_chunks {
                            for (x, y, z, nbt) in host
                                .section_block_entities(&dimension, section_x, section_y, section_z)
                            {
                                if let Some(compressed) = codec::compress_block_entity(&nbt) {
                                    sending_block_entities
                                        .insert(pack_block_pos(x, y, z), compressed);
                                }
                            }
                        }
                    }
                }
            } else {
                let chunk_key = pack_chunk_pos(section_x, section_z);
                sections_pending
                    .entry(chunk_key)
                    .or_default()
                    .push(section_y);
                pending_chunks.insert(chunk_key);
            }
        }
    }

    if pending_chunks.is_empty() {
        let global_bits = host.block_state_global_bits();
        let packets = codec::chunk_response_packets(
            id,
            &sending_block_entities,
            &sending_sections,
            global_bits,
        );
        send_chunk_packets(host, player, packets);
        return Ok(None);
    }

    Ok(Some(RequestChunksOperation {
        player,
        id,
        dimension,
        to_submit: pending_chunks.into_iter().collect(),
        in_flight: VecDeque::new(),
        block_entities_pending,
        sections_pending,
        send_block_entities_in_chunks,
        sending_block_entities,
        sending_sections,
        finished: false,
    }))
}

/// A chunk request waiting for its chunks to load (Java `RequestChunksOperation`).
pub struct RequestChunksOperation {
    player: u128,
    id: i64,
    dimension: String,
    /// Chunks not yet requested from the host, in ascending packed order.
    to_submit: VecDeque<i64>,
    /// Chunks requested, in submission order. Only a ready prefix is processed per tick.
    in_flight: VecDeque<i64>,
    block_entities_pending: BTreeMap<i64, Vec<i64>>,
    sections_pending: BTreeMap<i64, Vec<i32>>,
    send_block_entities_in_chunks: bool,
    sending_block_entities: BTreeMap<i64, CompressedBlockEntity>,
    sending_sections: BTreeMap<i64, Option<Box<BlockStates>>>,
    finished: bool,
}

impl RequestChunksOperation {
    /// Player the reply goes to.
    pub fn executor(&self) -> u128 {
        self.player
    }

    /// Dimension whose queue should hold this operation.
    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    /// True once the reply was sent or the player left.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// One server tick. Submits up to [`MAX_CHUNK_FUTURES`] loads, processes the ready prefix, and sends the
    /// reply when nothing is left. Stops early if the player has gone.
    pub fn tick(&mut self, host: &mut impl RequestHost) {
        if self.finished {
            return;
        }
        if !host.is_player_online(self.player) {
            for chunk_key in self.in_flight.drain(..) {
                let (chunk_x, chunk_z) = unpack_chunk_pos(chunk_key);
                host.release_chunk(&self.dimension, chunk_x, chunk_z);
            }
            self.finished = true;
            return;
        }

        while self.in_flight.len() < MAX_CHUNK_FUTURES {
            let Some(chunk_key) = self.to_submit.pop_front() else {
                break;
            };
            let (chunk_x, chunk_z) = unpack_chunk_pos(chunk_key);
            host.request_chunk(&self.dimension, chunk_x, chunk_z);
            self.in_flight.push_back(chunk_key);
        }

        // Like the Java loop: stop at the first chunk that is not ready yet.
        while let Some(&chunk_key) = self.in_flight.front() {
            let (chunk_x, chunk_z) = unpack_chunk_pos(chunk_key);
            if !host.is_chunk_ready(&self.dimension, chunk_x, chunk_z) {
                return;
            }
            self.in_flight.pop_front();
            self.process_chunk(host, chunk_key, chunk_x, chunk_z);
            host.release_chunk(&self.dimension, chunk_x, chunk_z);
        }

        if !self.to_submit.is_empty() {
            return;
        }

        let global_bits = host.block_state_global_bits();
        let packets = codec::chunk_response_packets(
            self.id,
            &self.sending_block_entities,
            &self.sending_sections,
            global_bits,
        );
        send_chunk_packets(host, self.player, packets);
        self.finished = true;
    }

    fn process_chunk(
        &mut self,
        host: &mut impl RequestHost,
        chunk_key: i64,
        chunk_x: i32,
        chunk_z: i32,
    ) {
        if let Some(positions) = self.block_entities_pending.remove(&chunk_key) {
            for packed in positions {
                let (x, y, z) = unpack_block_pos(packed);
                if let Some(nbt) = host.block_entity_nbt(&self.dimension, x, y, z) {
                    if let Some(compressed) = codec::compress_block_entity(&nbt) {
                        self.sending_block_entities.insert(packed, compressed);
                    }
                }
            }
        }

        if let Some(section_ys) = self.sections_pending.remove(&chunk_key) {
            let mut has_non_air_section = false;
            for &section_y in &section_ys {
                match host.section_states(&self.dimension, chunk_x, section_y, chunk_z) {
                    SectionRead::Missing => {}
                    SectionRead::Empty => {
                        self.sending_sections
                            .insert(pack_block_pos(chunk_x, section_y, chunk_z), None);
                    }
                    SectionRead::Blocks(states) => {
                        self.sending_sections
                            .insert(pack_block_pos(chunk_x, section_y, chunk_z), Some(states));
                        has_non_air_section = true;
                    }
                }
            }

            if self.send_block_entities_in_chunks && has_non_air_section {
                for &section_y in &section_ys {
                    for (x, y, z, nbt) in
                        host.section_block_entities(&self.dimension, chunk_x, section_y, chunk_z)
                    {
                        if let Some(compressed) = codec::compress_block_entity(&nbt) {
                            self.sending_block_entities
                                .insert(pack_block_pos(x, y, z), compressed);
                        }
                    }
                }
            }
        }
    }
}

/// Handles one `axiom:request_entity_data` payload. Always answers, even on refusal.
///
/// Entities that are players, not manipulable, not placeable for the player, or unsaveable are skipped.
/// The reply is split into several packets when the NBT budget (1 MiB per packet) runs out.
pub fn handle_entity_request(
    host: &mut impl RequestHost,
    player: u128,
    payload: &[u8],
) -> Result<(), RequestError> {
    let mut reader = Reader::new(payload);
    let id = reader.read_i64()?;

    if !can_use(host, player, PERMISSION_ENTITY_REQUEST_DATA)
        || host.is_mismatched_data_version(player)
    {
        send_entity_response(host, player, id, true, &[]);
        return Ok(());
    }
    let Some(dimension) = host.player_dimension(player) else {
        return Ok(());
    };
    if !host.can_modify_world(player, &dimension) {
        send_entity_response(host, player, id, true, &[]);
        return Ok(());
    }

    let limit = host.packet_collection_read_limit();
    let count = read_count(&mut reader)?;
    if count > limit {
        return Err(RequestError::CollectionTooLarge { count, limit });
    }
    let mut requested = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        requested.push(reader.read_uuid()?);
    }

    let max_packet = MAX_PACKET_SIZE as i64;
    let mut remaining = max_packet;
    let mut collected: Vec<(u128, Nbt)> = Vec::new();
    let mut visited: HashSet<u128> = HashSet::new();

    for uuid in requested {
        if !visited.insert(uuid) {
            continue;
        }
        let Some(info) = host.entity_info(&dimension, uuid) else {
            continue;
        };
        if info.entity_type == PLAYER_ENTITY_TYPE
            || !host.can_entity_be_manipulated(&info.entity_type)
        {
            continue;
        }
        let (x, y, z) = info.block_pos;
        if !host.can_place_block(player, &dimension, x, y, z) {
            continue;
        }
        let Some(nbt) = host.entity_save_nbt(&dimension, uuid) else {
            continue;
        };

        // Budget: uuid (16 bytes) plus the wire size of the NBT.
        let Some(nbt_len) = codec::network_nbt_len(&nbt) else {
            continue;
        };
        let size = 16 + nbt_len as i64;
        if size >= max_packet {
            send_entity_response(host, player, id, false, &[(uuid, nbt)]);
            continue;
        }
        if remaining - size < 0 {
            send_entity_response(host, player, id, false, &collected);
            collected.clear();
            remaining = max_packet;
        }
        collected.push((uuid, nbt));
        remaining -= size;
    }

    send_entity_response(host, player, id, true, &collected);
    Ok(())
}

fn send_entity_response(
    host: &mut impl RequestHost,
    player: u128,
    id: i64,
    finished: bool,
    entries: &[(u128, Nbt)],
) {
    let data = codec::entity_response_bytes(id, finished, entries);
    host.send_payload(player, RESPONSE_ENTITY_CHANNEL, data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::Writer;
    use std::collections::{HashMap, HashSet};

    enum FakeSection {
        Empty,
        Blocks(BlockStates),
    }

    #[derive(Default)]
    struct FakeHost {
        active: bool,
        perms: HashSet<&'static str>,
        mismatched: bool,
        online: bool,
        dimension: String,
        player_pos: (i32, i32, i32),
        max_distance: i32,
        global_bits: u8,
        build_height: (i32, i32),
        manipulable: HashSet<String>,
        blocked: HashSet<u128>,
        read_limit: usize,
        /// Raw network NBT per block entity position.
        block_entities: HashMap<(i32, i32, i32), Vec<u8>>,
        /// Sections by (chunk_x, section_y, chunk_z).
        sections: HashMap<(i32, i32, i32), FakeSection>,
        /// Chunks that are loaded.
        ready: HashSet<(i32, i32)>,
        requested: Vec<(i32, i32)>,
        /// Entities by uuid: (type, block pos, raw network NBT).
        entities: HashMap<u128, (String, (i32, i32, i32), Vec<u8>)>,
        sent: Vec<(u128, String, Vec<u8>)>,
    }

    impl RequestHost for FakeHost {
        fn is_axiom_active(&self, _: u128) -> bool {
            self.active
        }
        fn has_permission(&self, _: u128, node: &str) -> bool {
            self.perms.contains(node)
        }
        fn is_mismatched_data_version(&self, _: u128) -> bool {
            self.mismatched
        }
        fn is_player_online(&self, _: u128) -> bool {
            self.online
        }
        fn player_dimension(&self, _: u128) -> Option<String> {
            Some(self.dimension.clone())
        }
        fn player_block_pos(&self, _: u128) -> (i32, i32, i32) {
            self.player_pos
        }
        fn can_modify_world(&self, _: u128, _: &str) -> bool {
            true
        }
        fn can_place_block(&self, player: u128, _: &str, _: i32, _: i32, _: i32) -> bool {
            !self.blocked.contains(&player)
        }
        fn max_chunk_load_distance(&self, _: &str) -> i32 {
            self.max_distance
        }
        fn packet_collection_read_limit(&self) -> usize {
            self.read_limit
        }
        fn can_entity_be_manipulated(&self, entity_type: &str) -> bool {
            self.manipulable.contains(entity_type)
        }
        fn build_height(&self, _: &str) -> (i32, i32) {
            self.build_height
        }
        fn block_state_global_bits(&self) -> u8 {
            self.global_bits
        }
        fn request_chunk(&mut self, _: &str, cx: i32, cz: i32) {
            self.requested.push((cx, cz));
        }
        fn release_chunk(&mut self, _: &str, _cx: i32, _cz: i32) {}
        fn is_chunk_ready(&self, _: &str, cx: i32, cz: i32) -> bool {
            self.ready.contains(&(cx, cz))
        }
        fn section_states(&self, _: &str, cx: i32, sy: i32, cz: i32) -> SectionRead {
            match self.sections.get(&(cx, sy, cz)) {
                None => SectionRead::Missing,
                Some(FakeSection::Empty) => SectionRead::Empty,
                Some(FakeSection::Blocks(states)) => SectionRead::Blocks(Box::new(*states)),
            }
        }
        fn block_entity_nbt(&mut self, _: &str, x: i32, y: i32, z: i32) -> Option<Nbt> {
            self.block_entities.get(&(x, y, z)).map(|b| nbt_from(b))
        }
        fn section_block_entities(
            &mut self,
            _: &str,
            cx: i32,
            sy: i32,
            cz: i32,
        ) -> Vec<(i32, i32, i32, Nbt)> {
            self.block_entities
                .iter()
                .filter(|((x, y, z), _)| x >> 4 == cx && y >> 4 == sy && z >> 4 == cz)
                .map(|((x, y, z), b)| (*x, *y, *z, nbt_from(b)))
                .collect()
        }
        fn entity_info(&self, _: &str, uuid: u128) -> Option<EntityInfo> {
            self.entities.get(&uuid).map(|(t, p, _)| EntityInfo {
                entity_type: t.clone(),
                block_pos: *p,
            })
        }
        fn entity_save_nbt(&mut self, _: &str, uuid: u128) -> Option<Nbt> {
            self.entities.get(&uuid).map(|(_, _, b)| nbt_from(b))
        }
        fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
            self.sent.push((player, channel.to_string(), data));
        }
    }

    fn base_host() -> FakeHost {
        let mut host = FakeHost {
            active: true,
            online: true,
            dimension: "minecraft:overworld".into(),
            player_pos: (0, 64, 0),
            global_bits: 15,
            build_height: (-64, 384),
            read_limit: 16,
            ..Default::default()
        };
        host.perms.insert(PERMISSION_CHUNK_REQUEST);
        host.perms.insert(PERMISSION_CHUNK_REQUEST_BLOCK_ENTITY);
        host.perms.insert(PERMISSION_ENTITY_REQUEST_DATA);
        host.manipulable.insert("minecraft:armor_stand".into());
        host.manipulable.insert("minecraft:player".into());
        host
    }

    /// Network NBT for a compound with one string entry.
    fn compound_string(name: &str, value: &str) -> Vec<u8> {
        let mut b = vec![0x0A, 0x08];
        b.extend_from_slice(&(name.len() as u16).to_be_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&(value.len() as u16).to_be_bytes());
        b.extend_from_slice(value.as_bytes());
        b.push(0x00);
        b
    }

    /// Network NBT for a compound with one byte array entry of `n` bytes.
    fn compound_bytes(name: &str, n: usize) -> Vec<u8> {
        let mut b = vec![0x0A, 0x07];
        b.extend_from_slice(&(name.len() as u16).to_be_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&(n as i32).to_be_bytes());
        b.extend(std::iter::repeat(0x5A).take(n));
        b.push(0x00);
        b
    }

    fn nbt_from(bytes: &[u8]) -> Nbt {
        let mut r = Reader::new(bytes);
        r.read_nbt().expect("valid test NBT")
    }

    fn chunk_payload(id: i64, dim: &str, send_be: bool, be: &[i64], sections: &[i64]) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_i64(id);
        w.write_string(dim);
        w.write_bool(send_be);
        w.write_varint(be.len() as i32);
        for &p in be {
            w.write_i64(p);
        }
        w.write_varint(sections.len() as i32);
        for &p in sections {
            w.write_i64(p);
        }
        w.into_bytes()
    }

    fn empty_reply(id: i64) -> Vec<u8> {
        let mut v = id.to_be_bytes().to_vec();
        v.extend_from_slice(&MIN_POSITION_LONG.to_be_bytes());
        v.extend_from_slice(&MIN_POSITION_LONG.to_be_bytes());
        v.push(1);
        v
    }

    #[test]
    fn chunk_request_without_permission_gets_empty_reply() {
        let mut host = base_host();
        host.perms.clear();
        let payload = chunk_payload(77, "minecraft:overworld", true, &[], &[]);
        assert!(handle_chunk_request(&mut host, 1, &payload)
            .unwrap()
            .is_none());
        assert_eq!(host.sent.len(), 1);
        assert_eq!(host.sent[0].1, RESPONSE_CHUNK_CHANNEL);
        assert_eq!(host.sent[0].2, empty_reply(77));
    }

    #[test]
    fn chunk_request_for_other_dimension_gets_empty_reply() {
        let mut host = base_host();
        let payload = chunk_payload(5, "minecraft:the_nether", true, &[], &[]);
        assert!(handle_chunk_request(&mut host, 1, &payload)
            .unwrap()
            .is_none());
        assert_eq!(host.sent[0].2, empty_reply(5));
    }

    #[test]
    fn chunk_request_with_no_loading_and_no_be_permission_is_empty() {
        let mut host = base_host();
        host.perms.remove(PERMISSION_CHUNK_REQUEST_BLOCK_ENTITY);
        host.max_distance = 0;
        let payload = chunk_payload(6, "minecraft:overworld", true, &[], &[]);
        assert!(handle_chunk_request(&mut host, 1, &payload)
            .unwrap()
            .is_none());
        assert_eq!(host.sent[0].2, empty_reply(6));
        assert!(host.requested.is_empty());
    }

    #[test]
    fn far_block_entity_is_answered_at_once() {
        let mut host = base_host();
        host.max_distance = 0; // everything is "far"
        let pos = pack_block_pos(33, 70, -5);
        host.block_entities
            .insert((33, 70, -5), compound_string("id", "minecraft:chest"));
        let payload = chunk_payload(9, "minecraft:overworld", true, &[pos], &[]);
        assert!(handle_chunk_request(&mut host, 1, &payload)
            .unwrap()
            .is_none());
        assert!(host.requested.is_empty());
        assert_eq!(host.sent.len(), 1);
        let data = &host.sent[0].2;
        assert_eq!(&data[8..16], &pos.to_be_bytes());
        // The reply ends with: section terminator, finished = true.
        assert_eq!(
            &data[data.len() - 9..data.len() - 1],
            &MIN_POSITION_LONG.to_be_bytes()
        );
        assert_eq!(data[data.len() - 1], 1);
    }

    #[test]
    fn near_section_waits_for_chunk_load_then_sends() {
        let mut host = base_host();
        host.max_distance = 3;
        // Section at chunk (1, 0) is at distance 1 from the player's chunk (0, 0): needs a load.
        host.sections
            .insert((1, 4, 0), FakeSection::Blocks([9u32; SECTION_VOLUME]));
        let key = pack_block_pos(1, 4, 0);
        let payload = chunk_payload(10, "minecraft:overworld", false, &[], &[key]);
        let mut op = handle_chunk_request(&mut host, 1, &payload)
            .unwrap()
            .expect("pending op");
        assert_eq!(op.dimension(), "minecraft:overworld");

        op.tick(&mut host);
        assert_eq!(host.requested, vec![(1, 0)]);
        assert!(host.sent.is_empty());
        assert!(!op.is_finished());

        host.ready.insert((1, 0));
        op.tick(&mut host);
        assert!(op.is_finished());
        assert_eq!(host.sent.len(), 1);
        let data = &host.sent[0].2;
        // id, block entity terminator, section key, present flag.
        assert_eq!(&data[8..16], &MIN_POSITION_LONG.to_be_bytes());
        assert_eq!(&data[16..24], &key.to_be_bytes());
        assert_eq!(data[24], 1);
    }

    #[test]
    fn pending_operation_stops_when_player_leaves() {
        let mut host = base_host();
        host.max_distance = 3;
        let key = pack_block_pos(0, 4, 0);
        let payload = chunk_payload(11, "minecraft:overworld", false, &[], &[key]);
        let mut op = handle_chunk_request(&mut host, 1, &payload)
            .unwrap()
            .unwrap();
        host.online = false;
        op.tick(&mut host);
        assert!(op.is_finished());
        assert!(host.sent.is_empty());
    }

    #[test]
    fn empty_section_in_pending_chunk_is_sent_as_absent() {
        let mut host = base_host();
        host.max_distance = 3;
        host.sections.insert((0, 2, 0), FakeSection::Empty);
        let key = pack_block_pos(0, 2, 0);
        let payload = chunk_payload(12, "minecraft:overworld", false, &[], &[key]);
        let mut op = handle_chunk_request(&mut host, 1, &payload)
            .unwrap()
            .unwrap();
        host.ready.insert((0, 0));
        op.tick(&mut host);
        assert!(op.is_finished());
        assert_eq!(host.sent[0].2[24], 0);
    }

    #[test]
    fn entity_request_sends_saveable_entities_once_and_skips_players() {
        let mut host = base_host();
        host.entities.insert(
            1001,
            (
                "minecraft:armor_stand".into(),
                (1, 2, 3),
                compound_string("id", "a"),
            ),
        );
        host.entities.insert(
            1002,
            (
                "minecraft:player".into(),
                (0, 0, 0),
                compound_string("id", "p"),
            ),
        );
        host.entities.insert(
            1003,
            (
                "minecraft:pig".into(),
                (0, 0, 0),
                compound_string("id", "pig"),
            ),
        );
        let mut w = Writer::new();
        w.write_i64(44);
        w.write_varint(5);
        for uuid in [1001u128, 1001, 1002, 1003, 9999] {
            w.write_uuid(uuid);
        }
        handle_entity_request(&mut host, 1, &w.into_bytes()).unwrap();
        assert_eq!(host.sent.len(), 1);
        let data = &host.sent[0].2;
        assert_eq!(host.sent[0].1, RESPONSE_ENTITY_CHANNEL);
        // id, finished = true, count = 1 (only the armor stand; pig is not manipulable).
        assert_eq!(&data[..8], &44i64.to_be_bytes());
        assert_eq!(data[8], 1);
        assert_eq!(data[9], 1);
    }

    #[test]
    fn entity_request_over_limit_is_an_error() {
        let mut host = base_host();
        host.read_limit = 2;
        let mut w = Writer::new();
        w.write_i64(1);
        w.write_varint(3);
        let err = handle_entity_request(&mut host, 1, &w.into_bytes()).unwrap_err();
        assert!(matches!(
            err,
            RequestError::CollectionTooLarge { count: 3, limit: 2 }
        ));
    }

    #[test]
    fn entity_request_splits_when_budget_is_exhausted() {
        let mut host = base_host();
        let big = compound_bytes("data", 600_000);
        host.entities
            .insert(1, ("minecraft:armor_stand".into(), (0, 0, 0), big.clone()));
        host.entities
            .insert(2, ("minecraft:armor_stand".into(), (0, 0, 0), big));
        let mut w = Writer::new();
        w.write_i64(3);
        w.write_varint(2);
        w.write_uuid(1);
        w.write_uuid(2);
        handle_entity_request(&mut host, 1, &w.into_bytes()).unwrap();
        // Two packets: the first is not finished and holds one entity, the second is finished.
        assert_eq!(host.sent.len(), 2);
        assert_eq!(host.sent[0].2[8], 0);
        assert_eq!(host.sent[1].2[8], 1);
        assert_eq!(host.sent[0].2[9], 1);
        assert_eq!(host.sent[1].2[9], 1);
    }

    #[test]
    fn entity_request_without_permission_sends_empty_finished_reply() {
        let mut host = base_host();
        host.perms.clear();
        let mut w = Writer::new();
        w.write_i64(8);
        w.write_varint(0);
        handle_entity_request(&mut host, 1, &w.into_bytes()).unwrap();
        assert_eq!(
            host.sent[0].2,
            [8i64.to_be_bytes().as_slice(), &[1, 0]].concat()
        );
    }

    #[test]
    fn named_nbt_adds_empty_root_name() {
        let net = compound_string("a", "b");
        let named = codec::named_nbt_bytes(&nbt_from(&net)).unwrap();
        assert_eq!(&named[..3], &[0x0A, 0, 0]);
        assert_eq!(named.len(), net.len() + 2);
    }

    #[test]
    fn block_entity_compression_records_original_size() {
        let nbt = nbt_from(&compound_string("id", "minecraft:sign"));
        let c = codec::compress_block_entity(&nbt).unwrap();
        assert_eq!(
            c.original_size as usize,
            codec::named_nbt_bytes(&nbt).unwrap().len()
        );
        assert_eq!(c.compression_dict, 0);
        assert_eq!(&c.compressed[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
    }
}
