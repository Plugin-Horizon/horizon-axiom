// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): buffer/Position2ByteMap.java,
// buffer/PositionSet.java
//
// Sparse maps keyed by 16^3 section. Cell index inside a section is `x + y*16 + z*256` (Position2ByteMap)
// or, for PositionSet, `z*16 + y` per 16-bit row with bit `x`.

use std::collections::HashMap;

use super::{pack_pos, read_i16, read_i8, BufferError, END_OF_POSITIONS, SECTION_VOLUME};
use crate::buf::{Reader, Writer};

type ByteSection = Box<[i8; SECTION_VOLUME]>;

/// Sparse map from block (or quart) position to a byte, with a default for absent cells.
#[derive(Debug, Clone)]
pub struct Position2ByteMap {
    default_value: i8,
    map: HashMap<i64, ByteSection>,
}

fn local_index(x: i32, y: i32, z: i32) -> usize {
    ((x & 15) + (y & 15) * 16 + (z & 15) * 256) as usize
}

impl Default for Position2ByteMap {
    fn default() -> Self {
        Self::new()
    }
}

impl Position2ByteMap {
    pub fn new() -> Self {
        Self::with_default(0)
    }

    pub fn with_default(default_value: i8) -> Self {
        Position2ByteMap { default_value, map: HashMap::new() }
    }

    pub fn size(&self) -> usize {
        self.map.len()
    }

    /// Writes the default value, every chunk (sorted by key so the output is deterministic),
    /// then the end marker.
    pub fn save(&self, w: &mut Writer) {
        w.write_bytes(&[self.default_value as u8]);
        let mut keys: Vec<i64> = self.map.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            w.write_i64(key);
            let bytes: Vec<u8> = self.map[&key].iter().map(|&b| b as u8).collect();
            w.write_bytes(&bytes);
        }
        w.write_i64(END_OF_POSITIONS);
    }

    pub fn load(r: &mut Reader) -> Result<Self, BufferError> {
        let default_value = read_i8(r)?;
        let mut map = HashMap::new();
        loop {
            let pos = r.read_i64()?;
            if pos == END_OF_POSITIONS {
                break;
            }
            let raw = r.read_bytes(SECTION_VOLUME)?;
            let mut section: ByteSection = Box::new([0i8; SECTION_VOLUME]);
            for (dst, &src) in section.iter_mut().zip(raw.iter()) {
                *dst = src as i8;
            }
            map.insert(pos, section);
        }
        Ok(Position2ByteMap { default_value, map })
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn get(&self, x: i32, y: i32, z: i32) -> i8 {
        match self.map.get(&pack_pos(x >> 4, y >> 4, z >> 4)) {
            Some(section) => section[local_index(x, y, z)],
            None => self.default_value,
        }
    }

    pub fn put(&mut self, x: i32, y: i32, z: i32, v: i8) {
        self.section_mut(x, y, z)[local_index(x, y, z)] = v;
    }

    /// Adds `v` with wrapping byte arithmetic and returns the new value. Adding 0 only reads.
    pub fn add(&mut self, x: i32, y: i32, z: i32, v: i8) -> i8 {
        if v == 0 {
            return self.get(x, y, z);
        }
        let cell = &mut self.section_mut(x, y, z)[local_index(x, y, z)];
        *cell = cell.wrapping_add(v);
        *cell
    }

    /// Bitwise AND with `v`, returns the new value.
    pub fn binary_and(&mut self, x: i32, y: i32, z: i32, v: i8) -> i8 {
        let cell = &mut self.section_mut(x, y, z)[local_index(x, y, z)];
        *cell &= v;
        *cell
    }

    /// Stores `v` if it is smaller (signed compare, as in Java) than the current value.
    pub fn min(&mut self, x: i32, y: i32, z: i32, v: i8) -> bool {
        let cell = &mut self.section_mut(x, y, z)[local_index(x, y, z)];
        if v < *cell {
            *cell = v;
            true
        } else {
            false
        }
    }

    /// Calls `f` for every cell whose value differs from the default.
    pub fn for_each_entry(&self, mut f: impl FnMut(i32, i32, i32, i8)) {
        for (&key, section) in &self.map {
            let (cx, cy, cz) = super::unpack_pos(key);
            let (cx, cy, cz) = (cx * 16, cy * 16, cz * 16);
            let mut index = 0;
            for z in 0..16 {
                for y in 0..16 {
                    for x in 0..16 {
                        let v = section[index];
                        index += 1;
                        if v != self.default_value {
                            f(cx + x, cy + y, cz + z, v);
                        }
                    }
                }
            }
        }
    }

    fn section_mut(&mut self, x: i32, y: i32, z: i32) -> &mut ByteSection {
        let default_value = self.default_value;
        self.map
            .entry(pack_pos(x >> 4, y >> 4, z >> 4))
            .or_insert_with(|| Box::new([default_value; SECTION_VOLUME]))
    }
}

/// Set of block positions, stored as one 16-bit row mask per (y, z) inside each section.
#[derive(Debug, Clone, Default)]
pub struct PositionSet {
    map: HashMap<i64, Box<[u16; 256]>>,
    count: usize,
}

impl PositionSet {
    pub fn chunk_count(&self) -> usize {
        self.map.len()
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Calls `f` for every position in the set.
    pub fn for_each(&self, mut f: impl FnMut(i32, i32, i32)) {
        for (&key, rows) in &self.map {
            let (cx, cy, cz) = super::unpack_pos(key);
            let (cx, cy, cz) = (cx * 16, cy * 16, cz * 16);
            for z in 0..16 {
                for y in 0..16 {
                    let v = rows[z * 16 + y];
                    for x in 0..16 {
                        if v & (1 << x) != 0 {
                            f(cx + x, cy + y as i32, cz + z as i32);
                        }
                    }
                }
            }
        }
    }

    /// Reads: varint chunk count, then per chunk an i64 section key and 256 big-endian u16 rows.
    pub fn read(r: &mut Reader) -> Result<Self, BufferError> {
        let size = r.read_varint()?;
        let mut map = HashMap::new();
        let mut count = 0usize;
        for _ in 0..size.max(0) {
            let pos = r.read_i64()?;
            let mut rows = Box::new([0u16; 256]);
            for row in rows.iter_mut() {
                let s = read_i16(r)? as u16;
                count += s.count_ones() as usize;
                *row = s;
            }
            map.insert(pos, rows);
        }
        Ok(PositionSet { map, count })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_bytes(w: Writer) -> Vec<u8> {
        w.into_bytes()
    }

    #[test]
    fn default_value_for_absent_cells() {
        let m = Position2ByteMap::with_default(5);
        assert_eq!(m.get(-100, 40, 3), 5);
        assert_eq!(m.size(), 0);
    }

    #[test]
    fn put_get_across_section_boundaries() {
        let mut m = Position2ByteMap::new();
        m.put(-1, -1, -1, 9);
        m.put(15, 15, 15, 7);
        m.put(16, 0, 0, 3);
        assert_eq!(m.get(-1, -1, -1), 9);
        assert_eq!(m.get(15, 15, 15), 7);
        assert_eq!(m.get(16, 0, 0), 3);
        assert_eq!(m.get(0, 0, 0), 0);
        assert_eq!(m.size(), 3);
    }

    #[test]
    fn add_wraps_and_zero_does_not_allocate() {
        let mut m = Position2ByteMap::new();
        assert_eq!(m.add(1, 2, 3, 0), 0);
        assert_eq!(m.size(), 0);
        assert_eq!(m.add(1, 2, 3, 100), 100);
        assert_eq!(m.add(1, 2, 3, 100), -56); // 200 wraps to -56 as a byte
    }

    #[test]
    fn binary_and_and_min() {
        let mut m = Position2ByteMap::with_default(0);
        m.put(0, 0, 0, 0b1111);
        assert_eq!(m.binary_and(0, 0, 0, 0b0101), 0b0101);
        assert!(m.min(0, 0, 0, 2));
        assert!(!m.min(0, 0, 0, 3));
        assert_eq!(m.get(0, 0, 0), 2);
    }

    #[test]
    fn for_each_entry_skips_default_and_gives_coords() {
        let mut m = Position2ByteMap::new();
        m.put(-17, 0, 31, 4);
        m.put(0, 0, 0, 1);
        let mut seen = Vec::new();
        m.for_each_entry(|x, y, z, v| seen.push((x, y, z, v)));
        seen.sort();
        assert_eq!(seen, vec![(-17, 0, 31, 4), (0, 0, 0, 1)]);
    }

    #[test]
    fn save_load_round_trip() {
        let mut m = Position2ByteMap::with_default(2);
        m.put(3, 4, 5, 9);
        m.put(-40, 100, -2, -7);
        let mut w = Writer::new();
        m.save(&mut w);
        let bytes = to_bytes(w);
        let mut r = Reader::new(&bytes);
        let back = Position2ByteMap::load(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(back.get(3, 4, 5), 9);
        assert_eq!(back.get(-40, 100, -2), -7);
        assert_eq!(back.get(0, 0, 0), 2);
        assert_eq!(back.size(), 2);
    }

    #[test]
    fn position_set_read_and_iterate() {
        // One chunk at section (0,0,0): row z=0,y=0 has bits 0 and 15; row z=1,y=2 has bit 3.
        let mut w = Writer::new();
        w.write_varint(1);
        w.write_i64(pack_pos(0, 0, 0));
        let mut rows = [0i16; 256];
        rows[0] = (1i16 << 0) | (1i16 << 15);
        rows[16 + 2] = 1 << 3;
        for s in rows {
            w.write_bytes(&s.to_be_bytes());
        }
        let bytes = to_bytes(w);
        let mut r = Reader::new(&bytes);
        let set = PositionSet::read(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(set.count(), 3);
        assert_eq!(set.chunk_count(), 1);
        let mut out = Vec::new();
        set.for_each(|x, y, z| out.push((x, y, z)));
        out.sort();
        assert_eq!(out, vec![(0, 0, 0), (3, 2, 1), (15, 0, 0)]);
    }

    #[test]
    fn position_set_empty() {
        let mut w = Writer::new();
        w.write_varint(0);
        let bytes = to_bytes(w);
        let set = PositionSet::read(&mut Reader::new(&bytes)).unwrap();
        assert!(set.is_empty());
    }
}
