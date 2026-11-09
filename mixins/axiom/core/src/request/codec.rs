// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/RequestChunkDataPacketListener.java
// (sendResponse), buffer/CompressedBlockEntity.java, and the vanilla PalettedContainer.write layout used for section data.
//
// Wire codecs only: no host access in here. Everything is a pure function over values so it can be unit tested.

use std::collections::{BTreeMap, HashMap};

use crate::buf::{Nbt, Writer};
use crate::buffer::zstd_compress;

/// Maximum custom payload size used by the Java sender (0x100000).
pub const MAX_PACKET_SIZE: usize = 0x10_0000;
/// Leeway subtracted from the packet size before a split is forced (Java: `maxSize = 0x100000 - 64`).
pub const RESPONSE_LEEWAY: usize = 64;
/// Number of block states in one 16x16x16 section.
pub const SECTION_VOLUME: usize = 4096;
/// Block-state palette strategy: linear and hash-map palettes use at least this many bits (vanilla).
const MIN_PALETTE_BITS: u32 = 4;
/// Largest bit count that still uses an indirect (palette) layout; above it the global id map is used.
const MAX_PALETTE_BITS: u32 = 8;

/// Global block state ids of one section, indexed `(y << 8) | (z << 4) | x` (vanilla order).
pub type BlockStates = [u32; SECTION_VOLUME];

/// Java `BlockPos.asLong(x, y, z)`.
pub const fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

/// Java `BlockPos.getX/getY/getZ` on a packed long.
pub const fn unpack_block_pos(packed: i64) -> (i32, i32, i32) {
    let x = (packed >> 38) as i32;
    let y = ((packed << 52) >> 52) as i32;
    let z = ((packed << 26) >> 38) as i32;
    (x, y, z)
}

/// Java `ChunkPos.pack(x, z)`.
pub const fn pack_chunk_pos(x: i32, z: i32) -> i64 {
    ((x as u32) as i64) | (((z as u32) as i64) << 32)
}

/// Java `ChunkPos.getX/getZ` on a packed long.
pub const fn unpack_chunk_pos(packed: i64) -> (i32, i32) {
    let x = (packed as u32) as i32;
    let z = (((packed as u64) >> 32) as u32) as i32;
    (x, z)
}

/// Position of the end-of-list marker written by Java as `MIN_POSITION_LONG`.
pub const MIN_POSITION_LONG: i64 = pack_block_pos(-33_554_432, -2048, -33_554_432);

/// A block entity (or section-level NBT) after compression, mirror of Java `CompressedBlockEntity`.
#[derive(Debug)]
pub struct CompressedBlockEntity {
    /// Length of the uncompressed named NBT.
    pub original_size: i32,
    /// Dictionary selector. 0 = zstd with `block_entities_v1.dict` (see [`crate::buffer::zstd_compress`]).
    pub compression_dict: u8,
    /// The zstd frame.
    pub compressed: Vec<u8>,
}

/// Network NBT (unnamed root, as `FriendlyByteBuf.writeNbt`) converted to named NBT (as `NbtIo.write`).
///
/// Named form = root type byte, empty root name (u16 0), then the same payload. Returns `None` when the NBT
/// cannot be encoded or its root is not a compound, which is the only root type Axiom ever sends here.
pub fn named_nbt_bytes(nbt: &Nbt) -> Option<Vec<u8>> {
    let mut w = Writer::new();
    w.write_nbt(nbt).ok()?;
    let network = w.into_bytes();
    if network.first() != Some(&TAG_COMPOUND) {
        return None;
    }
    let mut out = Vec::with_capacity(network.len() + 2);
    out.push(TAG_COMPOUND);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&network[1..]);
    Some(out)
}

const TAG_COMPOUND: u8 = 10;

/// Network NBT length in bytes (used for the entity size budget). `None` when the NBT cannot be encoded.
pub fn network_nbt_len(nbt: &Nbt) -> Option<usize> {
    let mut w = Writer::new();
    w.write_nbt(nbt).ok()?;
    Some(w.into_bytes().len())
}

/// Compress one block entity the way Java does: named NBT, then zstd. Frames are made by ruzstd without a
/// dictionary (see [`crate::buffer::zstd_compress`]), so `compression_dict` stays 0 and Java decodes them.
pub fn compress_block_entity(nbt: &Nbt) -> Option<CompressedBlockEntity> {
    let named = named_nbt_bytes(nbt)?;
    Some(CompressedBlockEntity {
        original_size: named.len() as i32,
        compression_dict: 0,
        compressed: zstd_compress(&named),
    })
}

/// Encodes a section's block states as vanilla `PalettedContainer.write` does for block states.
///
/// Layout: bits byte, then palette (single value: one varint; linear/hash map: count plus varint ids;
/// global: nothing), then the packed storage as a long array (varint length plus big-endian longs).
/// The palette order is first-seen order. The client accepts any valid palette, so this does not have
/// to match the server's own container layout.
pub fn encode_block_states(w: &mut Writer, states: &BlockStates, global_bits: u8) {
    let mut palette: Vec<u32> = Vec::new();
    let mut index_of: HashMap<u32, u32> = HashMap::new();
    for &state in states.iter() {
        if !index_of.contains_key(&state) {
            index_of.insert(state, palette.len() as u32);
            palette.push(state);
        }
    }

    if palette.len() == 1 {
        // Single-valued: zero bits, no storage. The storage is an empty long array.
        w.write_bytes(&[0]);
        w.write_varint(palette[0] as i32);
        w.write_varint(0);
        return;
    }

    let needed = bits_for(palette.len());
    let (bits, values): (u32, Vec<u32>) = if needed <= MIN_PALETTE_BITS {
        write_palette(w, MIN_PALETTE_BITS, &palette);
        (
            MIN_PALETTE_BITS,
            states.iter().map(|s| index_of[s]).collect(),
        )
    } else if needed <= MAX_PALETTE_BITS {
        write_palette(w, needed, &palette);
        (needed, states.iter().map(|s| index_of[s]).collect())
    } else {
        // Global palette: raw state ids, no palette section.
        debug_assert!(global_bits as u32 >= bits_for(palette.len()));
        w.write_bytes(&[global_bits]);
        (global_bits as u32, states.to_vec())
    };

    // Packed storage: values per long = 64 / bits, a value never spans two longs.
    let per_long = (64 / bits) as usize;
    let mut longs = vec![0u64; SECTION_VOLUME.div_ceil(per_long)];
    for (index, value) in values.iter().enumerate() {
        longs[index / per_long] |= (*value as u64) << ((index % per_long) as u32 * bits);
    }
    w.write_varint(longs.len() as i32);
    for long in longs {
        w.write_i64(long as i64);
    }
}

fn write_palette(w: &mut Writer, bits: u32, palette: &[u32]) {
    w.write_bytes(&[bits as u8]);
    w.write_varint(palette.len() as i32);
    for &id in palette {
        w.write_varint(id as i32);
    }
}

/// Smallest bit count that addresses `count` palette entries (count >= 2).
fn bits_for(count: usize) -> u32 {
    64 - ((count - 1) as u64).leading_zeros()
}

/// Bytes of one block entity entry: key (packed position), then the compressed payload.
fn block_entity_entry(key: i64, entity: &CompressedBlockEntity) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_i64(key);
    w.write_varint(entity.original_size);
    w.write_bytes(&[entity.compression_dict]);
    w.write_varint(entity.compressed.len() as i32);
    w.write_bytes(&entity.compressed);
    w.into_bytes()
}

/// Bytes of one section entry: key, then presence flag, then the palette container if present.
fn section_entry(key: i64, states: Option<&BlockStates>, global_bits: u8) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_i64(key);
    match states {
        None => w.write_bool(false),
        Some(states) => {
            w.write_bool(true);
            encode_block_states(&mut w, states, global_bits);
        }
    }
    w.into_bytes()
}

fn header(id: i64) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_i64(id);
    w.into_bytes()
}

fn marker(build: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    build(&mut w);
    w.into_bytes()
}

/// Splits a chunk response into custom payloads, exactly as `RequestChunkDataPacketListener.sendResponse`.
///
/// Packet layout: id, block entities (key + entry) ..., MIN_POSITION, sections (key + flag [+ container]) ...,
/// MIN_POSITION, finished flag. A packet that reaches the size limit is cut. The `first_part` flag and the
/// "copy the overflowing entry into the next packet" rule are kept from Java.
pub fn chunk_response_packets(
    id: i64,
    block_entities: &BTreeMap<i64, CompressedBlockEntity>,
    sections: &BTreeMap<i64, Option<Box<BlockStates>>>,
    global_bits: u8,
) -> Vec<Vec<u8>> {
    let max_size = MAX_PACKET_SIZE - RESPONSE_LEEWAY;
    let mut packets = Vec::new();
    let mut current = header(id);
    let mut first_part = true;

    for (&key, entity) in block_entities {
        let before = current.len();
        current.extend(block_entity_entry(key, entity));
        if current.len() >= max_size {
            if first_part {
                // Finish this packet with both terminators and finished = false, continue empty.
                current.extend(marker(|w| {
                    w.write_i64(MIN_POSITION_LONG);
                    w.write_i64(MIN_POSITION_LONG);
                    w.write_bool(false);
                }));
                packets.push(std::mem::replace(&mut current, header(id)));
            } else {
                // Move the overflowing entry into the next packet.
                let copied = current.split_off(before);
                current.extend(marker(|w| {
                    w.write_i64(MIN_POSITION_LONG);
                    w.write_i64(MIN_POSITION_LONG);
                    w.write_bool(false);
                }));
                packets.push(std::mem::replace(&mut current, header(id)));
                current.extend(copied);
                first_part = true;
            }
        } else {
            first_part = false;
        }
    }

    current.extend(marker(|w| w.write_i64(MIN_POSITION_LONG)));

    for (&key, states) in sections {
        let before = current.len();
        current.extend(section_entry(key, states.as_deref(), global_bits));
        if current.len() >= max_size {
            if first_part {
                current.extend(marker(|w| {
                    w.write_i64(MIN_POSITION_LONG);
                    w.write_bool(false);
                }));
                packets.push(std::mem::replace(&mut current, header(id)));
                current.extend(marker(|w| w.write_i64(MIN_POSITION_LONG)));
            } else {
                let copied = current.split_off(before);
                current.extend(marker(|w| {
                    w.write_i64(MIN_POSITION_LONG);
                    w.write_bool(false);
                }));
                packets.push(std::mem::replace(&mut current, header(id)));
                current.extend(marker(|w| w.write_i64(MIN_POSITION_LONG)));
                current.extend(copied);
                first_part = true;
            }
        } else {
            first_part = false;
        }
    }

    current.extend(marker(|w| {
        w.write_i64(MIN_POSITION_LONG);
        w.write_bool(true);
    }));
    packets.push(current);
    packets
}

/// Entity data response: id, finished flag, count, then (uuid, network NBT) pairs. An entry whose NBT cannot be
/// encoded is left out, so the count always matches the pairs written.
pub fn entity_response_bytes(id: i64, finished: bool, entries: &[(u128, Nbt)]) -> Vec<u8> {
    let encoded: Vec<Vec<u8>> = entries
        .iter()
        .filter_map(|(uuid, nbt)| {
            let mut entry = Writer::new();
            entry.write_uuid(*uuid);
            entry.write_nbt(nbt).ok()?;
            Some(entry.into_bytes())
        })
        .collect();
    let mut w = Writer::new();
    w.write_i64(id);
    w.write_bool(finished);
    w.write_varint(encoded.len() as i32);
    for entry in &encoded {
        w.write_bytes(entry);
    }
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_position_matches_java_literal() {
        assert_eq!(
            MIN_POSITION_LONG,
            0b1000000000000000000000000010000000000000000000000000100000000000u64 as i64
        );
    }

    #[test]
    fn block_pos_pack_roundtrip_including_negatives() {
        for &(x, y, z) in &[
            (0, 0, 0),
            (-1, -1, -1),
            (123, -64, -4567),
            (-30_000_000, 2047, 30_000_000),
            (5, -2048, 7),
        ] {
            assert_eq!(unpack_block_pos(pack_block_pos(x, y, z)), (x, y, z));
        }
    }

    #[test]
    fn chunk_pos_pack_roundtrip() {
        for &(x, z) in &[(0, 0), (-1, 1), (i32::MIN, i32::MAX), (-123_456, 98_765)] {
            assert_eq!(unpack_chunk_pos(pack_chunk_pos(x, z)), (x, z));
        }
    }

    #[test]
    fn compress_block_entity_round_trips_through_zstd() {
        let mut map = crate::buf::NbtCompound::new();
        map.insert("id".to_owned(), Nbt::String("minecraft:chest".into()));
        map.insert("x".to_owned(), Nbt::Int(5));
        let nbt = Nbt::Compound(map);
        let be = compress_block_entity(&nbt).unwrap();
        assert_eq!(be.compression_dict, 0);
        let named = named_nbt_bytes(&nbt).unwrap();
        assert_eq!(be.original_size as usize, named.len());
        assert_eq!(crate::buffer::zstd_decompress(&be.compressed, named.len()).unwrap(), named);
    }

    /// Test-only decoder for the block-state container written above.
    fn decode_block_states(bytes: &[u8], global_bits: u32) -> (usize, BlockStates) {
        let mut pos = 0usize;
        let bits = bytes[pos] as u32;
        pos += 1;
        let (palette, bits): (Option<Vec<u32>>, u32) = if bits == 0 {
            let (v, n) = read_varint(&bytes[pos..]);
            pos += n;
            (Some(vec![v]), 0)
        } else if bits == global_bits {
            (None, bits)
        } else {
            let (count, n) = read_varint(&bytes[pos..]);
            pos += n;
            let mut palette = Vec::new();
            for _ in 0..count {
                let (v, n) = read_varint(&bytes[pos..]);
                pos += n;
                palette.push(v);
            }
            (Some(palette), bits)
        };
        let (len, n) = read_varint(&bytes[pos..]);
        pos += n;
        let mut longs = Vec::new();
        for _ in 0..len {
            longs.push(u64::from_be_bytes(bytes[pos..pos + 8].try_into().unwrap()));
            pos += 8;
        }
        let mut out = [0u32; SECTION_VOLUME];
        if bits == 0 {
            out = [palette.as_ref().unwrap()[0]; SECTION_VOLUME];
        } else {
            let per_long = (64 / bits) as usize;
            let mask = (1u64 << bits) - 1;
            for (i, slot) in out.iter_mut().enumerate() {
                let raw = ((longs[i / per_long] >> ((i % per_long) as u32 * bits)) & mask) as u32;
                *slot = match &palette {
                    Some(p) => p[raw as usize],
                    None => raw,
                };
            }
        }
        (pos, out)
    }

    fn read_varint(bytes: &[u8]) -> (u32, usize) {
        let mut value = 0u32;
        for (i, b) in bytes.iter().enumerate() {
            value |= ((b & 0x7F) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return (value, i + 1);
            }
        }
        panic!("unterminated varint");
    }

    fn states_from(f: impl Fn(usize) -> u32) -> BlockStates {
        let mut s = [0u32; SECTION_VOLUME];
        for (i, slot) in s.iter_mut().enumerate() {
            *slot = f(i);
        }
        s
    }

    #[test]
    fn block_states_single_value() {
        let mut w = Writer::new();
        encode_block_states(&mut w, &[42u32; SECTION_VOLUME], 15);
        let bytes = w.into_bytes();
        assert_eq!(bytes, vec![0, 42, 0]);
    }

    #[test]
    fn block_states_linear_and_hashmap_and_global_roundtrip() {
        // 2 values -> linear 4 bits.
        let two = states_from(|i| if i % 3 == 0 { 5 } else { 9 });
        // 17 values -> 5 bits hash map.
        let seventeen = states_from(|i| 100 + (i % 17) as u32);
        // 300 values -> global 15 bits.
        let global = states_from(|i| 1000 + (i % 300) as u32);
        for (states, expect_bits) in [(two, 4u8), (seventeen, 5), (global, 15)] {
            let mut w = Writer::new();
            encode_block_states(&mut w, &states, 15);
            let bytes = w.into_bytes();
            assert_eq!(bytes[0], expect_bits);
            let (used, decoded) = decode_block_states(&bytes, 15);
            assert_eq!(used, bytes.len());
            assert_eq!(decoded[..], states[..]);
        }
    }

    #[test]
    fn empty_chunk_response_is_id_markers_and_finished() {
        let packets = chunk_response_packets(7, &BTreeMap::new(), &BTreeMap::new(), 15);
        assert_eq!(packets.len(), 1);
        let mut expect = 7i64.to_be_bytes().to_vec();
        expect.extend_from_slice(&MIN_POSITION_LONG.to_be_bytes());
        expect.extend_from_slice(&MIN_POSITION_LONG.to_be_bytes());
        expect.push(1);
        assert_eq!(packets[0], expect);
    }

    #[test]
    fn section_entries_are_written_in_order_with_empty_marker() {
        let mut sections: BTreeMap<i64, Option<Box<BlockStates>>> = BTreeMap::new();
        sections.insert(pack_block_pos(1, 2, 3), None);
        let packets = chunk_response_packets(9, &BTreeMap::new(), &sections, 15);
        let p = &packets[0];
        // id(8) + BE terminator(8) + key(8) + flag(1) + section terminator(8) + finished(1)
        assert_eq!(p.len(), 8 + 8 + 8 + 1 + 8 + 1);
        assert_eq!(&p[16..24], &pack_block_pos(1, 2, 3).to_be_bytes());
        assert_eq!(p[24], 0);
        assert_eq!(*p.last().unwrap(), 1);
    }

    /// Splits a response into packets and reads back all entries, checking the terminators.
    fn read_back(
        packets: &[Vec<u8>],
        global_bits: u32,
    ) -> (Vec<(i64, i32, Vec<u8>)>, Vec<(i64, Option<BlockStates>)>) {
        let mut be = Vec::new();
        let mut sec = Vec::new();
        for p in packets {
            let mut pos = 8usize; // id
            loop {
                let key = i64::from_be_bytes(p[pos..pos + 8].try_into().unwrap());
                pos += 8;
                if key == MIN_POSITION_LONG {
                    break;
                }
                let (size, n) = read_varint(&p[pos..]);
                pos += n + 1; // dict byte
                let (len, n) = read_varint(&p[pos..]);
                pos += n;
                be.push((key, size as i32, p[pos..pos + len as usize].to_vec()));
                pos += len as usize;
            }
            loop {
                let key = i64::from_be_bytes(p[pos..pos + 8].try_into().unwrap());
                pos += 8;
                if key == MIN_POSITION_LONG {
                    pos += 1; // finished flag
                    assert_eq!(pos, p.len(), "packet has trailing bytes");
                    break;
                }
                let present = p[pos] == 1;
                pos += 1;
                if present {
                    let (used, states) = decode_block_states(&p[pos..], global_bits);
                    pos += used;
                    sec.push((key, Some(states)));
                } else {
                    sec.push((key, None));
                }
            }
        }
        (be, sec)
    }

    #[test]
    fn large_block_entities_split_and_keep_every_entry() {
        let mut be = BTreeMap::new();
        for i in 0..40i64 {
            be.insert(
                i,
                CompressedBlockEntity {
                    original_size: 300_000,
                    compression_dict: 0,
                    compressed: vec![i as u8; 300_000],
                },
            );
        }
        let packets = chunk_response_packets(3, &be, &BTreeMap::new(), 15);
        assert!(packets.len() > 1);
        assert!(packets.iter().all(|p| p.len() <= MAX_PACKET_SIZE));
        let (back, _) = read_back(&packets, 15);
        assert_eq!(back.len(), 40);
        for (i, (key, size, data)) in back.iter().enumerate() {
            assert_eq!(*key, i as i64);
            assert_eq!(*size, 300_000);
            assert_eq!(data.len(), 300_000);
            assert!(data.iter().all(|&b| b == i as u8));
        }
        // The packets after the first each start with the id again.
        for p in &packets {
            assert_eq!(&p[..8], &3i64.to_be_bytes());
        }
    }

    #[test]
    fn sections_split_and_keep_every_entry() {
        let mut sections: BTreeMap<i64, Option<Box<BlockStates>>> = BTreeMap::new();
        for i in 0..6i64 {
            sections.insert(
                i,
                Some(Box::new(states_from(|j| {
                    (j as u32 * 7 + i as u32) % 300 + 1000
                }))),
            );
        }
        // Force splits with a few block entities of max-ish size first.
        let mut be = BTreeMap::new();
        for i in 0..3i64 {
            be.insert(
                i,
                CompressedBlockEntity {
                    original_size: 1,
                    compression_dict: 0,
                    compressed: vec![1; 500_000],
                },
            );
        }
        let packets = chunk_response_packets(5, &be, &sections, 15);
        assert!(packets.iter().all(|p| p.len() <= MAX_PACKET_SIZE));
        let (back_be, back_sec) = read_back(&packets, 15);
        assert_eq!(back_be.len(), 3);
        assert_eq!(back_sec.len(), 6);
        for ((key, states), (want_key, want)) in back_sec.iter().zip(sections.iter()) {
            assert_eq!(key, want_key);
            assert_eq!(states.as_ref().unwrap()[..], want.as_ref().unwrap()[..]);
        }
    }

    #[test]
    fn entity_response_layout() {
        let bytes = entity_response_bytes(11, false, &[]);
        let mut expect = 11i64.to_be_bytes().to_vec();
        expect.push(0);
        expect.push(0);
        assert_eq!(bytes, expect);
    }
}
