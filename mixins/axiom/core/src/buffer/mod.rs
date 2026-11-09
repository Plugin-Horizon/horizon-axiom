// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd):
//   buffer/BlockBuffer.java, buffer/BiomeBuffer.java, buffer/Position2ByteMap.java,
//   buffer/PositionSet.java, buffer/CompressedBlockEntity.java, buffer/PositionConsumer.java,
//   buffer/TriIntConsumer.java
//
// Sparse block and biome buffers that Axiom clients send to the server (and that the server
// keeps for pending edits). Java's functional interfaces (PositionConsumer, TriIntConsumer,
// Position2ByteMap.EntryConsumer) are plain closures here.

use crate::buf::{BufError, Reader, Writer};

mod biome_buffer;
mod block_buffer;
mod block_entity;
mod paletted;
mod position_map;

pub use biome_buffer::BiomeBuffer;
pub use block_buffer::BlockBuffer;
pub use block_entity::{zstd_compress, zstd_decompress, CompressedBlockEntity};
pub use position_map::{Position2ByteMap, PositionSet};

/// Terminator written after the last entry of a sparse map (`AxiomConstants.MIN_POSITION_LONG`).
pub const END_OF_POSITIONS: i64 = pack_pos(-33_554_432, -2048, -33_554_432);

/// Number of blocks or quarter-blocks in one 16x16x16 section.
pub const SECTION_VOLUME: usize = 16 * 16 * 16;

/// Errors of the buffer codecs.
#[derive(Debug)]
pub enum BufferError {
    /// Underlying byte reader/writer error.
    Buf(BufError),
    /// Bits-per-entry byte is negative.
    InvalidBitsPerEntry(i8),
    /// Global (direct) palette bit count outside 9..=32.
    InvalidGlobalBits(u8),
    /// Indirect palette has a size that cannot be addressed with its bit count.
    PaletteTooLarge { len: i32, max: usize },
    /// Palette index read from the data array is past the end of the palette.
    PaletteIndexOutOfRange { index: usize, len: usize },
    /// Data array length prefix does not match the expected size.
    StorageLengthMismatch { expected: usize, found: i32 },
    /// Negative varint where a length or id was expected.
    NegativeValue(i32),
    /// A value does not fit into the global palette bit count.
    ValueTooLarge { value: u32, bits: u8 },
    /// A biome palette already holds 255 entries.
    TooManyBiomes,
    /// A section holds more block entities than the wire format allows (4096).
    TooManyBlockEntities(usize),
    /// Block entity uses a compression dictionary this port does not know.
    UnknownCompressionDict(i8),
    /// zstd (dictionary 0) failed: malformed frame, wrong dictionary, or truncated input. The text is ruzstd's message.
    Zstd(String),
    /// The zstd frame decoded to a different length than the declared original size.
    DecompressedSizeMismatch { expected: usize, found: usize },
    /// The declared original size is above [`block_entity::MAX_ORIGINAL_SIZE`].
    OriginalSizeTooLarge(usize),
}

impl From<BufError> for BufferError {
    fn from(e: BufError) -> Self {
        BufferError::Buf(e)
    }
}

/// Packs block coordinates into the Minecraft `BlockPos` long (`BlockPos.asLong`).
pub const fn pack_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

/// Unpacks a `BlockPos` long into `(x, y, z)` (`BlockPos.getX/getY/getZ`).
pub const fn unpack_pos(l: i64) -> (i32, i32, i32) {
    let x = (l >> 38) as i32;
    let y = ((l << 52) >> 52) as i32;
    let z = ((l << 26) >> 38) as i32;
    (x, y, z)
}

/// Section key for a block position: the long of its 16^3 section coordinates.
pub fn section_key(x: i32, y: i32, z: i32) -> i64 {
    pack_pos(x >> 4, y >> 4, z >> 4)
}

/// Index of a cell inside a 16^3 section in the vanilla paletted-container order: `y*256 + z*16 + x`.
const fn section_index(x: usize, y: usize, z: usize) -> usize {
    (y << 8) | (z << 4) | x
}

fn read_u8(r: &mut Reader) -> Result<u8, BufferError> {
    let b = r.read_bytes(1)?;
    Ok(b[0])
}

fn read_i8(r: &mut Reader) -> Result<i8, BufferError> {
    Ok(read_u8(r)? as i8)
}

fn read_i16(r: &mut Reader) -> Result<i16, BufferError> {
    let b = r.read_bytes(2)?;
    Ok(i16::from_be_bytes([b[0], b[1]]))
}

fn write_i16(w: &mut Writer, v: i16) {
    w.write_bytes(&v.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_position_long_matches_java_literal() {
        // Literal from AxiomConstants.java (the Java static check).
        let java = 0b1000000000000000000000000010000000000000000000000000100000000000u64 as i64;
        assert_eq!(END_OF_POSITIONS, java);
    }

    #[test]
    fn pack_unpack_round_trip() {
        for &(x, y, z) in &[(0, 0, 0), (1, -1, 2), (-30_000_000, 2047, 30_000_000), (-1, -2048, -1)] {
            assert_eq!(unpack_pos(pack_pos(x, y, z)), (x, y, z));
        }
    }

    #[test]
    fn section_key_uses_floor_division() {
        assert_eq!(unpack_pos(section_key(-1, -17, 16)), (-1, -2, 1));
        assert_eq!(unpack_pos(section_key(15, 0, -16)), (0, 0, -1));
    }
}
