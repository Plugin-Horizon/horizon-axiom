// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): blueprint/BlueprintIo.java
//
// Layout: int magic 0x0AE5BB36, int header length, header named NBT, int thumbnail length, thumbnail bytes,
// int block data length, gzip-compressed named NBT (DataVersion, BlockRegion, BlockEntities, Entities).
// All integers are big endian. The header length is written but not needed to read.

use crate::buf::{pack_block_pos, unpack_block_pos, Nbt, NbtCompound};

use super::{nbt, section, section_key, BlockSection, BlueprintError, BlueprintHeader, BlueprintHost, CompressedBlockEntity, RawBlueprint, StateIds};

/// File magic (`0xAE5BB36`).
pub const MAGIC: i32 = 0x0AE5_BB36;

/// Reads only the header of a `.bp` file.
pub fn read_header(bytes: &[u8]) -> Result<BlueprintHeader, BlueprintError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_magic()?;
    cursor.skip(4)?; // header length, not needed
    let (tag, _) = cursor.named_root()?;
    Ok(BlueprintHeader::load_nbt(&tag))
}

/// Reads a whole `.bp` file. Block states, block entities and entities are upgraded through the host when the
/// file was written with another data version.
pub fn read_raw<H: BlueprintHost + ?Sized>(host: &H, bytes: &[u8]) -> Result<RawBlueprint, BlueprintError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_magic()?;
    cursor.skip(4)?; // header length, not needed
    let (header_tag, _) = cursor.named_root()?;
    let header = BlueprintHeader::load_nbt(&header_tag);

    let thumbnail_len = cursor.i32()?;
    let thumbnail = cursor.take(length(thumbnail_len)?)?.to_vec();

    let block_len = length(cursor.i32()?)?; // block data length
    let block_bytes = nbt::gunzip(cursor.take(block_len)?)?;
    let (data, _) = nbt::read_named(&block_bytes)?;
    if !matches!(data, Nbt::Compound(_)) {
        return Err(BlueprintError::Malformed("block data root is not a compound".into()));
    }

    let current = host.data_version();
    let file_version = match nbt::int_or(&data, "DataVersion", 0) {
        0 => current,
        v => v,
    };
    let fix = file_version != current;
    let ids = StateIds::of(host);

    let mut blocks = std::collections::BTreeMap::new();
    for region in nbt::list_or_empty(&data, "BlockRegion") {
        let cx = nbt::int_or(region, "X", 0);
        let cy = nbt::int_or(region, "Y", 0);
        let cz = nbt::int_or(region, "Z", 0);
        let block_states = region
            .get("BlockStates")
            .filter(|tag| matches!(tag, Nbt::Compound(_)))
            .ok_or_else(|| BlueprintError::Malformed("block region without BlockStates".into()))?;
        let mut states = section::read_disk(block_states, |entry| {
            let entry = if fix {
                if !matches!(entry, Nbt::Compound(_)) {
                    return Err(BlueprintError::Malformed("block state palette entry is not a compound".into()));
                }
                host.upgrade_block_state(entry.clone(), file_version).map_err(BlueprintError::Malformed)?
            } else {
                entry.clone()
            };
            host.state_from_nbt(&entry)
                .ok_or_else(|| BlueprintError::Malformed(format!("unknown block state {entry:?}")))
        })?;
        section::normalize_empty(&mut states, header.version, ids);
        blocks.insert(pack_block_pos(cx, cy, cz), BlockSection::from_states(states)?);
    }

    let mut block_entities = std::collections::BTreeMap::new();
    for entry in nbt::list_or_empty(&data, "BlockEntities") {
        if !matches!(entry, Nbt::Compound(_)) {
            return Err(BlueprintError::Malformed("block entity is not a compound".into()));
        }
        let entry = if fix {
            host.upgrade_block_entity(entry.clone(), file_version).map_err(BlueprintError::Malformed)?
        } else {
            entry.clone()
        };
        let x = nbt::int_or(&entry, "x", 0);
        let y = nbt::int_or(&entry, "y", 0);
        let z = nbt::int_or(&entry, "z", 0);
        let id = nbt::string_or(&entry, "id", "");

        // Java skips unknown block entity types before it looks up the section. Here a missing section is always an
        // error, which only a damaged file can produce.
        let section = blocks
            .get(&section_key(x, y, z))
            .ok_or_else(|| BlueprintError::Malformed(format!("block entity at {x} {y} {z} has no block section")))?;
        let state = section.get(x, y, z);
        if !host.block_entity_accepts(&id, state) {
            continue;
        }

        let Nbt::Compound(mut map) = entry else { unreachable!("checked above") };
        for key in ["x", "y", "z", "id"] {
            map.remove(key);
        }
        let raw = nbt::write_named(&Nbt::Compound(map))?;
        let compressed = host.compress_block_entity(&raw);
        block_entities.insert(
            pack_block_pos(x, y, z),
            CompressedBlockEntity {
                original_size: raw.len() as i32,
                compression_dict: 0,
                compressed,
            },
        );
    }

    let mut entities = Vec::new();
    for entry in nbt::list_or_empty(&data, "Entities") {
        if !matches!(entry, Nbt::Compound(_)) {
            return Err(BlueprintError::Malformed("entity is not a compound".into()));
        }
        let entry = if fix {
            host.upgrade_entity(entry.clone(), file_version).map_err(BlueprintError::Malformed)?
        } else {
            entry.clone()
        };
        entities.push(entry);
    }

    Ok(RawBlueprint {
        header,
        thumbnail,
        blocks,
        block_entities,
        entities,
    })
}

/// Writes a blueprint to the `.bp` layout. Block entities whose block no longer carries a block entity type are
/// dropped, as in the Java code.
pub fn write_raw<H: BlueprintHost + ?Sized>(host: &H, raw: &RawBlueprint) -> Result<Vec<u8>, BlueprintError> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_be_bytes());

    let header = nbt::write_named(&raw.header.save_nbt())?;
    push_block(&mut out, &header);
    push_block(&mut out, &raw.thumbnail);

    let mut data = NbtCompound::new();
    data.insert("DataVersion".to_owned(), Nbt::Int(host.data_version()));

    let regions: Vec<Nbt> = raw
        .blocks
        .iter()
        .map(|(&key, section)| {
            let (cx, cy, cz) = unpack_block_pos(key);
            let mut map = NbtCompound::new();
            map.insert("X".to_owned(), Nbt::Int(cx));
            map.insert("Y".to_owned(), Nbt::Int(cy));
            map.insert("Z".to_owned(), Nbt::Int(cz));
            map.insert(
                "BlockStates".to_owned(),
                section::write_disk(section.states(), |state| host.state_to_nbt(state)),
            );
            Nbt::Compound(map)
        })
        .collect();
    data.insert("BlockRegion".to_owned(), Nbt::List(regions));

    let mut block_entities = Vec::new();
    for (&pos, entity) in &raw.block_entities {
        let (x, y, z) = unpack_block_pos(pos);
        let section = raw
            .blocks
            .get(&section_key(x, y, z))
            .ok_or_else(|| BlueprintError::Malformed(format!("block entity at {x} {y} {z} has no block section")))?;
        let Some(type_id) = host.block_entity_type_of(section.get(x, y, z)) else {
            continue;
        };
        if entity.compression_dict != 0 {
            return Err(BlueprintError::Malformed(format!(
                "unknown block entity compression dictionary {}",
                entity.compression_dict
            )));
        }
        let bytes = host
            .decompress_block_entity(&entity.compressed, entity.original_size.max(0) as usize)
            .map_err(BlueprintError::Malformed)?;
        let (tag, _) = nbt::read_named(&bytes)?;
        let Nbt::Compound(mut map) = tag else {
            return Err(BlueprintError::Malformed("block entity is not a compound".into()));
        };
        map.insert("x".to_owned(), Nbt::Int(x));
        map.insert("y".to_owned(), Nbt::Int(y));
        map.insert("z".to_owned(), Nbt::Int(z));
        map.insert("id".to_owned(), Nbt::String(type_id));
        block_entities.push(Nbt::Compound(map));
    }
    data.insert("BlockEntities".to_owned(), Nbt::List(block_entities));
    data.insert("Entities".to_owned(), Nbt::List(raw.entities.clone()));

    let block_data = nbt::gzip(&nbt::write_named(&Nbt::Compound(data))?)?;
    push_block(&mut out, &block_data);
    Ok(out)
}

/// Appends an int length and the bytes (the header and block data are length prefixed, the thumbnail too).
fn push_block(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn length(value: i32) -> Result<usize, BlueprintError> {
    usize::try_from(value).map_err(|_| BlueprintError::NotValid)
}

/// Big-endian reader over the file bytes.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], BlueprintError> {
        let end = self.pos.checked_add(n).ok_or(BlueprintError::NotValid)?;
        let slice = self.bytes.get(self.pos..end).ok_or(BlueprintError::NotValid)?;
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, n: usize) -> Result<(), BlueprintError> {
        self.take(n).map(|_| ())
    }

    fn i32(&mut self) -> Result<i32, BlueprintError> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn expect_magic(&mut self) -> Result<(), BlueprintError> {
        if self.i32()? == MAGIC {
            Ok(())
        } else {
            Err(BlueprintError::NotValid)
        }
    }

    /// A named root at the cursor, advancing past it.
    fn named_root(&mut self) -> Result<(Nbt, usize), BlueprintError> {
        let (tag, consumed) = nbt::read_named(&self.bytes[self.pos..])?;
        self.pos += consumed;
        Ok((tag, consumed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::test_host::{sample_blueprint, FakeHost, CHEST};

    #[test]
    fn file_round_trip_drops_block_entities_without_a_type() {
        let host = FakeHost::new();
        let original = sample_blueprint("tower");
        let bytes = write_raw(&host, &original).unwrap();
        let back = read_raw(&host, &bytes).unwrap();

        let mut expected = original.clone();
        expected.block_entities.remove(&crate::buf::pack_block_pos(23, -9, 7));
        assert_eq!(back, expected);
        assert_eq!(back.blocks.values().next().unwrap().get(4, 5, 6), CHEST);
    }

    #[test]
    fn file_starts_with_magic_and_header_reads_alone() {
        let host = FakeHost::new();
        let original = sample_blueprint("tower");
        let bytes = write_raw(&host, &original).unwrap();
        assert_eq!(&bytes[..4], &[0x0A, 0xE5, 0xBB, 0x36]);
        assert_eq!(read_header(&bytes).unwrap(), original.header);
    }

    #[test]
    fn wrong_magic_is_not_a_blueprint() {
        let host = FakeHost::new();
        let mut bytes = write_raw(&host, &sample_blueprint("x")).unwrap();
        bytes[3] ^= 1;
        assert!(matches!(read_raw(&host, &bytes), Err(BlueprintError::NotValid)));
        assert!(matches!(read_header(&bytes[..2]), Err(BlueprintError::NotValid)));
    }

    #[test]
    fn older_data_version_runs_the_fixers() {
        let mut old_host = FakeHost::new();
        old_host.data_version = 3000;
        old_host.legacy_stone_names = true;
        let original = sample_blueprint("old");
        let bytes = write_raw(&old_host, &original).unwrap();

        let host = FakeHost::new();
        let back = read_raw(&host, &bytes).unwrap();
        assert!(host.upgrades.get() > 0, "fixers should run for a different data version");
        assert_eq!(back.blocks, original.blocks);
    }

    #[test]
    fn same_data_version_skips_the_fixers() {
        let host = FakeHost::new();
        let bytes = write_raw(&host, &sample_blueprint("same")).unwrap();
        read_raw(&host, &bytes).unwrap();
        assert_eq!(host.upgrades.get(), 0);
    }

    #[test]
    fn block_entity_without_a_section_is_an_error() {
        let host = FakeHost::new();
        let mut original = sample_blueprint("orphan");
        original.blocks.clear();
        assert!(write_raw(&host, &original).is_err());
    }
}
