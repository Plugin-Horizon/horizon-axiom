// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): blueprint/BlueprintHeader.java
//
// Wire form: varint version (always CURRENT), name, author, string list (max 1024), int block count, bool
// contains-air. Disk form: a compound. The wire form drops the thumbnail camera values, as the Java code does.

use crate::buf::{Nbt, NbtCompound, Reader, Writer, DEFAULT_STRING_MAX_CHARS};

use super::{check_string, nbt, BlueprintError};

/// Blueprint format version written by this code. Java always writes this, even for loaded older versions.
pub const CURRENT_HEADER_VERSION: i32 = 2;

/// Limit on the tag list in the wire header (`NetworkHelper.readList(..., 1024)`).
const WIRE_TAG_LIMIT: usize = 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct BlueprintHeader {
    pub version: i32,
    pub name: String,
    pub author: String,
    pub tags: Vec<String>,
    pub thumbnail_yaw: f32,
    pub thumbnail_pitch: f32,
    pub locked_thumbnail: bool,
    pub block_count: i32,
    pub contains_air: bool,
}

impl BlueprintHeader {
    /// Empty block of this format version: structure void before version 2, void air from version 2 on.
    pub fn empty_block_state_name(&self) -> &'static str {
        if self.version <= 1 {
            super::STRUCTURE_VOID
        } else {
            super::VOID_AIR
        }
    }

    pub(crate) fn write_wire(&self, w: &mut Writer) -> Result<(), BlueprintError> {
        w.write_varint(CURRENT_HEADER_VERSION);
        check_string(&self.name)?;
        w.write_string(&self.name);
        check_string(&self.author)?;
        w.write_string(&self.author);
        for tag in &self.tags {
            check_string(tag)?;
        }
        w.write_collection(&self.tags, |w, tag| {
            w.write_string(tag);
            Ok(())
        })?;
        w.write_i32(self.block_count);
        w.write_bool(self.contains_air);
        Ok(())
    }

    pub(crate) fn read_wire(r: &mut Reader) -> Result<Self, BlueprintError> {
        let version = r.read_varint()?;
        let name = r.read_string(DEFAULT_STRING_MAX_CHARS)?;
        let author = r.read_string(DEFAULT_STRING_MAX_CHARS)?;
        let tags = r.read_list(WIRE_TAG_LIMIT, |r| r.read_string(DEFAULT_STRING_MAX_CHARS))?;
        let block_count = r.read_i32()?;
        let contains_air = r.read_bool()?;
        Ok(Self {
            version,
            name,
            author,
            tags,
            thumbnail_yaw: 0.0,
            thumbnail_pitch: 0.0,
            locked_thumbnail: true,
            block_count,
            contains_air,
        })
    }

    /// `BlueprintHeader.save`: the disk compound. Writes `CURRENT_HEADER_VERSION`, as Java does.
    pub(crate) fn save_nbt(&self) -> Nbt {
        let mut map = NbtCompound::new();
        map.insert("Version".to_owned(), Nbt::Int(CURRENT_HEADER_VERSION));
        map.insert("Name".to_owned(), Nbt::String(self.name.clone()));
        map.insert("Author".to_owned(), Nbt::String(self.author.clone()));
        map.insert(
            "Tags".to_owned(),
            Nbt::List(self.tags.iter().map(|t| Nbt::String(t.clone())).collect()),
        );
        map.insert("ThumbnailYaw".to_owned(), Nbt::Float(self.thumbnail_yaw));
        map.insert("ThumbnailPitch".to_owned(), Nbt::Float(self.thumbnail_pitch));
        map.insert("LockedThumbnail".to_owned(), Nbt::Byte(i8::from(self.locked_thumbnail)));
        map.insert("BlockCount".to_owned(), Nbt::Int(self.block_count));
        map.insert("ContainsAir".to_owned(), Nbt::Byte(i8::from(self.contains_air)));
        Nbt::Compound(map)
    }

    /// `BlueprintHeader.load`: missing or mistyped keys fall back to the Java defaults.
    pub(crate) fn load_nbt(tag: &Nbt) -> Self {
        Self {
            version: nbt::int_or(tag, "Version", 0),
            name: nbt::string_or(tag, "Name", ""),
            author: nbt::string_or(tag, "Author", ""),
            tags: nbt::list_or_empty(tag, "Tags")
                .iter()
                .filter_map(Nbt::as_str)
                .map(str::to_owned)
                .collect(),
            thumbnail_yaw: nbt::float_or(tag, "ThumbnailYaw", 135.0),
            thumbnail_pitch: nbt::float_or(tag, "ThumbnailPitch", 30.0),
            locked_thumbnail: nbt::bool_or(tag, "LockedThumbnail", false),
            block_count: nbt::int_or(tag, "BlockCount", 0),
            contains_air: nbt::bool_or(tag, "ContainsAir", false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> BlueprintHeader {
        BlueprintHeader {
            version: 2,
            name: "Tower".to_owned(),
            author: "Alice".to_owned(),
            tags: vec!["a".to_owned(), "bb".to_owned()],
            thumbnail_yaw: 1.0,
            thumbnail_pitch: 2.0,
            locked_thumbnail: true,
            block_count: 42,
            contains_air: false,
        }
    }

    #[test]
    fn wire_round_trip_drops_thumbnail_camera_as_java_does() {
        let mut w = Writer::new();
        header().write_wire(&mut w).unwrap();
        let bytes = w.into_bytes();
        let back = BlueprintHeader::read_wire(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(back.name, "Tower");
        assert_eq!(back.tags, header().tags);
        assert_eq!(back.block_count, 42);
        assert!(!back.contains_air);
        assert_eq!((back.thumbnail_yaw, back.thumbnail_pitch), (0.0, 0.0));
        assert!(back.locked_thumbnail);
    }

    #[test]
    fn wire_always_writes_current_version() {
        let mut old = header();
        old.version = 1;
        let mut w = Writer::new();
        old.write_wire(&mut w).unwrap();
        let bytes = w.into_bytes();
        assert_eq!(bytes[0] as i32, CURRENT_HEADER_VERSION);
    }

    #[test]
    fn disk_round_trip_keeps_every_field() {
        let back = BlueprintHeader::load_nbt(&header().save_nbt());
        assert_eq!(back, header());
    }

    #[test]
    fn load_uses_java_defaults_for_missing_keys() {
        let back = BlueprintHeader::load_nbt(&Nbt::Compound(NbtCompound::new()));
        assert_eq!(back.version, 0);
        assert_eq!((back.thumbnail_yaw, back.thumbnail_pitch), (135.0, 30.0));
        assert!(!back.locked_thumbnail);
        assert!(back.tags.is_empty());
    }

    #[test]
    fn empty_block_depends_on_version() {
        let mut h = header();
        assert_eq!(h.empty_block_state_name(), "minecraft:void_air");
        h.version = 1;
        assert_eq!(h.empty_block_state_name(), "minecraft:structure_void");
    }
}
