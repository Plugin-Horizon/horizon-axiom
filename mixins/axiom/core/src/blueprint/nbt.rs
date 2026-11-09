// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): NbtIo file roots and gzip as used by
// blueprint/BlueprintIo.java, and the typed CompoundTag getters (getIntOr and friends) it relies on.
//
// File roots are a tag type byte, a modified UTF-8 name and the payload. The payload encoding is the same as the
// network form in `buf`, so the root is read and written through `buf::Reader::read_nbt` and `Writer::write_nbt`.

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::buf::{Nbt, Reader, Writer};

use super::BlueprintError;

/// Writes a named root with an empty name (`NbtIo.write`).
pub(crate) fn write_named(tag: &Nbt) -> Result<Vec<u8>, BlueprintError> {
    let mut w = Writer::new();
    w.write_nbt(tag)?;
    let network = w.into_bytes();
    if tag.type_id() == 0 {
        return Ok(network);
    }
    // network form: type, payload. File form: type, name length (0), payload.
    let mut out = Vec::with_capacity(network.len() + 2);
    out.push(network[0]);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&network[1..]);
    Ok(out)
}

/// Reads a named root and returns it with the number of bytes it occupied. The name is ignored.
pub(crate) fn read_named(bytes: &[u8]) -> Result<(Nbt, usize), BlueprintError> {
    let Some((&type_id, rest)) = bytes.split_first() else {
        return Err(BlueprintError::NotValid);
    };
    if type_id == 0 {
        return Ok((Nbt::End, 1));
    }
    if rest.len() < 2 {
        return Err(BlueprintError::NotValid);
    }
    let name_len = u16::from_be_bytes([rest[0], rest[1]]) as usize;
    let payload_start = 3 + name_len;
    if bytes.len() < payload_start {
        return Err(BlueprintError::NotValid);
    }
    // Re-frame as a network root (type, payload) so the shared NBT reader can parse it.
    let mut framed = Vec::with_capacity(bytes.len() - payload_start + 1);
    framed.push(type_id);
    framed.extend_from_slice(&bytes[payload_start..]);
    let mut r = Reader::new(&framed);
    let tag = r.read_nbt()?;
    let consumed = payload_start + r.position() - 1;
    Ok((tag, consumed))
}

/// Gzip for the block data (`NbtIo.writeCompressed`).
pub(crate) fn gzip(data: &[u8]) -> Result<Vec<u8>, BlueprintError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

/// Gunzip when the data carries the gzip magic, otherwise the bytes unchanged (`NbtIo.readCompressed`).
pub(crate) fn gunzip(data: &[u8]) -> Result<Vec<u8>, BlueprintError> {
    if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
        let mut out = Vec::new();
        GzDecoder::new(data).read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(data.to_vec())
    }
}

/// `CompoundTag.getIntOr`: any numeric tag converts with Java's narrowing casts.
pub(crate) fn int_or(tag: &Nbt, key: &str, default: i32) -> i32 {
    match tag.get(key) {
        Some(Nbt::Byte(v)) => i32::from(*v),
        Some(Nbt::Short(v)) => i32::from(*v),
        Some(Nbt::Int(v)) => *v,
        Some(Nbt::Long(v)) => *v as i32,
        Some(Nbt::Float(v)) => *v as i32,
        Some(Nbt::Double(v)) => *v as i32,
        _ => default,
    }
}

/// `CompoundTag.getFloatOr`.
pub(crate) fn float_or(tag: &Nbt, key: &str, default: f32) -> f32 {
    match tag.get(key) {
        Some(Nbt::Byte(v)) => f32::from(*v),
        Some(Nbt::Short(v)) => f32::from(*v),
        Some(Nbt::Int(v)) => *v as f32,
        Some(Nbt::Long(v)) => *v as f32,
        Some(Nbt::Float(v)) => *v,
        Some(Nbt::Double(v)) => *v as f32,
        _ => default,
    }
}

/// `CompoundTag.getBooleanOr`: a numeric tag is true when non-zero.
pub(crate) fn bool_or(tag: &Nbt, key: &str, default: bool) -> bool {
    match tag.get(key) {
        Some(Nbt::Byte(v)) => *v != 0,
        Some(Nbt::Short(v)) => *v != 0,
        Some(Nbt::Int(v)) => *v != 0,
        Some(Nbt::Long(v)) => *v != 0,
        _ => default,
    }
}

/// `CompoundTag.getStringOr`.
pub(crate) fn string_or(tag: &Nbt, key: &str, default: &str) -> String {
    tag.get(key).and_then(Nbt::as_str).unwrap_or(default).to_owned()
}

/// `CompoundTag.getListOrEmpty`.
pub(crate) fn list_or_empty<'a>(tag: &'a Nbt, key: &str) -> &'a [Nbt] {
    tag.get(key).and_then(Nbt::as_list).unwrap_or(&[])
}
