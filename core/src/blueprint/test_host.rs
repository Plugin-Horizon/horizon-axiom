// Test double for the blueprint host trait (no Java counterpart). Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023
// Moulberry Pty Ltd) only in the sense that it stands in for the server calls the Java code makes.
//
//! Test double for [`BlueprintHost`]: a small block state table, one block entity type and recorded side effects.

use std::cell::Cell;
use std::collections::HashSet;

use crate::buf::{Nbt, NbtCompound};

use super::{section_key, BlockSection, BlueprintHeader, BlueprintHost, CompressedBlockEntity, RawBlueprint};

pub const VOID_AIR_ID: u32 = 0;
pub const STRUCTURE_VOID_ID: u32 = 1;
pub const STONE: u32 = 2;
pub const CHEST: u32 = 3;
pub const CHEST_TYPE: &str = "minecraft:chest";
pub const LEGACY_STONE_NAME: &str = "minecraft:legacy_stone";
pub const DATA_VERSION: i32 = 4000;

pub fn state_name(state: u32) -> String {
    match state {
        0 => "minecraft:void_air".into(),
        1 => "minecraft:structure_void".into(),
        2 => "minecraft:stone".into(),
        3 => "minecraft:chest".into(),
        n => format!("minecraft:block_{n}"),
    }
}

pub fn state_id(name: &str) -> Option<u32> {
    match name {
        "minecraft:void_air" => Some(0),
        "minecraft:structure_void" => Some(1),
        "minecraft:stone" => Some(2),
        "minecraft:chest" => Some(3),
        other => other.strip_prefix("minecraft:block_")?.parse().ok(),
    }
}

#[derive(Default)]
pub struct FakeHost {
    pub data_version: i32,
    /// Writes stone under the name `minecraft:legacy_stone` (for data version upgrade tests).
    pub legacy_stone_names: bool,
    pub permissions: HashSet<(u128, String)>,
    pub mismatched: HashSet<u128>,
    pub players: Vec<u128>,
    pub sent: Vec<(u128, String, Vec<u8>)>,
    pub messages: Vec<(u128, String)>,
    pub kicks: Vec<(u128, String)>,
    pub upgrades: Cell<usize>,
}

impl FakeHost {
    pub fn new() -> Self {
        Self { data_version: DATA_VERSION, ..Self::default() }
    }

    pub fn grant(&mut self, player: u128, permission: &str) {
        self.permissions.insert((player, permission.to_owned()));
    }
}

impl BlueprintHost for FakeHost {
    fn data_version(&self) -> i32 {
        self.data_version
    }

    fn default_state(&self, block: &str) -> u32 {
        state_id(block).expect("known block")
    }

    fn global_state_bits(&self) -> u8 {
        15
    }

    fn state_to_nbt(&self, state: u32) -> Nbt {
        let name = if self.legacy_stone_names && state == STONE {
            LEGACY_STONE_NAME.to_owned()
        } else {
            state_name(state)
        };
        let mut map = NbtCompound::new();
        map.insert("Name".to_owned(), Nbt::String(name));
        Nbt::Compound(map)
    }

    fn state_from_nbt(&self, nbt: &Nbt) -> Option<u32> {
        nbt.get("Name").and_then(Nbt::as_str).and_then(state_id)
    }

    fn block_entity_type_of(&self, state: u32) -> Option<String> {
        (state == CHEST).then(|| CHEST_TYPE.to_owned())
    }

    fn block_entity_accepts(&self, type_id: &str, state: u32) -> bool {
        type_id == CHEST_TYPE && state == CHEST
    }

    fn upgrade_block_state(&self, mut nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        self.upgrades.set(self.upgrades.get() + 1);
        if let Nbt::Compound(map) = &mut nbt {
            if map.get("Name").and_then(Nbt::as_str) == Some(LEGACY_STONE_NAME) {
                map.insert("Name".to_owned(), Nbt::String("minecraft:stone".to_owned()));
            }
        }
        Ok(nbt)
    }

    fn upgrade_block_entity(&self, nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        self.upgrades.set(self.upgrades.get() + 1);
        Ok(nbt)
    }

    fn upgrade_entity(&self, nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        self.upgrades.set(self.upgrades.get() + 1);
        Ok(nbt)
    }

    fn compress_block_entity(&self, raw: &[u8]) -> Vec<u8> {
        crate::buffer::zstd_compress(raw)
    }

    fn decompress_block_entity(&self, compressed: &[u8], original_size: usize) -> Result<Vec<u8>, String> {
        crate::buffer::zstd_decompress(compressed, original_size).map_err(|e| format!("{e:?}"))
    }

    fn can_use_axiom(&self, player: u128, permission: &str) -> bool {
        self.permissions.contains(&(player, permission.to_owned()))
    }

    fn is_mismatched_data_version(&self, player: u128) -> bool {
        self.mismatched.contains(&player)
    }

    fn game_version_name(&self) -> String {
        "26.3".to_owned()
    }

    fn send_message(&mut self, player: u128, text: &str) {
        self.messages.push((player, text.to_owned()));
    }

    fn kick(&mut self, player: u128, reason: &str) {
        self.kicks.push((player, reason.to_owned()));
    }

    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.sent.push((player, channel.to_owned(), data));
    }

    fn online_players(&self) -> Vec<u128> {
        self.players.clone()
    }
}

/// A blueprint with two sections, a chest with a block entity, a stone block with a stale block entity (dropped on
/// write) and one entity.
pub fn sample_blueprint(name: &str) -> RawBlueprint {
    let header = BlueprintHeader {
        version: 2,
        name: name.to_owned(),
        author: "tester".to_owned(),
        tags: vec!["small".to_owned(), "wood".to_owned()],
        thumbnail_yaw: 90.0,
        thumbnail_pitch: 12.5,
        locked_thumbnail: false,
        block_count: 3,
        contains_air: true,
    };

    let mut first = BlockSection::filled(VOID_AIR_ID);
    first.set(1, 2, 3, STONE);
    first.set(4, 5, 6, CHEST);
    let mut second = BlockSection::filled(VOID_AIR_ID);
    second.set(0, 0, 0, STONE);
    second.set(7, 7, 7, STONE);

    let mut blocks = std::collections::BTreeMap::new();
    blocks.insert(section_key(0, 0, 0), first);
    blocks.insert(section_key(16, -16, 0), second);

    let mut chest = NbtCompound::new();
    chest.insert("CustomName".to_owned(), Nbt::String("Loot".to_owned()));
    chest.insert("Items".to_owned(), Nbt::List(Vec::new()));
    let chest_raw = super::nbt::write_named(&Nbt::Compound(chest)).expect("encode");

    let mut stale = NbtCompound::new();
    stale.insert("Items".to_owned(), Nbt::List(Vec::new()));
    let stale_raw = super::nbt::write_named(&Nbt::Compound(stale)).expect("encode");

    let mut block_entities = std::collections::BTreeMap::new();
    block_entities.insert(
        crate::buf::pack_block_pos(4, 5, 6),
        CompressedBlockEntity {
            original_size: chest_raw.len() as i32,
            compression_dict: 0,
            compressed: crate::buffer::zstd_compress(&chest_raw),
        },
    );
    // Position of a stone block in the second section: no block entity type, so it is not written.
    block_entities.insert(
        crate::buf::pack_block_pos(23, -9, 7),
        CompressedBlockEntity {
            original_size: stale_raw.len() as i32,
            compression_dict: 0,
            compressed: crate::buffer::zstd_compress(&stale_raw),
        },
    );

    let mut armor_stand = NbtCompound::new();
    armor_stand.insert("id".to_owned(), Nbt::String("minecraft:armor_stand".to_owned()));
    armor_stand.insert("Pos".to_owned(), Nbt::List(vec![Nbt::Double(1.5), Nbt::Double(64.0), Nbt::Double(-2.0)]));

    RawBlueprint {
        header,
        thumbnail: vec![0x89, b'P', b'N', b'G', 1, 2, 3],
        blocks,
        block_entities,
        entities: vec![Nbt::Compound(armor_stand)],
    }
}
