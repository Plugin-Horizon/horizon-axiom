// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): buffer/BiomeBuffer.java
//
// Biome buffer over quart coordinates (4x4x4 blocks). The map stores 1-based palette indices (0 = none).
// Biome keys are plain resource-location strings ("minecraft:plains"); no registry types are used.

use std::collections::HashMap;

use super::{read_u8, BufferError, Position2ByteMap};
use crate::buf::{Reader, Writer};

/// Maximum palette entries: stored indices are 1..=255 in one byte.
const MAX_PALETTE: usize = 255;
/// Identifier length limit used by `FriendlyByteBuf.readResourceKey`.
const MAX_IDENTIFIER_LEN: usize = 32767;

pub struct BiomeBuffer {
    pub map: Position2ByteMap,
    palette: Vec<String>,
    palette_reverse: HashMap<String, u8>,
}

impl Default for BiomeBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl BiomeBuffer {
    pub fn new() -> Self {
        BiomeBuffer { map: Position2ByteMap::new(), palette: Vec::new(), palette_reverse: HashMap::new() }
    }

    pub fn section_count(&self) -> usize {
        self.map.size()
    }

    pub fn save(&self, w: &mut Writer) {
        w.write_bytes(&[self.palette.len() as u8]);
        for name in &self.palette {
            w.write_string(name);
        }
        self.map.save(w);
    }

    /// Reads the palette, then the map. The palette count is read as an unsigned byte
    /// (Java reads it signed, which breaks at 128 or more entries).
    pub fn load(r: &mut Reader) -> Result<Self, BufferError> {
        let count = read_u8(r)? as usize;
        let mut palette = Vec::with_capacity(count);
        let mut palette_reverse = HashMap::with_capacity(count);
        for i in 0..count {
            let name = r.read_string(MAX_IDENTIFIER_LEN)?.to_string();
            palette_reverse.insert(name.clone(), (i + 1) as u8);
            palette.push(name);
        }
        let map = Position2ByteMap::load(r)?;
        Ok(BiomeBuffer { map, palette, palette_reverse })
    }

    /// Clears the map. The palette is kept, as in Java.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Calls `f` for every quart with a biome.
    pub fn for_each_entry(&self, mut f: impl FnMut(i32, i32, i32, &str)) {
        self.map.for_each_entry(|x, y, z, v| {
            let index = v as u8 as usize;
            if index != 0 {
                if let Some(name) = self.palette.get(index - 1) {
                    f(x, y, z, name);
                }
            }
        });
    }

    pub fn get(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> Option<&str> {
        let index = self.map.get(quart_x, quart_y, quart_z) as u8 as usize;
        if index == 0 {
            None
        } else {
            self.palette.get(index - 1).map(String::as_str)
        }
    }

    pub fn set(&mut self, quart_x: i32, quart_y: i32, quart_z: i32, biome: &str) -> Result<(), BufferError> {
        let index = self.palette_index(biome)?;
        self.map.put(quart_x, quart_y, quart_z, index as i8);
        Ok(())
    }

    fn palette_index(&mut self, biome: &str) -> Result<u8, BufferError> {
        if let Some(&index) = self.palette_reverse.get(biome) {
            return Ok(index);
        }
        if self.palette.len() >= MAX_PALETTE {
            return Err(BufferError::TooManyBiomes);
        }
        self.palette.push(biome.to_string());
        let index = self.palette.len() as u8;
        self.palette_reverse.insert(biome.to_string(), index);
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_default() {
        let mut b = BiomeBuffer::new();
        assert_eq!(b.get(0, 0, 0), None);
        b.set(-1, 2, 3, "minecraft:plains").unwrap();
        b.set(4, 4, 4, "minecraft:desert").unwrap();
        b.set(5, 4, 4, "minecraft:plains").unwrap();
        assert_eq!(b.get(-1, 2, 3), Some("minecraft:plains"));
        assert_eq!(b.get(4, 4, 4), Some("minecraft:desert"));
        assert_eq!(b.get(5, 4, 4), Some("minecraft:plains"));
        assert_eq!(b.get(9, 9, 9), None);
        assert_eq!(b.section_count(), 2);
    }

    #[test]
    fn for_each_entry_reports_names() {
        let mut b = BiomeBuffer::new();
        b.set(1, 1, 1, "minecraft:jungle").unwrap();
        let mut seen = Vec::new();
        b.for_each_entry(|x, y, z, name| seen.push((x, y, z, name.to_string())));
        assert_eq!(seen, vec![(1, 1, 1, "minecraft:jungle".to_string())]);
    }

    #[test]
    fn palette_limit_is_255() {
        let mut b = BiomeBuffer::new();
        for i in 0..255 {
            b.set(i, 0, 0, &format!("ns:biome{i}")).unwrap();
        }
        // Existing names still work at the limit.
        b.set(0, 1, 0, "ns:biome7").unwrap();
        assert_eq!(b.get(0, 1, 0), Some("ns:biome7"));
        assert!(matches!(b.set(0, 0, 1, "ns:one_too_many"), Err(BufferError::TooManyBiomes)));
    }

    #[test]
    fn save_load_round_trip() {
        let mut b = BiomeBuffer::new();
        b.set(0, 0, 0, "minecraft:plains").unwrap();
        b.set(-7, 30, 2, "minecraft:swamp").unwrap();
        let mut w = Writer::new();
        b.save(&mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let back = BiomeBuffer::load(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(back.get(0, 0, 0), Some("minecraft:plains"));
        assert_eq!(back.get(-7, 30, 2), Some("minecraft:swamp"));
        assert_eq!(back.get(1, 1, 1), None);
        assert_eq!(back.section_count(), 2);
    }

    #[test]
    fn load_palette_count_above_127() {
        let mut w = Writer::new();
        w.write_bytes(&[200]);
        for i in 0..200 {
            w.write_string(&format!("ns:b{i}"));
        }
        w.write_bytes(&[0]); // default value
        w.write_i64(crate::buffer::END_OF_POSITIONS);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let b = BiomeBuffer::load(&mut r).unwrap();
        assert_eq!(b.palette.len(), 200);
    }
}
