// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): PalettedContainer<BlockState> as used by
// blueprint/BlueprintIo.java (disk) and blueprint/RawBlueprint.java (wire), plus the empty block mapping of
// buffer/BlockBuffer.getCodecForEmptyBlockState.
//
// Wire form: u8 bits; if bits == 0 a single varint id; if 1..=8 a varint palette length and varint ids; then fixed
// longs (no length prefix), 64 / bits values per long, never spanning two longs. Bits 9 and up mean a global
// palette: no palette, values are global ids at the global width.
// Disk form: a compound `{palette: [block state compounds], data: long[]}` with indices into the palette.

use std::collections::HashMap;

use crate::buf::{Nbt, NbtCompound, Reader, Writer};

use super::{BlueprintError, StateIds};

/// Entries in one 16x16x16 section.
pub const SECTION_ENTRIES: usize = 4096;

/// One section of block states, indexed `(y << 8) | (z << 4) | x` as in `PalettedContainer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSection {
    states: Vec<u32>,
}

impl BlockSection {
    pub fn filled(state: u32) -> Self {
        Self { states: vec![state; SECTION_ENTRIES] }
    }

    pub fn from_states(states: Vec<u32>) -> Result<Self, BlueprintError> {
        if states.len() != SECTION_ENTRIES {
            return Err(BlueprintError::Malformed(format!(
                "section has {} entries, expected {SECTION_ENTRIES}",
                states.len()
            )));
        }
        Ok(Self { states })
    }

    pub fn states(&self) -> &[u32] {
        &self.states
    }

    /// Global id at block coordinates; only the low 4 bits of each coordinate are used.
    pub fn get(&self, x: i32, y: i32, z: i32) -> u32 {
        self.states[index(x, y, z)]
    }

    pub fn set(&mut self, x: i32, y: i32, z: i32, state: u32) {
        self.states[index(x, y, z)] = state;
    }
}

fn index(x: i32, y: i32, z: i32) -> usize {
    (((y & 15) << 8) | ((z & 15) << 4) | (x & 15)) as usize
}

/// `Mth.ceillog2`: bits needed to address `n` distinct values.
fn ceil_log2(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

/// Longs needed for `SECTION_ENTRIES` values at `bits` each (`SimpleBitStorage`).
fn storage_len(bits: u32) -> usize {
    match 64u32.checked_div(bits) {
        Some(per) => SECTION_ENTRIES.div_ceil(per as usize),
        None => 0,
    }
}

fn storage_get(data: &[i64], bits: u32, i: usize) -> u32 {
    let per = (64 / bits) as usize;
    let (cell, offset) = (i / per, (i % per) * bits as usize);
    ((data[cell] as u64 >> offset) & ((1u64 << bits) - 1)) as u32
}

fn storage_pack(values: &[u32], bits: u32) -> Vec<i64> {
    let per = (64 / bits) as usize;
    let mut out = vec![0i64; values.len().div_ceil(per)];
    for (i, &value) in values.iter().enumerate() {
        out[i / per] |= (u64::from(value) << ((i % per) * bits as usize)) as i64;
    }
    out
}

/// Distinct values in first-seen order, and each entry's index into them.
fn palette_of(states: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut palette = Vec::new();
    let mut slot: HashMap<u32, u32> = HashMap::new();
    let mut indices = Vec::with_capacity(states.len());
    for &state in states {
        let index = *slot.entry(state).or_insert_with(|| {
            palette.push(state);
            (palette.len() - 1) as u32
        });
        indices.push(index);
    }
    (palette, indices)
}

/// Writes a section in the wire form, choosing the palette the way the server container would.
pub(crate) fn write_wire(states: &[u32], global_bits: u8, w: &mut Writer) {
    let (palette, indices) = palette_of(states);
    match palette.len() {
        1 => {
            w.write_u8(0);
            w.write_varint(palette[0] as i32);
        }
        n if n <= 256 => {
            let bits = if n <= 16 { 4 } else { ceil_log2(n) };
            w.write_u8(bits as u8);
            w.write_varint(n as i32);
            for &id in &palette {
                w.write_varint(id as i32);
            }
            for long in storage_pack(&indices, bits) {
                w.write_i64(long);
            }
        }
        _ => {
            let max = palette.iter().copied().max().unwrap_or(0);
            let bits = u32::from(global_bits).max(ceil_log2(max as usize + 1));
            w.write_u8(bits as u8);
            for long in storage_pack(states, bits) {
                w.write_i64(long);
            }
        }
    }
}

/// Reads a section in the wire form. Ids are returned as sent, before the empty-state mapping.
pub(crate) fn read_wire(r: &mut Reader, global_bits: u8) -> Result<Vec<u32>, BlueprintError> {
    let wire = u32::from(r.read_u8()?);
    let (has_palette, bits) = match wire {
        0 => (true, 0),
        1..=4 => (true, 4),
        5..=8 => (true, wire),
        _ => (false, u32::from(global_bits).max(wire)),
    };
    if bits > 32 {
        return Err(BlueprintError::Malformed(format!("{bits} bits per block state entry")));
    }

    let mut palette = Vec::new();
    if has_palette {
        if bits == 0 {
            palette.push(r.read_varint()? as u32);
        } else {
            let count = usize::try_from(r.read_varint()?)
                .map_err(|_| BlueprintError::Malformed("negative palette length".into()))?;
            if count > 1usize << bits {
                return Err(BlueprintError::Malformed(format!("palette of {count} entries at {bits} bits")));
            }
            for _ in 0..count {
                palette.push(r.read_varint()? as u32);
            }
        }
    }

    let mut data = Vec::with_capacity(storage_len(bits));
    for _ in 0..storage_len(bits) {
        data.push(r.read_i64()?);
    }

    let mut states = Vec::with_capacity(SECTION_ENTRIES);
    for i in 0..SECTION_ENTRIES {
        let state = if bits == 0 {
            palette[0]
        } else {
            let value = storage_get(&data, bits, i) as usize;
            if has_palette {
                *palette
                    .get(value)
                    .ok_or_else(|| BlueprintError::Malformed(format!("palette index {value} out of range")))?
            } else {
                value as u32
            }
        };
        states.push(state);
    }
    Ok(states)
}

/// Writes a section as the disk compound `{palette, data}`. `to_nbt` encodes one block state.
pub(crate) fn write_disk(states: &[u32], to_nbt: impl Fn(u32) -> Nbt) -> Nbt {
    let (palette, indices) = palette_of(states);
    let bits = if palette.len() <= 1 { 0 } else { ceil_log2(palette.len()).max(4) };
    let data = if bits == 0 { Vec::new() } else { storage_pack(&indices, bits) };

    let mut map = NbtCompound::new();
    map.insert("palette".to_owned(), Nbt::List(palette.into_iter().map(to_nbt).collect()));
    map.insert("data".to_owned(), Nbt::LongArray(data));
    Nbt::Compound(map)
}

/// Reads the disk compound. `resolve` turns one palette entry into a global id (it runs any data fixer first).
pub(crate) fn read_disk(
    tag: &Nbt,
    mut resolve: impl FnMut(&Nbt) -> Result<u32, BlueprintError>,
) -> Result<Vec<u32>, BlueprintError> {
    let palette_tag = tag
        .get("palette")
        .and_then(Nbt::as_list)
        .ok_or_else(|| BlueprintError::Malformed("block states without a palette".into()))?;
    if palette_tag.is_empty() {
        return Err(BlueprintError::Malformed("empty block state palette".into()));
    }
    let palette = palette_tag.iter().map(&mut resolve).collect::<Result<Vec<u32>, _>>()?;

    let bits = if palette.len() == 1 { 0 } else { ceil_log2(palette.len()).max(4) };
    let data: &[i64] = match tag.get("data") {
        None => &[],
        Some(Nbt::LongArray(longs)) => longs,
        Some(_) => return Err(BlueprintError::Malformed("block state data is not a long array".into())),
    };
    if bits > 0 && data.len() != storage_len(bits) {
        return Err(BlueprintError::Malformed(format!(
            "block state data has {} longs, expected {}",
            data.len(),
            storage_len(bits)
        )));
    }

    (0..SECTION_ENTRIES)
        .map(|i| {
            let value = if bits == 0 { 0 } else { storage_get(data, bits, i) as usize };
            palette
                .get(value)
                .copied()
                .ok_or_else(|| BlueprintError::Malformed(format!("palette index {value} out of range")))
        })
        .collect()
}

/// Blueprint format 1 stores structure void as its empty block. Loaded blocks of that state become void air,
/// which is the in-memory empty block for every version (`BlockBuffer.getCodecForEmptyBlockState`).
pub(crate) fn normalize_empty(states: &mut [u32], version: i32, ids: StateIds) {
    if version <= 1 && ids.structure_void != ids.void_air {
        for state in states.iter_mut() {
            if *state == ids.structure_void {
                *state = ids.void_air;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::test_host::{FakeHost, STONE, STRUCTURE_VOID_ID, VOID_AIR_ID};
    use crate::blueprint::BlueprintHost;

    /// Patterns that hit each palette configuration: single, linear (4 bits), hash map (5-8 bits), global (9+).
    fn patterns() -> Vec<(&'static str, Vec<u32>)> {
        let pick = |distinct: u32| -> Vec<u32> { (0..SECTION_ENTRIES as u32).map(|i| i % distinct).collect() };
        vec![
            ("single", vec![7; SECTION_ENTRIES]),
            ("two", pick(2)),
            ("sixteen", pick(16)),
            ("seventeen", pick(17)),
            ("two hundred", pick(200)),
            ("three hundred", pick(300)),
            ("scattered", (0..SECTION_ENTRIES as u32).map(|i| (i * 7919) % 9000).collect()),
        ]
    }

    #[test]
    fn wire_round_trip_for_every_palette_configuration() {
        for (name, states) in patterns() {
            let mut w = Writer::new();
            write_wire(&states, 15, &mut w);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            let back = read_wire(&mut r, 15).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(back, states, "{name}");
            assert!(r.is_finished(), "{name}: trailing bytes");
        }
    }

    #[test]
    fn single_palette_is_bits_zero_and_one_varint() {
        let mut w = Writer::new();
        write_wire(&vec![300; SECTION_ENTRIES], 15, &mut w);
        // bits 0, varint 300 = 0xAC 0x02
        assert_eq!(w.into_bytes(), vec![0, 0xAC, 0x02]);
    }

    #[test]
    fn linear_palette_uses_four_bits_and_fixed_longs() {
        let states: Vec<u32> = (0..SECTION_ENTRIES as u32).map(|i| i % 2).collect();
        let mut w = Writer::new();
        write_wire(&states, 15, &mut w);
        let bytes = w.into_bytes();
        // bits 4, palette length 2, ids 0 and 1, then 256 longs of 16 nibbles each (no length prefix)
        assert_eq!(bytes[0], 4);
        assert_eq!(bytes[1], 2);
        assert_eq!(bytes.len(), 1 + 1 + 2 + 256 * 8);
    }

    #[test]
    fn disk_round_trip_for_every_palette_configuration() {
        let host = FakeHost::new();
        for (name, states) in patterns() {
            let tag = write_disk(&states, |s| host.state_to_nbt(s));
            let back = read_disk(&tag, |entry| {
                host.state_from_nbt(entry)
                    .ok_or_else(|| BlueprintError::Malformed("unknown".into()))
            })
            .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(back, states, "{name}");
        }
    }

    #[test]
    fn disk_single_palette_has_empty_data() {
        let host = FakeHost::new();
        let tag = write_disk(&vec![STONE; SECTION_ENTRIES], |s| host.state_to_nbt(s));
        match tag.get("data") {
            Some(Nbt::LongArray(longs)) => assert!(longs.is_empty()),
            other => panic!("unexpected data tag {other:?}"),
        }
    }

    #[test]
    fn disk_rejects_wrong_data_length() {
        let host = FakeHost::new();
        let mut tag = write_disk(&(0..SECTION_ENTRIES as u32).map(|i| i % 2).collect::<Vec<_>>(), |s| {
            host.state_to_nbt(s)
        });
        if let Nbt::Compound(map) = &mut tag {
            map.insert("data".to_owned(), Nbt::LongArray(vec![0; 3]));
        }
        let result = read_disk(&tag, |entry| host.state_from_nbt(entry).ok_or(BlueprintError::NotValid));
        assert!(matches!(result, Err(BlueprintError::Malformed(_))));
    }

    #[test]
    fn structure_void_becomes_void_air_only_in_format_one() {
        let mut states = vec![STRUCTURE_VOID_ID, STONE, VOID_AIR_ID];
        let ids = StateIds { global_bits: 15, void_air: VOID_AIR_ID, structure_void: STRUCTURE_VOID_ID };

        normalize_empty(&mut states, 1, ids);
        assert_eq!(states, vec![VOID_AIR_ID, STONE, VOID_AIR_ID]);

        let mut states = vec![STRUCTURE_VOID_ID];
        normalize_empty(&mut states, 2, ids);
        assert_eq!(states, vec![STRUCTURE_VOID_ID]);
    }
}
