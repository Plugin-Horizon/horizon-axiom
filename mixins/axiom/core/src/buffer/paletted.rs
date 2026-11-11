// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): wire layout of
// net.minecraft.world.level.chunk.PalettedContainer as used by buffer/BlockBuffer.java
// (VersionHelper.createPalettedContainer / PalettedContainer.read). Native protocol only;
// the ViaVersion translation path (UnknownVersionHelper) is not ported.
//
// Wire layout of one 16^3 section (vanilla since 1.21.5, protocol 770, `writeFixedSizeLongArray`):
//   u8 bits-per-entry
//   bits 0:    varint single value (no longs)
//   bits 4..=8: varint palette length, varint palette ids, longs
//   bits >= 9: longs (global ids, bit count = global_bits)
// Releases before 1.21.5 put a varint long count before the longs ([`LongArrays::Counted`]): a client the
// server's protocol adapter translates (Java: ViaVersion's `PALETTE_TYPE_1_18`).
// Entries are packed without crossing 64-bit boundaries: floor(64 / bits) per long.
// Cell index = y*256 + z*16 + x.

use std::collections::{HashMap, HashSet};

use super::{BufferError, SECTION_VOLUME};
use crate::buf::{Reader, Writer};

/// Smallest bit count for an indirect (palette) section, as in vanilla block states.
const MIN_INDIRECT_BITS: u8 = 4;
/// Largest bit count for an indirect (palette) section; above it the global palette is used.
const MAX_INDIRECT_BITS: u8 = 8;

/// Dense 16^3 section of global block state ids, indexed by `section_index`.
pub(super) type Section = [u32; SECTION_VOLUME];

fn check_global_bits(global_bits: u8) -> Result<(), BufferError> {
    if (MAX_INDIRECT_BITS + 1..=32).contains(&global_bits) {
        Ok(())
    } else {
        Err(BufferError::InvalidGlobalBits(global_bits))
    }
}

fn read_palette_id(r: &mut Reader) -> Result<u32, BufferError> {
    let v = r.read_varint()?;
    u32::try_from(v).map_err(|_| BufferError::NegativeValue(v))
}

/// How a section's longs are framed on the wire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LongArrays {
    /// 1.21.5 (protocol 770) and later: exactly as many longs as the bit count needs, no count.
    #[default]
    Fixed,
    /// Before 1.21.5: a varint long count, then the longs.
    Counted,
}

impl LongArrays {
    /// The framing a client on `protocol` uses (Java `ViaVersionHelper.getPalettedContainerType`).
    pub fn for_protocol(protocol: i32) -> Self {
        if protocol >= 770 { Self::Fixed } else { Self::Counted }
    }
}

/// Reads the data array: the longs, after a varint count that must match `expected` if `framing` has one.
fn read_storage(r: &mut Reader, expected: usize, framing: LongArrays) -> Result<Vec<u64>, BufferError> {
    if framing == LongArrays::Counted {
        let found = r.read_varint()?;
        if found < 0 || found as usize != expected {
            return Err(BufferError::StorageLengthMismatch { expected, found });
        }
    }
    let mut longs = Vec::with_capacity(expected);
    for _ in 0..expected {
        longs.push(r.read_i64()? as u64);
    }
    Ok(longs)
}

/// Number of longs needed for `SECTION_VOLUME` entries of `bits` bits each.
fn storage_len(bits: u8) -> usize {
    let per_long = 64 / bits as usize;
    SECTION_VOLUME.div_ceil(per_long)
}

fn unpack(longs: &[u64], bits: u8, index: usize) -> usize {
    let per_long = 64 / bits as usize;
    let shift = (index % per_long) * bits as usize;
    let mask = (1u64 << bits) - 1;
    ((longs[index / per_long] >> shift) & mask) as usize
}

fn pack(values: impl Iterator<Item = u64>, bits: u8) -> Vec<i64> {
    let per_long = 64 / bits as usize;
    let mut longs = vec![0u64; storage_len(bits)];
    for (i, v) in values.enumerate() {
        longs[i / per_long] |= v << ((i % per_long) * bits as usize);
    }
    longs.into_iter().map(|l| l as i64).collect()
}

/// Reads one paletted section and returns it dense.
pub(super) fn read_section(r: &mut Reader, global_bits: u8, framing: LongArrays) -> Result<Box<Section>, BufferError> {
    let bits = super::read_i8(r)?;
    if bits < 0 {
        return Err(BufferError::InvalidBitsPerEntry(bits));
    }
    let bits = bits as u8;
    let mut out = Box::new([0u32; SECTION_VOLUME]);

    if bits == 0 {
        let value = read_palette_id(r)?;
        read_storage(r, 0, framing)?;
        out.fill(value);
    } else if bits <= MAX_INDIRECT_BITS {
        // Vanilla pads small entry sizes up to the minimum indirect size.
        let bits = bits.max(MIN_INDIRECT_BITS);
        let len = r.read_varint()?;
        let max = 1usize << bits;
        if len < 0 || len as usize > max {
            return Err(BufferError::PaletteTooLarge { len, max });
        }
        let mut palette = Vec::with_capacity(len as usize);
        for _ in 0..len {
            palette.push(read_palette_id(r)?);
        }
        let longs = read_storage(r, storage_len(bits), framing)?;
        for (i, cell) in out.iter_mut().enumerate() {
            let index = unpack(&longs, bits, i);
            *cell = *palette.get(index).ok_or(BufferError::PaletteIndexOutOfRange {
                index,
                len: palette.len(),
            })?;
        }
    } else {
        // Like vanilla, entries above 8 bits always use the global palette bit count.
        check_global_bits(global_bits)?;
        let longs = read_storage(r, storage_len(global_bits), framing)?;
        for (i, cell) in out.iter_mut().enumerate() {
            *cell = unpack(&longs, global_bits, i) as u32;
        }
    }
    Ok(out)
}

/// Writes one section. Uses the single-value form, an indirect palette (up to 256 distinct ids)
/// or the global palette, as vanilla does.
pub(super) fn write_section(w: &mut Writer, data: &Section, global_bits: u8) -> Result<(), BufferError> {
    let distinct: HashSet<u32> = data.iter().copied().collect();

    if distinct.len() == 1 {
        // Single-value form: bits byte 0, palette value, no longs.
        w.write_bytes(&[0]);
        write_palette_id(w, data[0])?;
        return Ok(());
    }

    if distinct.len() <= 1 << MAX_INDIRECT_BITS {
        let mut palette: Vec<u32> = Vec::with_capacity(distinct.len());
        let mut index_of: HashMap<u32, u64> = HashMap::with_capacity(distinct.len());
        for &v in data.iter() {
            index_of.entry(v).or_insert_with(|| {
                palette.push(v);
                (palette.len() - 1) as u64
            });
        }
        let needed = ceil_log2(palette.len()).max(MIN_INDIRECT_BITS);
        w.write_bytes(&[needed]);
        w.write_varint(palette.len() as i32);
        for &v in &palette {
            write_palette_id(w, v)?;
        }
        let longs = pack(data.iter().map(|v| index_of[v]), needed);
        write_longs(w, &longs);
    } else {
        check_global_bits(global_bits)?;
        let mut values = Vec::with_capacity(SECTION_VOLUME);
        for &v in data.iter() {
            if global_bits < 32 && (v as u64) >> global_bits != 0 {
                return Err(BufferError::ValueTooLarge { value: v, bits: global_bits });
            }
            values.push(v as u64);
        }
        w.write_bytes(&[global_bits]);
        let longs = pack(values.into_iter(), global_bits);
        write_longs(w, &longs);
    }
    Ok(())
}

fn write_palette_id(w: &mut Writer, v: u32) -> Result<(), BufferError> {
    let signed = i32::try_from(v).map_err(|_| BufferError::NegativeValue(v as i32))?;
    w.write_varint(signed);
    Ok(())
}

/// The longs as vanilla 1.21.5+ writes them (`writeFixedSizeLongArray`, no count).
fn write_longs(w: &mut Writer, longs: &[i64]) {
    for &l in longs {
        w.write_i64(l);
    }
}

/// Smallest `b` with `2^b >= n` (`Mth.ceillog2`).
fn ceil_log2(n: usize) -> u8 {
    if n <= 1 {
        0
    } else {
        (usize::BITS - (n - 1).leading_zeros()) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(data: &Section, global_bits: u8) -> Box<Section> {
        let mut w = Writer::new();
        write_section(&mut w, data, global_bits).unwrap();
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let back = read_section(&mut r, global_bits, LongArrays::Fixed).unwrap();
        assert_eq!(r.remaining(), 0, "section must consume exactly its bytes");
        back
    }

    #[test]
    fn single_value_round_trip() {
        let data = [7u32; SECTION_VOLUME];
        assert_eq!(*round_trip(&data, 15), data);
    }

    #[test]
    fn indirect_round_trip_with_padding_bits() {
        let mut data = [0u32; SECTION_VOLUME];
        for i in 0..SECTION_VOLUME {
            data[i] = (i % 3) as u32 * 100;
        }
        assert_eq!(*round_trip(&data, 15), data);
    }

    #[test]
    fn indirect_round_trip_255_distinct_values() {
        let mut data = [0u32; SECTION_VOLUME];
        for i in 0..SECTION_VOLUME {
            data[i] = (i % 255) as u32;
        }
        assert_eq!(*round_trip(&data, 15), data);
    }

    #[test]
    fn global_palette_round_trip() {
        let mut data = [0u32; SECTION_VOLUME];
        for i in 0..SECTION_VOLUME {
            data[i] = (i * 7 % 20_000) as u32;
        }
        assert_eq!(*round_trip(&data, 15), data);
    }

    #[test]
    fn wire_bytes_for_single_value_match_vanilla() {
        let data = [5u32; SECTION_VOLUME];
        let mut w = Writer::new();
        write_section(&mut w, &data, 15).unwrap();
        // bits 0, palette value 5, no longs
        assert_eq!(w.into_bytes(), vec![0u8, 5]);
    }

    #[test]
    fn value_too_large_for_global_bits_is_rejected() {
        let mut data = [0u32; SECTION_VOLUME];
        for i in 0..SECTION_VOLUME {
            data[i] = i as u32; // 4096 distinct values, forces the global palette
        }
        data[10] = 1 << 15;
        let mut w = Writer::new();
        assert!(matches!(
            write_section(&mut w, &data, 15),
            Err(BufferError::ValueTooLarge { .. })
        ));
    }

    #[test]
    fn palette_index_out_of_range_is_an_error() {
        // bits 4, palette of length 1 (id 3), every packed index is 1 -> out of range.
        let mut w = Writer::new();
        w.write_bytes(&[4]);
        w.write_varint(1);
        w.write_varint(3);
        let longs = vec![0x1111_1111_1111_1111i64; storage_len(4)];
        write_longs(&mut w, &longs);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert!(matches!(
            read_section(&mut r, 15, LongArrays::Fixed),
            Err(BufferError::PaletteIndexOutOfRange { index: 1, len: 1 })
        ));
    }

    #[test]
    fn storage_length_mismatch_is_an_error() {
        let mut w = Writer::new();
        w.write_bytes(&[0]);
        w.write_varint(3);
        w.write_varint(1);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert!(matches!(
            read_section(&mut r, 15, LongArrays::Counted),
            Err(BufferError::StorageLengthMismatch { expected: 0, found: 1 })
        ));
    }

    #[test]
    fn an_older_clients_counted_longs_read_like_the_fixed_ones() {
        let mut data = [0u32; SECTION_VOLUME];
        for (i, v) in data.iter_mut().enumerate() {
            *v = (i % 3) as u32 * 100;
        }
        let mut w = Writer::new();
        write_section(&mut w, &data, 15).unwrap();
        let fixed = w.into_bytes();
        // the same section with the long count a pre-1.21.5 client puts after the palette:
        // bits, palette length 3, ids 0 / 100 / 200 (200 takes two varint bytes) = 6 bytes, then varint 256
        assert_eq!(&fixed[..6], &[4, 3, 0, 100, 0xC8, 0x01]);
        let mut counted = fixed[..6].to_vec();
        counted.extend_from_slice(&[0x80, 0x02]);
        counted.extend_from_slice(&fixed[6..]);
        let back = read_section(&mut Reader::new(&counted), 15, LongArrays::Counted).unwrap();
        assert_eq!(*back, data);
        assert!(read_section(&mut Reader::new(&fixed), 15, LongArrays::Counted).is_err());
    }

    /// The cases of `tests/fixtures/vanilla_26_3/` (written by vanilla 26.3's `PalettedContainer.write` over a
    /// 30000-entry id map, so 15 global bits; the generator is in the fixture folder's README).
    fn vanilla_cases() -> Vec<(&'static str, fn(usize) -> u32)> {
        vec![
            ("single", |_| 7),
            ("three", |i| (i % 3) as u32 * 100),
            ("seventeen", |i| 100 + (i % 17) as u32),
            ("distinct255", |i| (i % 255) as u32),
            ("global300", |i| 1000 + (i % 300) as u32),
            ("global_all", |i| (i * 7 % 20000) as u32),
        ]
    }

    #[test]
    fn reads_the_sections_vanilla_26_3_writes() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vanilla_26_3");
        for (name, f) in vanilla_cases() {
            let bytes = std::fs::read(dir.join(format!("{name}.bin"))).unwrap();
            let mut r = Reader::new(&bytes);
            let back = read_section(&mut r, 15, LongArrays::Fixed).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(r.remaining(), 0, "{name}: every byte read");
            assert!((0..SECTION_VOLUME).all(|i| back[i] == f(i)), "{name}: values");
        }
    }

    /// With `AXIOM_WRITE_SECTIONS=<dir>`, writes the same cases for vanilla to read back (README).
    #[test]
    fn write_sections_for_vanilla() {
        let Some(dir) = std::env::var_os("AXIOM_WRITE_SECTIONS") else { return };
        for (name, f) in vanilla_cases() {
            let mut data = [0u32; SECTION_VOLUME];
            for (i, v) in data.iter_mut().enumerate() {
                *v = f(i);
            }
            let mut w = Writer::new();
            write_section(&mut w, &data, 15).unwrap();
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.bin")), w.into_bytes()).unwrap();
        }
    }

    #[test]
    fn negative_bits_is_an_error() {
        let bytes = [0x80u8];
        let mut r = Reader::new(&bytes);
        assert!(matches!(read_section(&mut r, 15, LongArrays::Fixed), Err(BufferError::InvalidBitsPerEntry(-128))));
    }

    #[test]
    fn ceil_log2_values() {
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(3), 2);
        assert_eq!(ceil_log2(16), 4);
        assert_eq!(ceil_log2(17), 5);
        assert_eq!(ceil_log2(256), 8);
    }
}
