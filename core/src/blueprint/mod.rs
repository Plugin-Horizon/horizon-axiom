// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): blueprint/BlueprintIo.java,
// blueprint/RawBlueprint.java, blueprint/ServerBlueprintManager.java, blueprint/ServerBlueprintRegistry.java,
// blueprint/BlockEntityMap.java (type lookup goes to the host), blueprint/DFUHelper.java (fixers go to the host),
// blueprint/BlueprintHeader.java, packet/impl/UploadBlueprintPacketListener.java,
// packet/impl/BlueprintRequestPacketListener.java.
//
//! Server blueprints: `.bp` files in a folder, their in-memory form, and the manifest, upload and request packets.
//! The game supplies block states, block entity types, data fixers, compression and permissions through
//! [`BlueprintHost`].

mod file;
mod header;
mod nbt;
mod packets;
mod section;
mod wire;

#[cfg(test)]
mod test_host;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::buf::{self, BufError, Nbt, Reader, Writer, DEFAULT_STRING_MAX_CHARS};

pub use file::{read_header, read_raw, write_raw, MAGIC};
pub use header::{BlueprintHeader, CURRENT_HEADER_VERSION};
pub use packets::{
    handle_request, handle_upload, manifest_packets, send_manifest, RequestOutcome, UploadOutcome,
};
pub use section::{BlockSection, SECTION_ENTRIES};

/// Permission node checked for the manifest (`AxiomPermission.BLUEPRINT_MANIFEST`).
pub const PERMISSION_MANIFEST: &str = "axiom.blueprint.manifest";
/// Permission node checked for uploads (`AxiomPermission.BLUEPRINT_UPLOAD`).
pub const PERMISSION_UPLOAD: &str = "axiom.blueprint.upload";
/// Permission node checked for requests (`AxiomPermission.BLUEPRINT_REQUEST`).
pub const PERMISSION_REQUEST: &str = "axiom.blueprint.request";

/// Channel carrying the manifest to clients.
pub const CHANNEL_MANIFEST: &str = "axiom:blueprint_manifest";
/// Channel carrying one requested blueprint back to the client.
pub const CHANNEL_RESPONSE: &str = "axiom:response_blueprint";

/// Java `ServerBlueprintManager.MAX_SIZE`: a manifest packet is closed once it grows past this many bytes.
pub const MANIFEST_MAX_PACKET_BYTES: usize = 1_000_000;

/// Empty block of blueprint format version 2 and in-memory default.
pub const VOID_AIR: &str = "minecraft:void_air";
/// Empty block of blueprint format version 1.
pub const STRUCTURE_VOID: &str = "minecraft:structure_void";

/// Errors while reading or writing blueprints.
#[derive(Debug)]
pub enum BlueprintError {
    /// Magic number or framing is wrong (`Not a valid Blueprint`).
    NotValid,
    /// Structurally valid bytes with content that cannot be used.
    Malformed(String),
    /// Byte-level decoding or encoding failed.
    Buf(BufError),
    /// File system error.
    Io(std::io::Error),
}

impl fmt::Display for BlueprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotValid => write!(f, "Not a valid Blueprint"),
            Self::Malformed(reason) => write!(f, "malformed blueprint: {reason}"),
            Self::Buf(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BlueprintError {}

impl From<BufError> for BlueprintError {
    fn from(e: BufError) -> Self {
        Self::Buf(e)
    }
}

impl From<std::io::Error> for BlueprintError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Everything the blueprint code needs from the game. Block states are global ids, players are UUIDs and
/// dimensions are registry names. Calls come from the server's main thread.
pub trait BlueprintHost {
    /// Current game data version (`DFUHelper.DATA_VERSION`).
    fn data_version(&self) -> i32;

    /// Global id of a block state with default properties, e.g. `minecraft:void_air`.
    fn default_state(&self, block: &str) -> u32;

    /// Bits needed for a global block state id (the block states global palette width).
    fn global_state_bits(&self) -> u8;

    /// `BlockState.CODEC` encode: a compound with `Name` and optional `Properties`.
    fn state_to_nbt(&self, state: u32) -> Nbt;

    /// `BlockState.CODEC` decode. `None` for unknown blocks or properties.
    fn state_from_nbt(&self, nbt: &Nbt) -> Option<u32>;

    /// `BlockEntityMap.get(block)`: registry id of the block entity type a state carries, if any.
    fn block_entity_type_of(&self, state: u32) -> Option<String>;

    /// `BlockEntityType.isValid` for the type with this registry id. `false` when the id is unknown.
    fn block_entity_accepts(&self, type_id: &str, state: u32) -> bool;

    /// `DataFixers` update of a block state from `from_version` to `data_version()`.
    fn upgrade_block_state(&self, nbt: Nbt, from_version: i32) -> Result<Nbt, String>;

    /// `DataFixers` update of a block entity from `from_version` to `data_version()`.
    fn upgrade_block_entity(&self, nbt: Nbt, from_version: i32) -> Result<Nbt, String>;

    /// `DataFixers` update of an entity from `from_version` to `data_version()`.
    fn upgrade_entity(&self, nbt: Nbt, from_version: i32) -> Result<Nbt, String>;

    /// `CompressedBlockEntity.compress`: zstd with the `block_entities_v1` dictionary (dictionary 0).
    fn compress_block_entity(&self, raw: &[u8]) -> Vec<u8>;

    /// `CompressedBlockEntity.decompress` for dictionary 0. `original_size` is the expected output length.
    fn decompress_block_entity(&self, compressed: &[u8], original_size: usize) -> Result<Vec<u8>, String>;

    /// `AxiomPaper.canUseAxiom(player, permission)`.
    fn can_use_axiom(&self, player: u128, permission: &str) -> bool;

    /// `AxiomPaper.isMismatchedDataVersion(player)`.
    fn is_mismatched_data_version(&self, player: u128) -> bool;

    /// Human readable game version, used in the version mismatch message.
    fn game_version_name(&self) -> String;

    /// Plain text chat or system message to one player.
    fn send_message(&mut self, player: u128, text: &str);

    /// Disconnects one player with a reason.
    fn kick(&mut self, player: u128, reason: &str);

    /// Sends one custom payload on `channel` to one player.
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>);

    /// Every online player, for the manifest broadcast after an upload.
    fn online_players(&self) -> Vec<u128>;
}

/// Block state ids the codecs need, taken from the host once per call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StateIds {
    pub global_bits: u8,
    pub void_air: u32,
    pub structure_void: u32,
}

impl StateIds {
    pub(crate) fn of<H: BlueprintHost + ?Sized>(host: &H) -> Self {
        Self {
            global_bits: host.global_state_bits(),
            void_air: host.default_state(VOID_AIR),
            structure_void: host.default_state(STRUCTURE_VOID),
        }
    }
}

/// Block entity data kept compressed in memory, with the wire layout of `CompressedBlockEntity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedBlockEntity {
    pub original_size: i32,
    pub compression_dict: i8,
    pub compressed: Vec<u8>,
}

impl CompressedBlockEntity {
    pub(crate) fn read(r: &mut Reader) -> Result<Self, BlueprintError> {
        let original_size = r.read_varint()?;
        let compression_dict = r.read_i8()?;
        let compressed = r.read_byte_array(usize::MAX)?;
        Ok(Self { original_size, compression_dict, compressed })
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.write_varint(self.original_size);
        w.write_i8(self.compression_dict);
        w.write_byte_array(&self.compressed);
    }
}

/// A blueprint as held in memory: header, thumbnail, block sections, block entities and entities.
#[derive(Debug, Clone, PartialEq)]
pub struct RawBlueprint {
    pub header: BlueprintHeader,
    pub thumbnail: Vec<u8>,
    /// Sections keyed by `BlockPos.asLong(x >> 4, y >> 4, z >> 4)`.
    pub blocks: BTreeMap<i64, BlockSection>,
    /// Block entities keyed by `BlockPos.asLong(x, y, z)`.
    pub block_entities: BTreeMap<i64, CompressedBlockEntity>,
    pub entities: Vec<Nbt>,
}

/// Blueprints of one folder, keyed by `/`-separated path without the `.bp` suffix (e.g. `/houses/small`).
#[derive(Debug, Default)]
pub struct BlueprintStore {
    root: PathBuf,
    blueprints: BTreeMap<String, RawBlueprint>,
}

impl BlueprintStore {
    /// `ServerBlueprintManager.initialize`: reads every `.bp` file below `root`. Files that fail to read are
    /// returned with their error and left out, as the Java code only prints them.
    pub fn load<H: BlueprintHost + ?Sized>(host: &H, root: PathBuf) -> (Self, Vec<(PathBuf, BlueprintError)>) {
        let mut blueprints = BTreeMap::new();
        let mut errors = Vec::new();
        load_folder(host, &root, "/", &mut blueprints, &mut errors);
        (Self { root, blueprints }, errors)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn blueprints(&self) -> &BTreeMap<String, RawBlueprint> {
        &self.blueprints
    }

    pub fn get(&self, key: &str) -> Option<&RawBlueprint> {
        self.blueprints.get(key)
    }

    pub(crate) fn insert(&mut self, key: String, blueprint: RawBlueprint) {
        self.blueprints.insert(key, blueprint);
    }
}

/// `ServerBlueprintManager.loadRegistryFromFolder`: files ending in `.bp` by name, directories recursively.
fn load_folder<H: BlueprintHost + ?Sized>(
    host: &H,
    folder: &Path,
    location: &str,
    out: &mut BTreeMap<String, RawBlueprint>,
    errors: &mut Vec<(PathBuf, BlueprintError)>,
) {
    if !folder.is_dir() {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(folder) else { return };
    let mut paths: Vec<PathBuf> = read_dir.filter_map(|entry| entry.ok().map(|e| e.path())).collect();
    paths.sort();

    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(stem) = name.strip_suffix(".bp") {
            let loaded = std::fs::read(&path)
                .map_err(BlueprintError::from)
                .and_then(|bytes| read_raw(host, &bytes));
            match loaded {
                Ok(raw) => {
                    out.insert(format!("{location}{stem}"), raw);
                }
                Err(e) => errors.push((path.clone(), e)),
            }
        } else if path.is_dir() {
            load_folder(host, &path, &format!("{location}{name}/"), out, errors);
        }
    }
}

/// Rejects text that vanilla `writeUtf` would refuse (more than 32767 UTF-16 units).
pub(crate) fn check_string(s: &str) -> Result<(), BlueprintError> {
    let units = s.encode_utf16().count();
    if units > DEFAULT_STRING_MAX_CHARS {
        return Err(BlueprintError::Malformed(format!(
            "string of {units} chars exceeds {DEFAULT_STRING_MAX_CHARS}"
        )));
    }
    Ok(())
}

/// Packs a section position (block coordinates shifted right by 4).
pub(crate) fn section_key(x: i32, y: i32, z: i32) -> i64 {
    buf::pack_block_pos(x >> 4, y >> 4, z >> 4)
}
