// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): buffer/CompressedBlockEntity.java
//
// Wire layout: varint original_size, i8 compression_dict, byte array (varint length + bytes).
// Dictionary 0 is zstd with the `block_entities_v1` dictionary (assets/block_entities_v1.dict, copied unchanged from
// AxiomPaperPlugin's resources, MIT). Decoding uses ruzstd 0.9 (pure Rust) with that dictionary. Encoding uses ruzstd
// without a dictionary: its frames declare no dictionary id, which every zstd decoder accepts, Java's dictionary
// decoder included. They are a little larger than Java's output, nothing more.

use ruzstd::decoding::{Dictionary, FrameDecoder};
use ruzstd::encoding::{compress_to_vec, CompressionLevel};

use super::BufferError;
use crate::buf::{BufError, Nbt, Reader, Writer};

/// The `block_entities_v1` zstd dictionary (dictionary 0 on the wire), from AxiomPaperPlugin (MIT).
const BLOCK_ENTITY_DICTIONARY: &[u8] = include_bytes!("../../assets/block_entities_v1.dict");

/// Upper bound for a declared original size. The Java side has no fixed limit here; this keeps a hostile packet from
/// reserving gigabytes before the frame is read.
pub const MAX_ORIGINAL_SIZE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedBlockEntity {
    pub original_size: i32,
    pub compression_dict: i8,
    pub compressed: Vec<u8>,
}

impl CompressedBlockEntity {
    pub fn read(r: &mut Reader) -> Result<Self, BufferError> {
        let original_size = r.read_varint()?;
        let compression_dict = super::read_i8(r)?;
        let len = r.read_varint()?;
        let len = usize::try_from(len).map_err(|_| BufferError::NegativeValue(len))?;
        let compressed = r.read_bytes(len)?.to_vec();
        Ok(CompressedBlockEntity { original_size, compression_dict, compressed })
    }

    pub fn write(&self, w: &mut Writer) {
        w.write_varint(self.original_size);
        w.write_bytes(&[self.compression_dict as u8]);
        w.write_varint(self.compressed.len() as i32);
        w.write_bytes(&self.compressed);
    }

    /// `CompressedBlockEntity.decompress`: dictionary 0 is zstd over named NBT, decoded to an unnamed [`Nbt`] root.
    pub fn decompress(&self) -> Result<Nbt, BufferError> {
        if self.compression_dict != 0 {
            return Err(BufferError::UnknownCompressionDict(self.compression_dict));
        }
        let size = usize::try_from(self.original_size)
            .map_err(|_| BufferError::NegativeValue(self.original_size))?;
        let named = zstd_decompress(&self.compressed, size)?;
        named_to_network_nbt(&named)
    }
}

/// zstd-compresses `raw` without a dictionary. The frame declares no dictionary id and carries a checksum.
pub fn zstd_compress(raw: &[u8]) -> Vec<u8> {
    compress_to_vec(raw, CompressionLevel::Fastest)
}

/// zstd-decompresses one frame with the `block_entities_v1` dictionary. Frames without a dictionary id decode the
/// same way, as in Java's `Zstd.decompress(src, dict, originalSize)`. The output must be exactly `original_size` bytes.
pub fn zstd_decompress(compressed: &[u8], original_size: usize) -> Result<Vec<u8>, BufferError> {
    if original_size > MAX_ORIGINAL_SIZE {
        return Err(BufferError::OriginalSizeTooLarge(original_size));
    }
    let dictionary = Dictionary::decode_dict(BLOCK_ENTITY_DICTIONARY).map_err(|e| BufferError::Zstd(e.to_string()))?;
    let mut decoder = FrameDecoder::new();
    decoder.add_dict(dictionary).map_err(|e| BufferError::Zstd(e.to_string()))?;

    let mut out = Vec::with_capacity(original_size);
    decoder.decode_all_to_vec(compressed, &mut out).map_err(|e| BufferError::Zstd(e.to_string()))?;
    if out.len() != original_size {
        return Err(BufferError::DecompressedSizeMismatch { expected: original_size, found: out.len() });
    }
    Ok(out)
}

/// Named NBT (`NbtIo.write`: root type, u16 root name, payload) to the unnamed network form [`Reader::read_nbt`] reads.
fn named_to_network_nbt(named: &[u8]) -> Result<Nbt, BufferError> {
    let [tag, name_hi, name_lo, rest @ ..] = named else {
        return Err(BufError::UnexpectedEnd { needed: 3, remaining: named.len() }.into());
    };
    let name_len = u16::from_be_bytes([*name_hi, *name_lo]) as usize;
    let payload = rest
        .get(name_len..)
        .ok_or(BufError::UnexpectedEnd { needed: name_len, remaining: rest.len() })?;
    let mut network = Vec::with_capacity(payload.len() + 1);
    network.push(*tag);
    network.extend_from_slice(payload);
    Ok(Reader::new(&network).read_nbt()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain named NBT (a sign block entity), and the same bytes compressed by the real `zstd` CLI with
    /// `zstd -D assets/block_entities_v1.dict -c tests/data/block_entity_v1.nbt -o tests/data/block_entity_v1.zst`.
    const PLAIN: &[u8] = include_bytes!("../../tests/data/block_entity_v1.nbt");
    const CLI_FRAME: &[u8] = include_bytes!("../../tests/data/block_entity_v1.zst");

    #[test]
    fn wire_round_trip() {
        let be = CompressedBlockEntity {
            original_size: 300,
            compression_dict: 0,
            compressed: vec![1, 2, 3, 250],
        };
        let mut w = Writer::new();
        be.write(&mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let back = CompressedBlockEntity::read(&mut r).unwrap();
        assert_eq!(back, be);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn decodes_frame_from_zstd_cli_with_dictionary() {
        assert_eq!(zstd_decompress(CLI_FRAME, PLAIN.len()).unwrap(), PLAIN);

        let be = CompressedBlockEntity {
            original_size: PLAIN.len() as i32,
            compression_dict: 0,
            compressed: CLI_FRAME.to_vec(),
        };
        let nbt = be.decompress().unwrap();
        let Nbt::Compound(map) = &nbt else { panic!("root is not a compound") };
        assert_eq!(map.get("id").and_then(Nbt::as_str), Some("minecraft:oak_sign"));
        assert_eq!(map.get("x").and_then(Nbt::as_int), Some(-120));
        assert_eq!(map.get("y").and_then(Nbt::as_int), Some(64));
        assert_eq!(map.get("z").and_then(Nbt::as_int), Some(87));
        assert!(map.get("front_text").and_then(Nbt::as_compound).is_some());
    }

    fn frame_of_cli() -> CompressedBlockEntity {
        CompressedBlockEntity { original_size: PLAIN.len() as i32, compression_dict: 0, compressed: CLI_FRAME.to_vec() }
    }

    #[test]
    fn round_trip_compress_then_decompress() {
        let frame = zstd_compress(PLAIN);
        assert_eq!(&frame[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        assert_eq!(frame[4] & 0b11, 0, "no dictionary id in the frame header");
        assert_eq!(zstd_decompress(&frame, PLAIN.len()).unwrap(), PLAIN);

        let be = CompressedBlockEntity { original_size: PLAIN.len() as i32, compression_dict: 0, compressed: frame };
        let mut w = Writer::new();
        be.write(&mut w);
        let bytes = w.into_bytes();
        let back = CompressedBlockEntity::read(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(back, be);
        assert_eq!(back.decompress().unwrap(), frame_of_cli().decompress().unwrap());
    }

    #[test]
    fn round_trip_empty_and_large_inputs() {
        assert_eq!(zstd_decompress(&zstd_compress(&[]), 0).unwrap(), Vec::<u8>::new());
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(zstd_decompress(&zstd_compress(&big), big.len()).unwrap(), big);
    }

    #[test]
    fn wrong_original_size_is_an_error() {
        let frame = zstd_compress(PLAIN);
        assert!(matches!(
            zstd_decompress(&frame, PLAIN.len() + 1),
            Err(BufferError::DecompressedSizeMismatch { .. })
        ));
        assert!(matches!(zstd_decompress(&frame, PLAIN.len() - 1), Err(BufferError::Zstd(_))));
    }

    #[test]
    fn garbage_and_limits_are_errors() {
        assert!(matches!(zstd_decompress(b"not zstd at all", 10), Err(BufferError::Zstd(_))));
        assert!(matches!(
            zstd_decompress(CLI_FRAME, MAX_ORIGINAL_SIZE + 1),
            Err(BufferError::OriginalSizeTooLarge(_))
        ));
        let negative = CompressedBlockEntity { original_size: -1, compression_dict: 0, compressed: vec![] };
        assert!(matches!(negative.decompress(), Err(BufferError::NegativeValue(-1))));
        let other = CompressedBlockEntity { original_size: 0, compression_dict: 3, compressed: vec![] };
        assert!(matches!(other.decompress(), Err(BufferError::UnknownCompressionDict(3))));
    }
}
