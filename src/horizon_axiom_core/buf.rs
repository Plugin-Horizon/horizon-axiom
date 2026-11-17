// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): NetworkHelper.java (collection and
// BlockHitResult codecs), packet/PacketHandler.java and the FriendlyByteBuf operations used by packet/impl/*.java.
// The wire format follows vanilla Minecraft's FriendlyByteBuf: big-endian fixed-width numbers, LEB128 varints,
// length-prefixed UTF-8 strings, packed BlockPos longs and network NBT (unnamed root tag).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::Hash;

/// Vanilla `readUtf()` default limit, in UTF-16 units (Java `String.length()`).
pub const DEFAULT_STRING_MAX_CHARS: usize = 32767;
/// Vanilla NBT reader nesting limit.
pub const NBT_MAX_DEPTH: usize = 512;

/// Decoding and encoding errors. Any error means the packet is malformed and the caller should reject it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufError {
    /// Fewer bytes left than the read needs.
    UnexpectedEnd { needed: usize, remaining: usize },
    /// A varint used more than 5 bytes.
    VarIntTooBig,
    /// A varlong used more than 10 bytes.
    VarLongTooBig,
    /// A length or count was negative.
    NegativeLength(i32),
    /// A byte length exceeded the allowed maximum.
    LengthTooLarge { len: usize, max: usize },
    /// A collection count exceeded the caller's limit.
    CollectionTooLarge { count: usize, limit: usize },
    /// String bytes were not valid UTF-8.
    InvalidUtf8,
    /// String had more UTF-16 units than allowed.
    StringTooLong { chars: usize, max: usize },
    /// An enum varint was outside the known variants.
    InvalidEnumIndex { index: i32, count: usize },
    /// NBT tag id is not one of 0..=12.
    NbtUnknownTag(u8),
    /// NBT nested deeper than `NBT_MAX_DEPTH`.
    NbtDepthExceeded,
    /// NBT list with element type End but a non-zero length.
    NbtListWithoutType,
    /// NBT list whose items do not all share one type (write side).
    NbtMixedListTypes,
    /// NBT End tag inside a compound value (write side).
    NbtEndInCompound,
    /// NBT string or array does not fit its length prefix.
    NbtTooLong { what: &'static str, len: usize },
    /// NBT modified UTF-8 is malformed.
    InvalidModifiedUtf8,
}

impl fmt::Display for BufError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd { needed, remaining } => {
                write!(f, "unexpected end of buffer: needed {needed} bytes, {remaining} left")
            }
            Self::VarIntTooBig => write!(f, "VarInt too big"),
            Self::VarLongTooBig => write!(f, "VarLong too big"),
            Self::NegativeLength(n) => write!(f, "negative length {n}"),
            Self::LengthTooLarge { len, max } => write!(f, "length {len} exceeds maximum {max}"),
            Self::CollectionTooLarge { count, limit } => {
                write!(f, "collection size {count} is too large; max {limit}")
            }
            Self::InvalidUtf8 => write!(f, "invalid UTF-8"),
            Self::StringTooLong { chars, max } => write!(f, "string of {chars} chars exceeds maximum {max}"),
            Self::InvalidEnumIndex { index, count } => write!(f, "enum index {index} out of range 0..{count}"),
            Self::NbtUnknownTag(id) => write!(f, "unknown NBT tag id {id}"),
            Self::NbtDepthExceeded => write!(f, "NBT nested deeper than {NBT_MAX_DEPTH}"),
            Self::NbtListWithoutType => write!(f, "NBT list has no element type"),
            Self::NbtMixedListTypes => write!(f, "NBT list items have different types"),
            Self::NbtEndInCompound => write!(f, "NBT End tag used as a compound value"),
            Self::NbtTooLong { what, len } => write!(f, "NBT {what} of length {len} is too long"),
            Self::InvalidModifiedUtf8 => write!(f, "invalid modified UTF-8 in NBT"),
        }
    }
}

impl std::error::Error for BufError {}

// ---------------------------------------------------------------------------------------------------------------------
// Block positions and directions
// ---------------------------------------------------------------------------------------------------------------------

/// Packs a block position like vanilla `BlockPos.asLong`: x in bits 38..64 (26 bits), z in bits 12..38 (26 bits),
/// y in bits 0..12 (12 bits). Out-of-range coordinates are truncated, as in vanilla.
pub const fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

/// Inverse of `pack_block_pos`, with sign extension of each field.
pub const fn unpack_block_pos(packed: i64) -> (i32, i32, i32) {
    let x = (packed >> 38) as i32;
    let y = ((packed << 52) >> 52) as i32;
    let z = ((packed << 26) >> 38) as i32;
    (x, y, z)
}

/// Direction, in vanilla `Direction` order (3D data value 0..=5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];

    /// Vanilla `Direction.from3DDataValue`: `BY_3D_DATA[abs(data % 6)]`, so out-of-range ids wrap instead of failing.
    pub fn from_3d_data(data: i32) -> Direction {
        Self::ALL[(data % 6).unsigned_abs() as usize]
    }

    /// The 3D data value (0..=5).
    pub fn to_3d_data(self) -> i32 {
        self as i32
    }
}

/// Block hit as sent by the client (`NetworkHelper.readBlockHitResult`). Coordinates stay raw; the glue adds
/// `pos + click` to build a world position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockHitResult {
    pub pos: (i32, i32, i32),
    pub direction: Direction,
    pub click: (f32, f32, f32),
    pub inside: bool,
    pub world_border: bool,
}

// ---------------------------------------------------------------------------------------------------------------------
// Network NBT
// ---------------------------------------------------------------------------------------------------------------------

pub type NbtCompound = BTreeMap<String, Nbt>;

/// Minimal NBT tree covering all 13 tag types. `End` is only valid as a root (an empty nullable NBT) or as the
/// terminator of a compound, so it never appears as a stored value.
#[derive(Debug, Clone, PartialEq)]
pub enum Nbt {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    /// Items share one type; an empty list has no element type and encodes as type 0.
    List(Vec<Nbt>),
    Compound(NbtCompound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    /// NBT tag id (0..=12).
    pub const fn type_id(&self) -> u8 {
        match self {
            Nbt::End => 0,
            Nbt::Byte(_) => 1,
            Nbt::Short(_) => 2,
            Nbt::Int(_) => 3,
            Nbt::Long(_) => 4,
            Nbt::Float(_) => 5,
            Nbt::Double(_) => 6,
            Nbt::ByteArray(_) => 7,
            Nbt::String(_) => 8,
            Nbt::List(_) => 9,
            Nbt::Compound(_) => 10,
            Nbt::IntArray(_) => 11,
            Nbt::LongArray(_) => 12,
        }
    }

    pub fn as_compound(&self) -> Option<&NbtCompound> {
        match self {
            Nbt::Compound(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_compound_mut(&mut self) -> Option<&mut NbtCompound> {
        match self {
            Nbt::Compound(map) => Some(map),
            _ => None,
        }
    }

    /// Looks up a key of a compound; `None` for other tags or missing keys.
    pub fn get(&self, key: &str) -> Option<&Nbt> {
        self.as_compound()?.get(key)
    }

    pub fn as_list(&self) -> Option<&[Nbt]> {
        match self {
            Nbt::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Nbt::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            Nbt::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_long(&self) -> Option<i64> {
        match self {
            Nbt::Long(v) => Some(*v),
            _ => None,
        }
    }
}

/// Encodes a string as modified UTF-8 (NUL as C0 80, supplementary characters as surrogate pairs).
fn encode_mutf8(s: &str) -> Result<Vec<u8>, BufError> {
    let mut out = Vec::with_capacity(s.len());
    for unit in s.encode_utf16() {
        match unit {
            0 => out.extend_from_slice(&[0xC0, 0x80]),
            0x01..=0x7F => out.push(unit as u8),
            0x80..=0x7FF => out.extend_from_slice(&[0xC0 | (unit >> 6) as u8, 0x80 | (unit & 0x3F) as u8]),
            _ => out.extend_from_slice(&[
                0xE0 | (unit >> 12) as u8,
                0x80 | ((unit >> 6) & 0x3F) as u8,
                0x80 | (unit & 0x3F) as u8,
            ]),
        }
    }
    if out.len() > u16::MAX as usize {
        return Err(BufError::NbtTooLong { what: "string", len: out.len() });
    }
    Ok(out)
}

/// Decodes modified UTF-8. Lone surrogates cannot be represented in a Rust `String` and are rejected.
fn decode_mutf8(bytes: &[u8]) -> Result<String, BufError> {
    let cont = |i: usize| -> Result<u16, BufError> {
        bytes
            .get(i)
            .filter(|&&b| (b & 0xC0) == 0x80)
            .map(|&b| (b & 0x3F) as u16)
            .ok_or(BufError::InvalidModifiedUtf8)
    };
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i] as u16;
        if b & 0x80 == 0 {
            units.push(b);
            i += 1;
        } else if b & 0xE0 == 0xC0 {
            let c = cont(i + 1)?;
            units.push(((b & 0x1F) << 6) | c);
            i += 2;
        } else if b & 0xF0 == 0xE0 {
            let c1 = cont(i + 1)?;
            let c2 = cont(i + 2)?;
            units.push(((b & 0x0F) << 12) | (c1 << 6) | c2);
            i += 3;
        } else {
            return Err(BufError::InvalidModifiedUtf8);
        }
    }
    String::from_utf16(&units).map_err(|_| BufError::InvalidModifiedUtf8)
}

// ---------------------------------------------------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------------------------------------------------

/// Cursor over a received payload. Every read is bounds-checked and returns `BufError` instead of panicking.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    /// True when every byte was consumed. Listeners reject packets that leave bytes unread.
    pub fn is_finished(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], BufError> {
        let remaining = self.remaining();
        if n > remaining {
            return Err(BufError::UnexpectedEnd { needed: n, remaining });
        }
        let data: &'a [u8] = self.data;
        let slice = &data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], BufError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// Skips `n` bytes.
    pub fn skip(&mut self, n: usize) -> Result<(), BufError> {
        self.take(n).map(|_| ())
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>, BufError> {
        self.take(n).map(|s| s.to_vec())
    }

    pub fn read_u8(&mut self) -> Result<u8, BufError> {
        Ok(self.array::<1>()?[0])
    }

    pub fn read_i8(&mut self) -> Result<i8, BufError> {
        Ok(self.read_u8()? as i8)
    }

    /// Vanilla `readBoolean`: any non-zero byte is `true`.
    pub fn read_bool(&mut self) -> Result<bool, BufError> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_i16(&mut self) -> Result<i16, BufError> {
        Ok(i16::from_be_bytes(self.array()?))
    }

    pub fn read_i32(&mut self) -> Result<i32, BufError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    pub fn read_i64(&mut self) -> Result<i64, BufError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    pub fn read_f32(&mut self) -> Result<f32, BufError> {
        Ok(f32::from_be_bytes(self.array()?))
    }

    pub fn read_f64(&mut self) -> Result<f64, BufError> {
        Ok(f64::from_be_bytes(self.array()?))
    }

    /// Unsigned LEB128 varint, read as `i32`. Like vanilla, a fifth byte with bits above 0x0F is truncated, not rejected.
    pub fn read_varint(&mut self) -> Result<i32, BufError> {
        let mut result: u32 = 0;
        for i in 0..5 {
            let b = self.read_u8()?;
            result |= ((b & 0x7F) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(result as i32);
            }
        }
        Err(BufError::VarIntTooBig)
    }

    /// LEB128 varlong (up to 10 bytes), read as `i64`.
    pub fn read_varlong(&mut self) -> Result<i64, BufError> {
        let mut result: u64 = 0;
        for i in 0..10 {
            let b = self.read_u8()?;
            result |= ((b & 0x7F) as u64) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(result as i64);
            }
        }
        Err(BufError::VarLongTooBig)
    }

    /// Reads a varint length or count, rejecting negative values.
    fn read_length(&mut self) -> Result<usize, BufError> {
        let n = self.read_varint()?;
        usize::try_from(n).map_err(|_| BufError::NegativeLength(n))
    }

    /// Varint-prefixed UTF-8 string. Like vanilla `readUtf(max)`: at most `max * 3` bytes and `max` UTF-16 units.
    pub fn read_string(&mut self, max_chars: usize) -> Result<String, BufError> {
        let len = self.read_length()?;
        let max_bytes = max_chars.saturating_mul(3);
        if len > max_bytes {
            return Err(BufError::LengthTooLarge { len, max: max_bytes });
        }
        let s = std::str::from_utf8(self.take(len)?).map_err(|_| BufError::InvalidUtf8)?;
        let chars = s.encode_utf16().count();
        if chars > max_chars {
            return Err(BufError::StringTooLong { chars, max: max_chars });
        }
        Ok(s.to_owned())
    }

    /// Identifier as sent on the wire (`readIdentifier`, a string with the default limit). Not parsed.
    pub fn read_identifier(&mut self) -> Result<String, BufError> {
        self.read_string(DEFAULT_STRING_MAX_CHARS)
    }

    /// Varint-prefixed byte array with an explicit maximum (`readByteArray(max)`).
    pub fn read_byte_array(&mut self, max: usize) -> Result<Vec<u8>, BufError> {
        let len = self.read_length()?;
        if len > max {
            return Err(BufError::LengthTooLarge { len, max });
        }
        self.read_bytes(len)
    }

    /// Packed BlockPos as `(x, y, z)`.
    pub fn read_block_pos(&mut self) -> Result<(i32, i32, i32), BufError> {
        Ok(unpack_block_pos(self.read_i64()?))
    }

    /// UUID as a `u128` (most significant long first, like vanilla `readUUID`).
    pub fn read_uuid(&mut self) -> Result<u128, BufError> {
        let msb = self.read_i64()? as u64;
        let lsb = self.read_i64()? as u64;
        Ok(((msb as u128) << 64) | lsb as u128)
    }

    /// Enum written as its varint ordinal; `values` lists the variants in ordinal order.
    pub fn read_enum<T: Copy>(&mut self, values: &[T]) -> Result<T, BufError> {
        let index = self.read_varint()?;
        usize::try_from(index)
            .ok()
            .and_then(|i| values.get(i))
            .copied()
            .ok_or(BufError::InvalidEnumIndex { index, count: values.len() })
    }

    /// Bool-prefixed optional value (`readNullable`).
    pub fn read_nullable<T>(
        &mut self,
        f: impl FnOnce(&mut Reader<'a>) -> Result<T, BufError>,
    ) -> Result<Option<T>, BufError> {
        if self.read_bool()? {
            f(self).map(Some)
        } else {
            Ok(None)
        }
    }

    fn read_count(&mut self, limit: usize) -> Result<usize, BufError> {
        let count = self.read_length()?;
        if count > limit {
            return Err(BufError::CollectionTooLarge { count, limit });
        }
        Ok(count)
    }

    /// Varint count, then `count` items (`NetworkHelper.readList`).
    pub fn read_list<T>(
        &mut self,
        limit: usize,
        mut f: impl FnMut(&mut Reader<'a>) -> Result<T, BufError>,
    ) -> Result<Vec<T>, BufError> {
        let count = self.read_count(limit)?;
        let mut out = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            out.push(f(self)?);
        }
        Ok(out)
    }

    /// Like `LinkedHashMap`: entries in first-seen order, a repeated key keeps its position and takes the last value.
    pub fn read_linked_hash_map<K, V>(
        &mut self,
        limit: usize,
        mut fk: impl FnMut(&mut Reader<'a>) -> Result<K, BufError>,
        mut fv: impl FnMut(&mut Reader<'a>) -> Result<V, BufError>,
    ) -> Result<Vec<(K, V)>, BufError>
    where
        K: Eq + Hash + Clone,
    {
        let count = self.read_count(limit)?;
        let mut entries: Vec<(K, V)> = Vec::with_capacity(count.min(1024));
        let mut index: HashMap<K, usize> = HashMap::with_capacity(count.min(1024));
        for _ in 0..count {
            let key = fk(self)?;
            let value = fv(self)?;
            match index.get(&key) {
                Some(&i) => entries[i].1 = value,
                None => {
                    index.insert(key.clone(), entries.len());
                    entries.push((key, value));
                }
            }
        }
        Ok(entries)
    }

    /// Like `LinkedHashSet`: first occurrence wins, order kept.
    pub fn read_linked_hash_set<T>(
        &mut self,
        limit: usize,
        mut f: impl FnMut(&mut Reader<'a>) -> Result<T, BufError>,
    ) -> Result<Vec<T>, BufError>
    where
        T: Eq + Hash + Clone,
    {
        let count = self.read_count(limit)?;
        let mut entries: Vec<T> = Vec::with_capacity(count.min(1024));
        let mut seen: HashSet<T> = HashSet::with_capacity(count.min(1024));
        for _ in 0..count {
            let item = f(self)?;
            if seen.insert(item.clone()) {
                entries.push(item);
            }
        }
        Ok(entries)
    }

    /// `BlockHitResult` as sent by the client.
    pub fn read_block_hit_result(&mut self) -> Result<BlockHitResult, BufError> {
        let pos = self.read_block_pos()?;
        let direction = Direction::from_3d_data(self.read_i8()? as i32);
        let click = (self.read_f32()?, self.read_f32()?, self.read_f32()?);
        let inside = self.read_bool()?;
        let world_border = self.read_bool()?;
        Ok(BlockHitResult { pos, direction, click, inside, world_border })
    }

    /// Reads an unnamed network NBT root (1.20.2+). A leading End tag yields `Nbt::End`.
    pub fn read_nbt(&mut self) -> Result<Nbt, BufError> {
        let tag_type = self.read_u8()?;
        self.read_nbt_payload(tag_type, 0)
    }

    /// Element count of an array or list. Each element takes at least `min_elem_size` bytes, so a count that cannot
    /// fit in the rest of the buffer is rejected before anything is allocated.
    fn read_nbt_count(&mut self, min_elem_size: usize) -> Result<usize, BufError> {
        let len = self.read_nbt_len()?;
        self.check_nbt_count(len, min_elem_size)?;
        Ok(len)
    }

    /// NBT length prefixes are big-endian i32, unlike the varints used on the packet level.
    fn read_nbt_len(&mut self) -> Result<usize, BufError> {
        let n = self.read_i32()?;
        usize::try_from(n).map_err(|_| BufError::NegativeLength(n))
    }

    fn check_nbt_count(&self, len: usize, min_elem_size: usize) -> Result<(), BufError> {
        let needed = len.saturating_mul(min_elem_size);
        if needed > self.remaining() {
            return Err(BufError::UnexpectedEnd { needed, remaining: self.remaining() });
        }
        Ok(())
    }

    fn read_nbt_string(&mut self) -> Result<String, BufError> {
        let len = self.read_u16_be()? as usize;
        decode_mutf8(self.take(len)?)
    }

    fn read_u16_be(&mut self) -> Result<u16, BufError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn read_nbt_payload(&mut self, tag_type: u8, depth: usize) -> Result<Nbt, BufError> {
        if depth > NBT_MAX_DEPTH {
            return Err(BufError::NbtDepthExceeded);
        }
        Ok(match tag_type {
            0 => Nbt::End,
            1 => Nbt::Byte(self.read_i8()?),
            2 => Nbt::Short(self.read_i16()?),
            3 => Nbt::Int(self.read_i32()?),
            4 => Nbt::Long(self.read_i64()?),
            5 => Nbt::Float(self.read_f32()?),
            6 => Nbt::Double(self.read_f64()?),
            7 => {
                let len = self.read_nbt_count(1)?;
                Nbt::ByteArray(self.take(len)?.iter().map(|&b| b as i8).collect())
            }
            8 => Nbt::String(self.read_nbt_string()?),
            9 => {
                let elem_type = self.read_u8()?;
                if elem_type > 12 {
                    return Err(BufError::NbtUnknownTag(elem_type));
                }
                let len = self.read_nbt_len()?;
                if elem_type == 0 && len > 0 {
                    return Err(BufError::NbtListWithoutType);
                }
                self.check_nbt_count(len, 1)?;
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.read_nbt_payload(elem_type, depth + 1)?);
                }
                Nbt::List(items)
            }
            10 => {
                let mut map = NbtCompound::new();
                loop {
                    let child_type = self.read_u8()?;
                    if child_type == 0 {
                        break;
                    }
                    let name = self.read_nbt_string()?;
                    let value = self.read_nbt_payload(child_type, depth + 1)?;
                    map.insert(name, value);
                }
                Nbt::Compound(map)
            }
            11 => {
                let len = self.read_nbt_count(4)?;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    out.push(self.read_i32()?);
                }
                Nbt::IntArray(out)
            }
            12 => {
                let len = self.read_nbt_count(8)?;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    out.push(self.read_i64()?);
                }
                Nbt::LongArray(out)
            }
            other => return Err(BufError::NbtUnknownTag(other)),
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------------------------------------------------

/// Builds a payload. Primitive writes cannot fail; NBT and nested callbacks can, and report `BufError`.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { buf: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_i8(&mut self, v: i8) {
        self.buf.push(v as u8);
    }

    pub fn write_bool(&mut self, v: bool) {
        self.buf.push(v as u8);
    }

    pub fn write_i16(&mut self, v: i16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Raw bytes, no prefix.
    pub fn write_bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    pub fn write_varint(&mut self, value: i32) {
        let mut v = value as u32;
        loop {
            if v & !0x7F == 0 {
                self.buf.push(v as u8);
                return;
            }
            self.buf.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }

    pub fn write_varlong(&mut self, value: i64) {
        let mut v = value as u64;
        loop {
            if v & !0x7F == 0 {
                self.buf.push(v as u8);
                return;
            }
            self.buf.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }

    /// Varint byte length, then UTF-8 bytes (no limit check; the caller owns the content).
    pub fn write_string(&mut self, s: &str) {
        self.write_varint(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    /// Same wire form as `write_string`.
    pub fn write_identifier(&mut self, id: &str) {
        self.write_string(id);
    }

    /// Varint length, then bytes.
    pub fn write_byte_array(&mut self, v: &[u8]) {
        self.write_varint(v.len() as i32);
        self.buf.extend_from_slice(v);
    }

    pub fn write_block_pos(&mut self, pos: (i32, i32, i32)) {
        self.write_i64(pack_block_pos(pos.0, pos.1, pos.2));
    }

    pub fn write_uuid(&mut self, uuid: u128) {
        self.write_i64((uuid >> 64) as u64 as i64);
        self.write_i64(uuid as u64 as i64);
    }

    /// Enum written as its ordinal varint.
    pub fn write_enum_index(&mut self, ordinal: usize) {
        self.write_varint(ordinal as i32);
    }

    pub fn write_block_hit_result(&mut self, hit: &BlockHitResult) {
        self.write_block_pos(hit.pos);
        self.write_i8(hit.direction.to_3d_data() as i8);
        self.write_f32(hit.click.0);
        self.write_f32(hit.click.1);
        self.write_f32(hit.click.2);
        self.write_bool(hit.inside);
        self.write_bool(hit.world_border);
    }

    /// Bool prefix, then the value if present.
    pub fn write_nullable<T>(
        &mut self,
        value: Option<&T>,
        f: impl FnOnce(&mut Writer, &T) -> Result<(), BufError>,
    ) -> Result<(), BufError> {
        match value {
            Some(v) => {
                self.write_bool(true);
                f(self, v)
            }
            None => {
                self.write_bool(false);
                Ok(())
            }
        }
    }

    /// Varint count, then each item (`NetworkHelper.writeCollection`).
    pub fn write_collection<T>(
        &mut self,
        items: &[T],
        mut f: impl FnMut(&mut Writer, &T) -> Result<(), BufError>,
    ) -> Result<(), BufError> {
        self.write_varint(items.len() as i32);
        for item in items {
            f(self, item)?;
        }
        Ok(())
    }

    /// Varint count, then key and value pairs in order (`NetworkHelper.writeMap`).
    pub fn write_map<K, V>(
        &mut self,
        entries: &[(K, V)],
        mut fk: impl FnMut(&mut Writer, &K) -> Result<(), BufError>,
        mut fv: impl FnMut(&mut Writer, &V) -> Result<(), BufError>,
    ) -> Result<(), BufError> {
        self.write_varint(entries.len() as i32);
        for (k, v) in entries {
            fk(self, k)?;
            fv(self, v)?;
        }
        Ok(())
    }

    fn write_nbt_len(&mut self, len: usize, what: &'static str) -> Result<(), BufError> {
        let n = i32::try_from(len).map_err(|_| BufError::NbtTooLong { what, len })?;
        self.write_i32(n);
        Ok(())
    }

    /// Writes an unnamed network NBT root (1.20.2+): the type byte, then the payload with no name.
    pub fn write_nbt(&mut self, nbt: &Nbt) -> Result<(), BufError> {
        self.write_u8(nbt.type_id());
        self.write_nbt_payload(nbt, 0)
    }

    fn write_nbt_payload(&mut self, nbt: &Nbt, depth: usize) -> Result<(), BufError> {
        if depth > NBT_MAX_DEPTH {
            return Err(BufError::NbtDepthExceeded);
        }
        match nbt {
            Nbt::End => {}
            Nbt::Byte(v) => self.write_i8(*v),
            Nbt::Short(v) => self.write_i16(*v),
            Nbt::Int(v) => self.write_i32(*v),
            Nbt::Long(v) => self.write_i64(*v),
            Nbt::Float(v) => self.write_f32(*v),
            Nbt::Double(v) => self.write_f64(*v),
            Nbt::ByteArray(v) => {
                self.write_nbt_len(v.len(), "byte array")?;
                for &b in v {
                    self.write_i8(b);
                }
            }
            Nbt::String(s) => self.write_nbt_string(s)?,
            Nbt::List(items) => {
                let elem_type = items.first().map_or(0, Nbt::type_id);
                if items.iter().any(|item| item.type_id() != elem_type) {
                    return Err(BufError::NbtMixedListTypes);
                }
                if elem_type == 0 && !items.is_empty() {
                    return Err(BufError::NbtListWithoutType);
                }
                self.write_u8(elem_type);
                self.write_nbt_len(items.len(), "list")?;
                for item in items {
                    self.write_nbt_payload(item, depth + 1)?;
                }
            }
            Nbt::Compound(map) => {
                for (name, value) in map {
                    if value.type_id() == 0 {
                        return Err(BufError::NbtEndInCompound);
                    }
                    self.write_u8(value.type_id());
                    self.write_nbt_string(name)?;
                    self.write_nbt_payload(value, depth + 1)?;
                }
                self.write_u8(0);
            }
            Nbt::IntArray(v) => {
                self.write_nbt_len(v.len(), "int array")?;
                for &x in v {
                    self.write_i32(x);
                }
            }
            Nbt::LongArray(v) => {
                self.write_nbt_len(v.len(), "long array")?;
                for &x in v {
                    self.write_i64(x);
                }
            }
        }
        Ok(())
    }

    fn write_nbt_string(&mut self, s: &str) -> Result<(), BufError> {
        let encoded = encode_mutf8(s)?;
        self.buf.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(&encoded);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_varint(v: i32) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_varint(v);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_varint().unwrap(), v);
        assert!(r.is_finished());
        bytes
    }

    #[test]
    fn varint_wire_vectors() {
        assert_eq!(roundtrip_varint(0), vec![0x00]);
        assert_eq!(roundtrip_varint(1), vec![0x01]);
        assert_eq!(roundtrip_varint(127), vec![0x7F]);
        assert_eq!(roundtrip_varint(128), vec![0x80, 0x01]);
        assert_eq!(roundtrip_varint(255), vec![0xFF, 0x01]);
        assert_eq!(roundtrip_varint(25565), vec![0xDD, 0xC7, 0x01]);
        assert_eq!(roundtrip_varint(2097151), vec![0xFF, 0xFF, 0x7F]);
        assert_eq!(roundtrip_varint(i32::MAX), vec![0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
        assert_eq!(roundtrip_varint(-1), vec![0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        assert_eq!(roundtrip_varint(i32::MIN), vec![0x80, 0x80, 0x80, 0x80, 0x08]);
    }

    #[test]
    fn varint_too_big_and_truncated() {
        // Five continuation bytes: the continuation bit is still set after the fifth byte.
        let mut r = Reader::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert_eq!(r.read_varint(), Err(BufError::VarIntTooBig));
        let mut r = Reader::new(&[0x80, 0x80]);
        assert!(matches!(r.read_varint(), Err(BufError::UnexpectedEnd { .. })));
    }

    #[test]
    fn varint_fifth_byte_high_bits_are_truncated_like_vanilla() {
        // Vanilla keeps the low 32 bits and ignores the rest of the fifth byte.
        let mut r = Reader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
        assert_eq!(r.read_varint().unwrap(), -1);
    }

    #[test]
    fn varlong_wire_vectors_and_roundtrip() {
        for v in [0i64, 1, 127, 128, 300, i32::MAX as i64, i64::MAX, -1, i64::MIN] {
            let mut w = Writer::new();
            w.write_varlong(v);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            assert_eq!(r.read_varlong().unwrap(), v, "value {v}");
            assert!(r.is_finished());
        }
        let mut w = Writer::new();
        w.write_varlong(-1);
        assert_eq!(w.as_bytes(), &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
        let mut w = Writer::new();
        w.write_varlong(i64::MIN);
        assert_eq!(w.as_bytes(), &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        let mut r = Reader::new(&[0x80; 10]);
        assert_eq!(r.read_varlong(), Err(BufError::VarLongTooBig));
    }

    #[test]
    fn fixed_width_numbers_are_big_endian() {
        let mut w = Writer::new();
        w.write_i32(0x0102_0304);
        w.write_i16(-2);
        w.write_i64(-1);
        w.write_f32(1.5);
        w.write_f64(-0.25);
        w.write_bool(true);
        w.write_bool(false);
        w.write_i8(-128);
        let bytes = w.into_bytes();
        assert_eq!(&bytes[..4], &[1, 2, 3, 4]);
        assert_eq!(&bytes[4..6], &[0xFF, 0xFE]);
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_i32().unwrap(), 0x0102_0304);
        assert_eq!(r.read_i16().unwrap(), -2);
        assert_eq!(r.read_i64().unwrap(), -1);
        assert_eq!(r.read_f32().unwrap(), 1.5);
        assert_eq!(r.read_f64().unwrap(), -0.25);
        assert!(r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        assert_eq!(r.read_i8().unwrap(), -128);
        assert!(r.is_finished());
    }

    #[test]
    fn bool_is_any_nonzero_byte() {
        let mut r = Reader::new(&[0, 1, 2, 0xFF]);
        assert!(!r.read_bool().unwrap());
        assert!(r.read_bool().unwrap());
        assert!(r.read_bool().unwrap());
        assert!(r.read_bool().unwrap());
    }

    #[test]
    fn unexpected_end_reports_counts() {
        let mut r = Reader::new(&[1, 2]);
        assert_eq!(r.read_i32(), Err(BufError::UnexpectedEnd { needed: 4, remaining: 2 }));
        // A failed read does not move the cursor.
        assert_eq!(r.remaining(), 2);
    }

    #[test]
    fn floats_keep_nan_and_infinity() {
        let mut w = Writer::new();
        w.write_f64(f64::INFINITY);
        w.write_f32(f32::NAN);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_f64().unwrap(), f64::INFINITY);
        assert!(r.read_f32().unwrap().is_nan());
    }

    #[test]
    fn string_roundtrip_ascii_multibyte_and_emoji() {
        for s in ["", "axiom", "Grüße ✓", "emoji \u{1F600} end"] {
            let mut w = Writer::new();
            w.write_string(s);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            assert_eq!(r.read_string(DEFAULT_STRING_MAX_CHARS).unwrap(), s);
            assert!(r.is_finished());
        }
    }

    #[test]
    fn string_prefix_is_utf8_byte_count() {
        let mut w = Writer::new();
        w.write_string("\u{1F600}"); // 4 UTF-8 bytes, 2 UTF-16 units
        assert_eq!(w.as_bytes(), &[4, 0xF0, 0x9F, 0x98, 0x80]);
    }

    #[test]
    fn string_limit_counts_utf16_units() {
        let mut w = Writer::new();
        w.write_string("\u{1F600}\u{1F600}"); // 2 emoji = 4 UTF-16 units
        let bytes = w.into_bytes();
        assert_eq!(
            Reader::new(&bytes).read_string(3),
            Err(BufError::StringTooLong { chars: 4, max: 3 })
        );
        assert_eq!(Reader::new(&bytes).read_string(4).unwrap(), "\u{1F600}\u{1F600}");
    }

    #[test]
    fn string_byte_limit_is_three_times_max() {
        // max 2 chars allows at most 6 bytes; 7 bytes must fail before the content is read.
        let mut bytes = vec![7u8];
        bytes.extend_from_slice(b"abcdefg");
        assert_eq!(
            Reader::new(&bytes).read_string(2),
            Err(BufError::LengthTooLarge { len: 7, max: 6 })
        );
    }

    #[test]
    fn string_rejects_negative_length_and_bad_utf8() {
        let mut w = Writer::new();
        w.write_varint(-1);
        assert_eq!(Reader::new(w.as_bytes()).read_string(10), Err(BufError::NegativeLength(-1)));
        assert_eq!(Reader::new(&[2, 0xFF, 0xFE]).read_string(10), Err(BufError::InvalidUtf8));
    }

    #[test]
    fn byte_array_roundtrip_and_limit() {
        let mut w = Writer::new();
        w.write_byte_array(&[9, 8, 7]);
        let bytes = w.into_bytes();
        assert_eq!(Reader::new(&bytes).read_byte_array(3).unwrap(), vec![9, 8, 7]);
        assert_eq!(
            Reader::new(&bytes).read_byte_array(2),
            Err(BufError::LengthTooLarge { len: 3, max: 2 })
        );
        assert_eq!(
            Reader::new(&[0x7F]).read_byte_array(10),
            Err(BufError::LengthTooLarge { len: 127, max: 10 })
        );
        assert_eq!(
            Reader::new(&[0x7F]).read_byte_array(200),
            Err(BufError::UnexpectedEnd { needed: 127, remaining: 0 })
        );
    }

    #[test]
    fn block_pos_layout_matches_vanilla() {
        assert_eq!(pack_block_pos(0, 0, 0), 0);
        assert_eq!(pack_block_pos(1, 0, 0), 1 << 38);
        assert_eq!(pack_block_pos(0, 0, 1), 1 << 12);
        assert_eq!(pack_block_pos(0, 1, 0), 1);
        assert_eq!(pack_block_pos(-1, -1, -1), -1);
    }

    #[test]
    fn block_pos_min_constant_matches_java_check() {
        // AxiomConstants.MIN_POSITION_LONG: x and z bit 25 (sign of the 26-bit fields), y bit 11 (sign of the 12-bit field).
        let expected = (1i64 << 63) | (1i64 << 37) | (1i64 << 11);
        assert_eq!(pack_block_pos(-33_554_432, -2048, -33_554_432), expected);
        assert_eq!(unpack_block_pos(expected), (-33_554_432, -2048, -33_554_432));
    }

    #[test]
    fn block_pos_negative_roundtrip_and_extremes() {
        let cases = [
            (0, 0, 0),
            (-1, 0, 0),
            (0, -1, 0),
            (0, 0, -1),
            (-30_000_000, -64, 30_000_000),
            (33_554_431, 2047, -33_554_432),
            (-33_554_432, -2048, 33_554_431),
            (12, -63, -700),
        ];
        for (x, y, z) in cases {
            let packed = pack_block_pos(x, y, z);
            assert_eq!(unpack_block_pos(packed), (x, y, z), "({x}, {y}, {z})");
            let mut w = Writer::new();
            w.write_block_pos((x, y, z));
            let bytes = w.into_bytes();
            assert_eq!(bytes.len(), 8);
            assert_eq!(&bytes[..], &packed.to_be_bytes()[..]);
            assert_eq!(Reader::new(&bytes).read_block_pos().unwrap(), (x, y, z));
        }
    }

    #[test]
    fn uuid_is_msb_then_lsb() {
        let uuid: u128 = 0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210;
        let mut w = Writer::new();
        w.write_uuid(uuid);
        let bytes = w.into_bytes();
        assert_eq!(&bytes[..8], &0x0123_4567_89AB_CDEFu64.to_be_bytes());
        assert_eq!(&bytes[8..], &0xFEDC_BA98_7654_3210u64.to_be_bytes());
        assert_eq!(Reader::new(&bytes).read_uuid().unwrap(), uuid);
        let mut w = Writer::new();
        w.write_uuid(u128::MAX);
        assert_eq!(Reader::new(w.as_bytes()).read_uuid().unwrap(), u128::MAX);
    }

    #[test]
    fn enum_by_ordinal() {
        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Hand {
            Main,
            Off,
        }
        let values = [Hand::Main, Hand::Off];
        let mut r = Reader::new(&[1, 0, 2]);
        assert_eq!(r.read_enum(&values).unwrap(), Hand::Off);
        assert_eq!(r.read_enum(&values).unwrap(), Hand::Main);
        assert_eq!(r.read_enum(&values), Err(BufError::InvalidEnumIndex { index: 2, count: 2 }));
        let mut w = Writer::new();
        w.write_enum_index(1);
        assert_eq!(w.as_bytes(), &[1]);
        assert_eq!(
            Reader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).read_enum(&values),
            Err(BufError::InvalidEnumIndex { index: -1, count: 2 })
        );
    }

    #[test]
    fn nullable_roundtrip() {
        let mut w = Writer::new();
        w.write_nullable(Some(&42i32), |w, v| {
            w.write_i32(*v);
            Ok(())
        })
        .unwrap();
        w.write_nullable::<i32>(None, |_, _| Ok(())).unwrap();
        let bytes = w.into_bytes();
        assert_eq!(bytes, vec![1, 0, 0, 0, 42, 0]);
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_nullable(|r| r.read_i32()).unwrap(), Some(42));
        assert_eq!(r.read_nullable(|r| r.read_i32()).unwrap(), None);
    }

    #[test]
    fn list_roundtrip_and_limit() {
        let mut w = Writer::new();
        w.write_collection(&[1i32, 2, 3], |w, v| {
            w.write_i32(*v);
            Ok(())
        })
        .unwrap();
        let bytes = w.into_bytes();
        assert_eq!(bytes[0], 3);
        let list = Reader::new(&bytes).read_list(3, |r| r.read_i32()).unwrap();
        assert_eq!(list, vec![1, 2, 3]);
        assert_eq!(
            Reader::new(&bytes).read_list(2, |r| r.read_i32()),
            Err(BufError::CollectionTooLarge { count: 3, limit: 2 })
        );
    }

    #[test]
    fn negative_collection_count_is_rejected() {
        // Java's readList silently returns an empty list for a negative count; the port rejects it instead.
        let mut w = Writer::new();
        w.write_varint(-5);
        assert_eq!(
            Reader::new(w.as_bytes()).read_list(10, |r| r.read_u8()),
            Err(BufError::NegativeLength(-5))
        );
    }

    #[test]
    fn linked_hash_map_keeps_first_position_and_last_value() {
        let mut w = Writer::new();
        w.write_map(&[((1, 2, 3), 10i32), ((4, 5, 6), 20), ((1, 2, 3), 30)], |w, k| {
            w.write_block_pos(*k);
            Ok(())
        }, |w, v| {
            w.write_i32(*v);
            Ok(())
        })
        .unwrap();
        let bytes = w.into_bytes();
        let map = Reader::new(&bytes)
            .read_linked_hash_map(10, |r| r.read_block_pos(), |r| r.read_i32())
            .unwrap();
        assert_eq!(map, vec![((1, 2, 3), 30), ((4, 5, 6), 20)]);
    }

    #[test]
    fn linked_hash_set_keeps_first_occurrence() {
        let mut w = Writer::new();
        w.write_collection(&[(0, 0, 0), (1, 1, 1), (0, 0, 0)], |w, p| {
            w.write_block_pos(*p);
            Ok(())
        })
        .unwrap();
        let bytes = w.into_bytes();
        let set = Reader::new(&bytes).read_linked_hash_set(10, |r| r.read_block_pos()).unwrap();
        assert_eq!(set, vec![(0, 0, 0), (1, 1, 1)]);
    }

    #[test]
    fn block_hit_result_roundtrip_and_direction_wrap() {
        let hit = BlockHitResult {
            pos: (-5, 64, 12),
            direction: Direction::East,
            click: (0.5, 1.0, 0.25),
            inside: true,
            world_border: false,
        };
        let mut w = Writer::new();
        w.write_block_hit_result(&hit);
        let bytes = w.into_bytes();
        assert_eq!(Reader::new(&bytes).read_block_hit_result().unwrap(), hit);

        assert_eq!(Direction::from_3d_data(6), Direction::Down);
        assert_eq!(Direction::from_3d_data(-1), Direction::Up);
        assert_eq!(Direction::from_3d_data(5), Direction::East);
        for d in Direction::ALL {
            assert_eq!(Direction::from_3d_data(d.to_3d_data()), d);
        }
    }

    #[test]
    fn mutf8_nul_and_supplementary_characters() {
        // NUL is encoded as C0 80 in modified UTF-8.
        assert_eq!(encode_mutf8("\0").unwrap(), vec![0xC0, 0x80]);
        assert_eq!(decode_mutf8(&[0xC0, 0x80]).unwrap(), "\0");
        // U+1F600 is a surrogate pair, 3 bytes each.
        let encoded = encode_mutf8("\u{1F600}").unwrap();
        assert_eq!(encoded, vec![0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);
        assert_eq!(decode_mutf8(&encoded).unwrap(), "\u{1F600}");
        assert_eq!(decode_mutf8(&encode_mutf8("Grüße ✓").unwrap()).unwrap(), "Grüße ✓");
        // A lone high surrogate cannot be represented in a Rust String.
        assert_eq!(decode_mutf8(&[0xED, 0xA0, 0xBD]), Err(BufError::InvalidModifiedUtf8));
        assert_eq!(decode_mutf8(&[0xFF]), Err(BufError::InvalidModifiedUtf8));
    }

    /// Builds a compound with one entry per NBT type.
    fn all_types_compound() -> Nbt {
        let mut map = NbtCompound::new();
        map.insert("byte".into(), Nbt::Byte(-7));
        map.insert("short".into(), Nbt::Short(-300));
        map.insert("int".into(), Nbt::Int(123_456));
        map.insert("long".into(), Nbt::Long(-9_000_000_000));
        map.insert("float".into(), Nbt::Float(2.5));
        map.insert("double".into(), Nbt::Double(-1.125));
        map.insert("bytes".into(), Nbt::ByteArray(vec![-1, 0, 127]));
        map.insert("string".into(), Nbt::String("h\u{e9}llo \u{1F600}\0".into()));
        map.insert("list".into(), Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)]));
        map.insert("empty_list".into(), Nbt::List(vec![]));
        map.insert(
            "compound".into(),
            Nbt::Compound(NbtCompound::from([("inner".to_string(), Nbt::Long(1))])),
        );
        map.insert("ints".into(), Nbt::IntArray(vec![i32::MIN, 0, i32::MAX]));
        map.insert("longs".into(), Nbt::LongArray(vec![i64::MIN, 5]));
        Nbt::Compound(map)
    }

    #[test]
    fn nbt_all_types_roundtrip() {
        let root = all_types_compound();
        let mut w = Writer::new();
        w.write_nbt(&root).unwrap();
        let bytes = w.into_bytes();
        assert_eq!(bytes[0], 10, "unnamed root starts with the compound id");
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_nbt().unwrap(), root);
        assert!(r.is_finished());
    }

    #[test]
    fn nbt_all_thirteen_type_ids_are_distinct() {
        let samples = [
            Nbt::End,
            Nbt::Byte(0),
            Nbt::Short(0),
            Nbt::Int(0),
            Nbt::Long(0),
            Nbt::Float(0.0),
            Nbt::Double(0.0),
            Nbt::ByteArray(vec![]),
            Nbt::String(String::new()),
            Nbt::List(vec![]),
            Nbt::Compound(NbtCompound::new()),
            Nbt::IntArray(vec![]),
            Nbt::LongArray(vec![]),
        ];
        for (i, nbt) in samples.iter().enumerate() {
            assert_eq!(nbt.type_id() as usize, i);
        }
    }

    #[test]
    fn nbt_root_wire_bytes_are_unnamed() {
        let mut map = NbtCompound::new();
        map.insert("name".into(), Nbt::String("bob".into()));
        let mut w = Writer::new();
        w.write_nbt(&Nbt::Compound(map)).unwrap();
        assert_eq!(
            w.as_bytes(),
            &[0x0A, 0x08, 0x00, 0x04, b'n', b'a', b'm', b'e', 0x00, 0x03, b'b', b'o', b'b', 0x00]
        );
    }

    #[test]
    fn nbt_list_wire_bytes() {
        let mut map = NbtCompound::new();
        map.insert("l".into(), Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)]));
        let mut w = Writer::new();
        w.write_nbt(&Nbt::Compound(map)).unwrap();
        assert_eq!(
            w.as_bytes(),
            &[
                0x0A, 0x09, 0x00, 0x01, b'l', 0x03, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
                0x00, 0x02, 0x00
            ]
        );
    }

    #[test]
    fn nbt_null_root_is_end() {
        let mut w = Writer::new();
        w.write_nbt(&Nbt::End).unwrap();
        assert_eq!(w.as_bytes(), &[0]);
        assert_eq!(Reader::new(&[0]).read_nbt().unwrap(), Nbt::End);
    }

    #[test]
    fn nbt_accessors() {
        let mut root = all_types_compound();
        assert_eq!(root.get("int").and_then(Nbt::as_int), Some(123_456));
        assert_eq!(root.get("string").and_then(Nbt::as_str), Some("h\u{e9}llo \u{1F600}\0"));
        assert_eq!(root.get("long").and_then(Nbt::as_long), Some(-9_000_000_000));
        assert_eq!(root.get("list").and_then(Nbt::as_list).map(|l| l.len()), Some(2));
        assert!(root.get("missing").is_none());
        assert!(Nbt::Int(1).get("x").is_none());
        root.as_compound_mut().unwrap().remove("int");
        assert!(root.get("int").is_none());
    }

    #[test]
    fn nbt_rejects_unknown_tag_and_truncation() {
        assert_eq!(Reader::new(&[13]).read_nbt(), Err(BufError::NbtUnknownTag(13)));
        // Compound with one Int entry that is cut off.
        let bytes = [10u8, 3, 0, 1, b'a', 0, 0];
        assert!(matches!(Reader::new(&bytes).read_nbt(), Err(BufError::UnexpectedEnd { .. })));
    }

    #[test]
    fn nbt_list_with_end_type_and_items_is_rejected() {
        // Root list, element type End, length 1.
        let bytes = [9u8, 0, 0, 0, 0, 1];
        assert_eq!(Reader::new(&bytes).read_nbt(), Err(BufError::NbtListWithoutType));
        // Root list, element type End, length 0 is fine.
        let bytes = [9u8, 0, 0, 0, 0, 0];
        assert_eq!(Reader::new(&bytes).read_nbt().unwrap(), Nbt::List(vec![]));
    }

    #[test]
    fn nbt_array_count_cannot_outrun_the_buffer() {
        // Int array claiming 1_000_000 elements in 4 bytes of input.
        let bytes = [11u8, 0x00, 0x0F, 0x42, 0x40];
        assert!(matches!(Reader::new(&bytes).read_nbt(), Err(BufError::UnexpectedEnd { .. })));
        let bytes = [7u8, 0xFF, 0xFF, 0xFF, 0xFF];
        assert_eq!(Reader::new(&bytes).read_nbt(), Err(BufError::NegativeLength(-1)));
    }

    fn nested_list_bytes(levels: usize) -> Vec<u8> {
        // Root list, then `levels` nested lists with one item each, ending in an empty list at depth `levels`.
        let mut bytes = vec![9u8];
        for _ in 0..levels {
            bytes.extend_from_slice(&[9, 0, 0, 0, 1]);
        }
        bytes.extend_from_slice(&[0, 0, 0, 0, 0]);
        bytes
    }

    #[test]
    fn nbt_depth_limit_on_read() {
        assert!(Reader::new(&nested_list_bytes(NBT_MAX_DEPTH)).read_nbt().is_ok());
        assert_eq!(
            Reader::new(&nested_list_bytes(NBT_MAX_DEPTH + 1)).read_nbt(),
            Err(BufError::NbtDepthExceeded)
        );
    }

    #[test]
    fn nbt_depth_limit_on_write() {
        let mut nbt = Nbt::List(vec![]);
        for _ in 0..NBT_MAX_DEPTH {
            nbt = Nbt::List(vec![nbt]);
        }
        let mut w = Writer::new();
        assert!(w.write_nbt(&nbt).is_ok());
        let nbt = Nbt::List(vec![nbt]);
        assert_eq!(Writer::new().write_nbt(&nbt), Err(BufError::NbtDepthExceeded));
    }

    #[test]
    fn nbt_write_rejects_invalid_lists_and_end_values() {
        let mixed = Nbt::List(vec![Nbt::Int(1), Nbt::Long(2)]);
        assert_eq!(Writer::new().write_nbt(&mixed), Err(BufError::NbtMixedListTypes));
        let end_items = Nbt::List(vec![Nbt::End]);
        assert_eq!(Writer::new().write_nbt(&end_items), Err(BufError::NbtListWithoutType));
        let mut map = NbtCompound::new();
        map.insert("x".into(), Nbt::End);
        assert_eq!(Writer::new().write_nbt(&Nbt::Compound(map)), Err(BufError::NbtEndInCompound));
    }

    #[test]
    fn nbt_string_longer_than_u16_is_rejected() {
        let long = "a".repeat(70_000);
        assert!(matches!(
            Writer::new().write_nbt(&Nbt::String(long)),
            Err(BufError::NbtTooLong { what: "string", .. })
        ));
    }

    #[test]
    fn nbt_nested_complex_roundtrip() {
        let root = Nbt::Compound(NbtCompound::from([(
            "outer".to_string(),
            Nbt::List(vec![
                Nbt::Compound(NbtCompound::from([("a".to_string(), Nbt::LongArray(vec![1, 2]))])),
                Nbt::Compound(NbtCompound::new()),
            ]),
        )]));
        let mut w = Writer::new();
        w.write_nbt(&root).unwrap();
        assert_eq!(Reader::new(w.as_bytes()).read_nbt().unwrap(), root);
    }

    #[test]
    fn mixed_payload_roundtrip_in_one_buffer() {
        let mut w = Writer::new();
        w.write_identifier("axiom:set_block");
        w.write_block_pos((-100, -64, 200));
        w.write_varint(-1);
        w.write_nbt(&Nbt::Compound(NbtCompound::new())).unwrap();
        w.write_uuid(0xDEAD_BEEF);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_identifier().unwrap(), "axiom:set_block");
        assert_eq!(r.read_block_pos().unwrap(), (-100, -64, 200));
        assert_eq!(r.read_varint().unwrap(), -1);
        assert_eq!(r.read_nbt().unwrap(), Nbt::Compound(NbtCompound::new()));
        assert_eq!(r.read_uuid().unwrap(), 0xDEAD_BEEF);
        assert!(r.is_finished());
    }
}
