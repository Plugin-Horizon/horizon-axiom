// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): buffer/BlockBuffer.java
//
// Sparse block buffer: 16^3 sections of global block state ids plus compressed block entities per section.
// Java's `EMPTY_STATE` (void_air) is passed in as `empty`; it reads as "no block" for get/remove.
// Java's version-translation path (ViaVersion, UnknownVersionHelper) and the custom-empty-state
// ID remapping (getIdMapForEmptyBlockState) are not ported; ids are passed through unchanged.
// `save` is not in the Java source; it mirrors `load` so buffers can be round-tripped in tests.

use std::collections::{BTreeMap, HashMap};

use super::paletted::{self, LongArrays, Section};
use super::{read_i16, section_index, section_key, write_i16, BufferError, CompressedBlockEntity, END_OF_POSITIONS, SECTION_VOLUME};
use crate::buf::{Reader, Writer};

/// Largest number of block entities Java accepts per section on load.
const MAX_BLOCK_ENTITIES_PER_SECTION: i32 = 4096;

pub struct BlockBuffer {
    empty: u32,
    global_bits: u8,
    sections: HashMap<i64, Box<Section>>,
    block_entities: HashMap<i64, BTreeMap<i16, CompressedBlockEntity>>,
    total_block_entities: u64,
    total_block_entity_bytes: u64,
}

impl BlockBuffer {
    /// `empty` is the sentinel global id for "no block" (void_air).
    /// `global_bits` is the bit count of the global block state palette (`ceil(log2(state count))`, 9..=32).
    pub fn new(empty: u32, global_bits: u8) -> Self {
        BlockBuffer {
            empty,
            global_bits,
            sections: HashMap::new(),
            block_entities: HashMap::new(),
            total_block_entities: 0,
            total_block_entity_bytes: 0,
        }
    }

    pub fn empty(&self) -> u32 {
        self.empty
    }

    /// Reads entries until the end marker. Corresponds to `BlockBuffer.load`.
    /// `framing`: how the sender frames section longs (an older client's, [`LongArrays::for_protocol`]).
    pub fn load(r: &mut Reader, empty: u32, global_bits: u8, framing: LongArrays) -> Result<Self, BufferError> {
        let mut buffer = BlockBuffer::new(empty, global_bits);
        loop {
            let index = r.read_i64()?;
            if index == END_OF_POSITIONS {
                break;
            }
            let section = paletted::read_section(r, global_bits, framing)?;
            buffer.sections.insert(index, section);

            let size = r.read_varint()?.min(MAX_BLOCK_ENTITIES_PER_SECTION);
            if size > 0 {
                let start = r.remaining();
                let mut map = BTreeMap::new();
                for _ in 0..size {
                    let offset = read_i16(r)?;
                    let entity = CompressedBlockEntity::read(r)?;
                    map.insert(offset, entity);
                }
                buffer.block_entities.insert(index, map);
                buffer.total_block_entities += size as u64;
                buffer.total_block_entity_bytes += (start - r.remaining()) as u64;
            }
        }
        Ok(buffer)
    }

    /// Every block state replaced by `f` (a client's ids to the server's), the buffer then in the server's
    /// palette width `global_bits`. Java decodes with the player's mapped registry instead; the result is the same.
    pub fn map_states(&mut self, global_bits: u8, f: impl Fn(u32) -> u32) {
        for section in self.sections.values_mut() {
            for v in section.iter_mut() {
                *v = f(*v);
            }
        }
        self.global_bits = global_bits;
    }

    /// Writes sections (sorted by key) and block entities, then the end marker.
    pub fn save(&self, w: &mut Writer) -> Result<(), BufferError> {
        let mut keys: Vec<i64> = self.sections.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            w.write_i64(key);
            paletted::write_section(w, &self.sections[&key], self.global_bits)?;
            match self.block_entities.get(&key) {
                Some(map) => {
                    if map.len() > MAX_BLOCK_ENTITIES_PER_SECTION as usize {
                        return Err(BufferError::TooManyBlockEntities(map.len()));
                    }
                    w.write_varint(map.len() as i32);
                    for (&offset, entity) in map {
                        write_i16(w, offset);
                        entity.write(w);
                    }
                }
                None => w.write_varint(0),
            }
        }
        w.write_i64(END_OF_POSITIONS);
        Ok(())
    }

    pub fn total_block_entities(&self) -> u64 {
        self.total_block_entities
    }

    pub fn total_block_entity_bytes(&self) -> u64 {
        self.total_block_entity_bytes
    }

    /// Block entities of one section, keyed by the i16 offset read from the wire.
    pub fn block_entities(&self, section: i64) -> Option<&BTreeMap<i16, CompressedBlockEntity>> {
        self.block_entities.get(&section)
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// All sections as `(section key, dense ids)`. Keys are `section_key` values.
    pub fn sections(&self) -> impl Iterator<Item = (i64, &Section)> {
        self.sections.iter().map(|(&k, v)| (k, &**v))
    }

    /// Block state id at a block position, or `None` if the section is absent or the cell is empty.
    pub fn get(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let section = self.sections.get(&section_key(x, y, z))?;
        let state = section[section_index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)];
        (state != self.empty).then_some(state)
    }

    pub fn set(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let section = self.section_or_create(section_key(x, y, z));
        section[section_index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)] = state;
    }

    /// Like `set`, with the section coordinates and the in-section coordinates (each 0..16) given separately.
    #[allow(clippy::too_many_arguments)]
    pub fn set_local(&mut self, cx: i32, cy: i32, cz: i32, lx: usize, ly: usize, lz: usize, state: u32) {
        assert!(lx < 16 && ly < 16 && lz < 16, "local coordinates must be 0..16");
        let section = self.section_or_create(section_key(cx << 4, cy << 4, cz << 4));
        section[section_index(lx, ly, lz)] = state;
    }

    /// Clears a position and returns the old state, or `None` if it was already empty.
    pub fn remove(&mut self, x: i32, y: i32, z: i32) -> Option<u32> {
        let empty = self.empty;
        let section = self.sections.get_mut(&section_key(x, y, z))?;
        let cell = &mut section[section_index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)];
        if *cell == empty {
            None
        } else {
            let old = *cell;
            *cell = empty;
            Some(old)
        }
    }

    fn section_or_create(&mut self, key: i64) -> &mut Section {
        let empty = self.empty;
        self.sections
            .entry(key)
            .or_insert_with(|| Box::new([empty; SECTION_VOLUME]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VOID: u32 = 1;
    const STONE: u32 = 17;
    const DIRT: u32 = 3000;

    #[test]
    fn set_get_remove_across_negative_coordinates() {
        let mut b = BlockBuffer::new(VOID, 15);
        assert_eq!(b.get(-1, -1, -1), None);
        b.set(-1, -1, -1, STONE);
        b.set(0, 0, 0, DIRT);
        assert_eq!(b.get(-1, -1, -1), Some(STONE));
        assert_eq!(b.get(0, 0, 0), Some(DIRT));
        assert_eq!(b.section_count(), 2);
        assert_eq!(b.remove(-1, -1, -1), Some(STONE));
        assert_eq!(b.remove(-1, -1, -1), None);
        assert_eq!(b.get(-1, -1, -1), None);
    }

    #[test]
    fn map_states_renumbers_every_block_and_takes_the_servers_width() {
        // a client on another release: its stone is 16, its dirt 2999
        let mut b = BlockBuffer::new(VOID, 14);
        b.set(0, 0, 0, 16);
        b.set(-20, 3, 7, 2999);
        b.map_states(15, |v| match v {
            16 => STONE,
            2999 => DIRT,
            other => other,
        });
        assert_eq!(b.get(0, 0, 0), Some(STONE));
        assert_eq!(b.get(-20, 3, 7), Some(DIRT));
        assert_eq!(b.global_bits, 15);
    }

    #[test]
    fn empty_state_reads_as_none() {
        let mut b = BlockBuffer::new(VOID, 15);
        b.set(5, 5, 5, VOID);
        assert_eq!(b.get(5, 5, 5), None);
        assert_eq!(b.remove(5, 5, 5), None);
    }

    #[test]
    fn set_local_matches_set() {
        let mut b = BlockBuffer::new(VOID, 15);
        b.set_local(-1, 0, 2, 3, 4, 5, STONE);
        assert_eq!(b.get(-16 + 3, 4, 32 + 5), Some(STONE));
    }

    #[test]
    fn empty_buffer_saves_only_end_marker_and_loads_back() {
        let b = BlockBuffer::new(VOID, 15);
        let mut w = Writer::new();
        b.save(&mut w).unwrap();
        let bytes = w.into_bytes();
        assert_eq!(bytes.len(), 8);
        let back = BlockBuffer::load(&mut Reader::new(&bytes), VOID, 15, LongArrays::Fixed).unwrap();
        assert_eq!(back.section_count(), 0);
    }

    #[test]
    fn save_load_round_trip_with_block_entities() {
        let mut b = BlockBuffer::new(VOID, 15);
        for i in 0..SECTION_VOLUME {
            b.set(i as i32 % 16 - 20, (i as i32 / 256) % 16, (i as i32 / 16) % 16, if i % 2 == 0 { STONE } else { DIRT + (i as u32 % 5) });
        }
        b.set(100, 100, 100, STONE);
        b.block_entities.insert(
            section_key(-20, 0, 0),
            BTreeMap::from([(
                5i16,
                CompressedBlockEntity { original_size: 12, compression_dict: 0, compressed: vec![1, 2, 3] },
            )]),
        );

        let mut w = Writer::new();
        b.save(&mut w).unwrap();
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let back = BlockBuffer::load(&mut r, VOID, 15, LongArrays::Fixed).unwrap();
        assert_eq!(r.remaining(), 0);

        assert_eq!(back.section_count(), b.section_count());
        for x in -20..100 {
            for y in 0..16 {
                for z in 0..16 {
                    assert_eq!(back.get(x, y, z), b.get(x, y, z), "at {x} {y} {z}");
                }
            }
        }
        assert_eq!(back.get(100, 100, 100), Some(STONE));
        assert_eq!(back.total_block_entities(), 1);
        assert_eq!(back.block_entities(section_key(-20, 0, 0)).unwrap().len(), 1);
        assert!(back.total_block_entity_bytes() > 0);
    }

    #[test]
    fn load_counts_block_entity_bytes() {
        let mut w = Writer::new();
        w.write_i64(section_key(0, 0, 0));
        // Single-value section, value VOID.
        w.write_bytes(&[0]);
        w.write_varint(VOID as i32);
        // Two block entities.
        w.write_varint(2);
        for off in [7i16, 9] {
            write_i16(&mut w, off);
            CompressedBlockEntity { original_size: 1, compression_dict: 0, compressed: vec![0xAA] }.write(&mut w);
        }
        w.write_i64(END_OF_POSITIONS);
        let bytes = w.into_bytes();
        let back = BlockBuffer::load(&mut Reader::new(&bytes), VOID, 15, LongArrays::Fixed).unwrap();
        assert_eq!(back.total_block_entities(), 2);
        assert_eq!(back.total_block_entity_bytes(), 2 * (2 + 1 + 1 + 1 + 1));
        assert_eq!(back.get(0, 0, 0), None); // whole section is void
    }

    #[test]
    fn negative_block_entity_count_is_skipped() {
        let mut w = Writer::new();
        w.write_i64(section_key(0, 0, 0));
        w.write_bytes(&[0]);
        w.write_varint(DIRT as i32);
        w.write_varint(-1);
        w.write_i64(END_OF_POSITIONS);
        let bytes = w.into_bytes();
        let back = BlockBuffer::load(&mut Reader::new(&bytes), VOID, 15, LongArrays::Fixed).unwrap();
        assert_eq!(back.total_block_entities(), 0);
        assert_eq!(back.get(3, 3, 3), Some(DIRT));
    }
}
