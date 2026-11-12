// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): RawBlueprint.java (writeHeader, readHeader,
// write, read). Layout: header, thumbnail byte array, block sections (long key + paletted container), block entities
// (long key + CompressedBlockEntity), entities (network NBT). Counts are varints.

use std::collections::BTreeMap;

use crate::buf::{Reader, Writer};

use super::{section, BlockSection, BlueprintError, BlueprintHeader, CompressedBlockEntity, RawBlueprint, StateIds};

impl RawBlueprint {
    /// `RawBlueprint.writeHeader`: header and thumbnail, without block data. The manifest sends this per blueprint.
    pub(crate) fn write_header_wire(&self, w: &mut Writer) -> Result<(), BlueprintError> {
        self.header.write_wire(w)?;
        w.write_byte_array(&self.thumbnail);
        Ok(())
    }

    /// `RawBlueprint.write`: the full blueprint, as sent in response to a request.
    pub(crate) fn write_wire(&self, w: &mut Writer, ids: StateIds) -> Result<(), BlueprintError> {
        self.write_header_wire(w)?;

        w.write_varint(self.blocks.len() as i32);
        for (&key, section) in &self.blocks {
            w.write_i64(key);
            section::write_wire(section.states(), ids.global_bits, w);
        }

        w.write_varint(self.block_entities.len() as i32);
        for (&key, entity) in &self.block_entities {
            w.write_i64(key);
            entity.write(w);
        }

        w.write_varint(self.entities.len() as i32);
        for entity in &self.entities {
            w.write_nbt(entity)?;
        }
        Ok(())
    }

    /// `RawBlueprint.read`: a full blueprint as uploaded. Palette ids are mapped to void air where the header
    /// format says so.
    pub(crate) fn read_wire(r: &mut Reader, ids: StateIds) -> Result<Self, BlueprintError> {
        let header = BlueprintHeader::read_wire(r)?;
        let thumbnail = r.read_byte_array(usize::MAX)?;

        let mut blocks = BTreeMap::new();
        for _ in 0..read_count(r)? {
            let key = r.read_i64()?;
            let mut states = section::read_wire(r, ids.global_bits)?;
            section::normalize_empty(&mut states, header.version, ids);
            blocks.insert(key, BlockSection::from_states(states)?);
        }

        let mut block_entities = BTreeMap::new();
        for _ in 0..read_count(r)? {
            let key = r.read_i64()?;
            block_entities.insert(key, CompressedBlockEntity::read(r)?);
        }

        let mut entities = Vec::new();
        for _ in 0..read_count(r)? {
            entities.push(r.read_nbt()?);
        }

        Ok(Self {
            header,
            thumbnail,
            blocks,
            block_entities,
            entities,
        })
    }
}

/// A varint count. Every counted item takes at least one byte, so a count above the remaining bytes is rejected
/// before anything is allocated for it.
fn read_count(r: &mut Reader) -> Result<usize, BlueprintError> {
    let count = usize::try_from(r.read_varint()?)
        .map_err(|_| BlueprintError::Malformed("negative count".into()))?;
    if count > r.remaining() {
        return Err(BlueprintError::Malformed(format!(
            "count {count} exceeds the {} bytes left",
            r.remaining()
        )));
    }
    Ok(count)
}
