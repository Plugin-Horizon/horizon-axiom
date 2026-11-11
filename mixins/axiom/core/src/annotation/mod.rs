// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd):
//   annotations/ServerAnnotations.java, annotations/ServerAnnotationsAdapater.java,
//   packet/impl/UpdateAnnotationPacketListener.java, packet/impl/MarkerNbtRequestPacketListener.java,
//   WorldExtension.java (axiom:marker_data tick and join snapshot), marker/MarkerData.java (see marker.rs)
//
//! Annotations (lines, text, images, outlines) and markers.
//!
//! Persistence: Java keeps annotations in the world's PersistentDataContainer. Here each dimension's
//! annotations live in `<world folder>/axiom/annotations.bin`, written through a temp file and a rename.
//! A corrupt file header is an error, not an empty store, so a later save cannot wipe the data.
//!
//! Entry points for the glue (all on the game thread):
//! - `handle_update_packet`: raw `axiom:annotation_update` payload from a client.
//! - `handle_marker_nbt_request`: raw `axiom:marker_nbt_request` payload from a client.
//! - `on_player_join`: marker snapshot, and the annotation list for players with Axiom.
//! - `tick_markers`: once per game tick per dimension.
//! - `AnnotationState::unload_world`: when a dimension unloads.

pub mod data;
pub mod marker;

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::buf::{BufError, Nbt, Reader, Writer};

pub use data::{AnnotationData, AnnotationUpdateAction};
pub use marker::MarkerData;

use data::{read_action_list, write_action_list};

pub const CHANNEL_ANNOTATION_UPDATE: &str = "axiom:annotation_update";
pub const CHANNEL_MARKER_NBT_REQUEST: &str = "axiom:marker_nbt_request";
pub const CHANNEL_MARKER_DATA: &str = "axiom:marker_data";
pub const CHANNEL_MARKER_NBT_RESPONSE: &str = "axiom:marker_nbt_response";

/// `AxiomPermission.ANNOTATION_CREATE`.
pub const NODE_ANNOTATION_CREATE: &str = "axiom.annotation.create";
/// `AxiomPermission.ENTITY_REQUESTDATA`.
pub const NODE_ENTITY_REQUEST_DATA: &str = "axiom.entity.request_data";

const ANNOTATION_DIR: &str = "axiom";
const ANNOTATION_FILE: &str = "annotations.bin";

#[derive(Debug)]
pub enum AnnotationError {
    /// A payload or file could not be decoded. For a client payload the glue should disconnect, as Java does.
    Buf(BufError),
    /// Loading or saving the annotation file failed. The message is safe to show to players.
    Persist(String),
}

impl From<BufError> for AnnotationError {
    fn from(e: BufError) -> Self {
        Self::Buf(e)
    }
}

impl fmt::Display for AnnotationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Buf(e) => write!(f, "malformed annotation data: {e}"),
            Self::Persist(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AnnotationError {}

/// A Marker entity as the host sees it.
#[derive(Clone, Debug)]
pub struct MarkerEntity {
    pub uuid: u128,
    pub position: [f64; 3],
    /// Custom data compound. Use an empty compound when the marker has none.
    pub data: Nbt,
}

/// What the annotation code needs from the game. Players are uuids, dimensions are names such as
/// `"minecraft:overworld"`.
pub trait AnnotationHost {
    /// Config `allow-annotations`. When false, every entry point does nothing.
    fn allow_annotations(&self) -> bool;
    /// Whether marker sync runs (`sendMarkers` in Java).
    fn send_markers(&self) -> bool;
    /// Folder of the world for this dimension, or `None` to keep annotations in memory only.
    fn world_folder(&self, dim: &str) -> Option<PathBuf>;
    /// Dimension the player is in, or `None` if the player is not online.
    fn player_dimension(&self, player: u128) -> Option<String>;
    /// Players currently in the dimension.
    fn world_players(&self, dim: &str) -> Vec<u128>;
    /// `canUseAxiom(player)`: the player has Axiom active.
    fn can_use_axiom(&self, player: u128) -> bool;
    /// Permission node check for the player (`hasPermission`).
    fn has_permission(&self, player: u128, node: &str) -> bool;
    /// `canModifyWorld(player, world)`: world whitelist/blacklist and related events.
    fn can_modify_world(&self, player: u128, dim: &str) -> bool;
    /// Marker entities in the dimension, excluding markers the host hides from Axiom
    /// (`ImplAxiomHiddenEntities.isMarkerHidden`). Java scans every entity of the level, not only the
    /// ones near players.
    fn markers(&mut self, dim: &str) -> Vec<MarkerEntity>;
    /// Custom data of the Marker entity with this uuid, an empty compound if it has none.
    /// `None` if there is no marker with that uuid.
    fn marker_custom_data(&mut self, dim: &str, uuid: u128) -> Option<Nbt>;
    /// Sends a plugin-channel payload to one player.
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>);

    /// `canUseAxiom(player, permission)`: Axiom is active and the permission is held.
    fn can(&self, player: u128, node: &str) -> bool {
        self.can_use_axiom(player) && self.has_permission(player, node)
    }
}

/// Annotations of one dimension, kept in insertion order like Java's `LinkedHashMap`.
#[derive(Default)]
pub struct AnnotationStore {
    folder: Option<PathBuf>,
    entries: Vec<(u128, AnnotationData)>,
}

fn upsert(entries: &mut Vec<(u128, AnnotationData)>, uuid: u128, data: AnnotationData) {
    match entries.iter_mut().find(|(id, _)| *id == uuid) {
        Some(slot) => slot.1 = data,
        None => entries.push((uuid, data)),
    }
}

impl AnnotationStore {
    fn load(folder: Option<PathBuf>) -> Result<Self, AnnotationError> {
        let entries = match &folder {
            Some(folder) => match fs::read(folder.join(ANNOTATION_DIR).join(ANNOTATION_FILE)) {
                Ok(bytes) => decode(&bytes)?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(AnnotationError::Persist(format!("annotation file unreadable: {e}"))),
            },
            None => Vec::new(),
        };
        Ok(Self { folder, entries })
    }

    fn persist(&self) -> Result<(), AnnotationError> {
        let Some(folder) = &self.folder else {
            return Ok(());
        };
        let dir = folder.join(ANNOTATION_DIR);
        let path = dir.join(ANNOTATION_FILE);
        let tmp = dir.join(format!("{ANNOTATION_FILE}.tmp"));
        let save = || -> io::Result<()> {
            fs::create_dir_all(&dir)?;
            fs::write(&tmp, encode(&self.entries))?;
            fs::rename(&tmp, &path)
        };
        save().map_err(|e| AnnotationError::Persist(format!("annotation save failed: {e}")))
    }

    /// Applies one action. Returns whether the stored data changed (Java's `dirty` flag).
    fn apply(&mut self, action: &AnnotationUpdateAction) -> bool {
        match action {
            AnnotationUpdateAction::Create { uuid, data } => {
                upsert(&mut self.entries, *uuid, data.clone());
                true
            }
            AnnotationUpdateAction::Delete { uuid } => {
                let before = self.entries.len();
                self.entries.retain(|(id, _)| id != uuid);
                self.entries.len() != before
            }
            AnnotationUpdateAction::Move { uuid, to } => match self.entries.iter_mut().find(|(id, _)| id == uuid) {
                Some((_, data)) => {
                    data.set_position(*to);
                    true
                }
                None => false,
            },
            AnnotationUpdateAction::ClearAll => {
                let changed = !self.entries.is_empty();
                self.entries.clear();
                changed
            }
            AnnotationUpdateAction::Rotate { uuid, to } => match self.entries.iter_mut().find(|(id, _)| id == uuid) {
                Some((_, data)) => {
                    data.set_rotation(*to);
                    true
                }
                None => false,
            },
        }
    }
}

/// File format: varint count, then per entry the uuid and a byte array holding the encoded annotation.
fn encode(entries: &[(u128, AnnotationData)]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_varint(entries.len() as i32);
    for (uuid, data) in entries {
        let mut entry = Writer::new();
        data.write(&mut entry);
        w.write_uuid(*uuid);
        w.write_byte_array(&entry.into_bytes());
    }
    w.into_bytes()
}

/// Entries that fail to decode are skipped, as Java's `ServerAnnotationsAdapater` does. A broken
/// file header is an error.
fn decode(bytes: &[u8]) -> Result<Vec<(u128, AnnotationData)>, AnnotationError> {
    let mut r = Reader::new(bytes);
    let count = r.read_varint()?;
    if count < 0 {
        return Err(AnnotationError::Buf(BufError::NegativeLength(count)));
    }
    let mut entries = Vec::new();
    for _ in 0..count {
        let uuid = r.read_uuid()?;
        let payload = r.read_byte_array(r.remaining())?;
        if let Ok(Some(data)) = AnnotationData::read(&mut Reader::new(&payload)) {
            upsert(&mut entries, uuid, data);
        }
    }
    Ok(entries)
}

/// Marker snapshot of one dimension, kept between ticks.
#[derive(Default)]
struct MarkerState {
    previous: HashMap<u128, MarkerData>,
}

/// Per-dimension annotation and marker state. The glue keeps one for the whole server.
#[derive(Default)]
pub struct AnnotationState {
    stores: HashMap<String, AnnotationStore>,
    markers: HashMap<String, MarkerState>,
}

impl AnnotationState {
    /// The store of a dimension, loaded from the host's world folder on first use.
    fn store<H: AnnotationHost>(&mut self, host: &H, dim: &str) -> Result<&mut AnnotationStore, AnnotationError> {
        match self.stores.entry(dim.to_owned()) {
            Entry::Occupied(e) => Ok(e.into_mut()),
            Entry::Vacant(e) => {
                let store = AnnotationStore::load(host.world_folder(dim))?;
                Ok(e.insert(store))
            }
        }
    }

    /// Drops the cached state of a dimension that unloaded. The file on disk is kept.
    pub fn unload_world(&mut self, dim: &str) {
        self.stores.remove(dim);
        self.markers.remove(dim);
    }
}

fn encode_marker_data(changed: &[MarkerData], removed: &[u128]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_varint(changed.len() as i32);
    for marker in changed {
        marker.write(&mut w);
    }
    w.write_varint(removed.len() as i32);
    for uuid in removed {
        w.write_uuid(*uuid);
    }
    w.into_bytes()
}

/// Handles a client's `axiom:annotation_update` payload (`UpdateAnnotationPacketListener`).
pub fn handle_update_packet<H: AnnotationHost>(
    host: &mut H,
    state: &mut AnnotationState,
    player: u128,
    payload: &[u8],
) -> Result<(), AnnotationError> {
    if !host.allow_annotations() || !host.can(player, NODE_ANNOTATION_CREATE) {
        return Ok(());
    }
    let Some(dim) = host.player_dimension(player) else {
        return Ok(());
    };
    if !host.can_modify_world(player, &dim) {
        return Ok(());
    }
    let actions = read_action_list(&mut Reader::new(payload))?;
    apply_updates(host, state, &dim, &actions)
}

/// Applies actions to the dimension's store, saves once if anything changed, then forwards every
/// action (including ones that changed nothing, as Java does) to the Axiom users in that dimension.
pub fn apply_updates<H: AnnotationHost>(
    host: &mut H,
    state: &mut AnnotationState,
    dim: &str,
    actions: &[AnnotationUpdateAction],
) -> Result<(), AnnotationError> {
    if !host.allow_annotations() {
        return Ok(());
    }
    let store = state.store(&*host, dim)?;
    let mut dirty = false;
    for action in actions {
        dirty |= store.apply(action);
    }
    if dirty {
        store.persist()?;
    }

    let recipients: Vec<u128> = host
        .world_players(dim)
        .into_iter()
        .filter(|p| host.can_use_axiom(*p))
        .collect();
    if !recipients.is_empty() {
        let bytes = write_action_list(actions);
        for player in recipients {
            host.send_payload(player, CHANNEL_ANNOTATION_UPDATE, bytes.clone());
        }
    }
    Ok(())
}

/// Sends the full annotation list to one player: a ClearAll, then a Create per annotation
/// (`ServerAnnotations.sendAll`).
pub fn send_all<H: AnnotationHost>(
    host: &mut H,
    state: &mut AnnotationState,
    dim: &str,
    player: u128,
) -> Result<(), AnnotationError> {
    if !host.allow_annotations() {
        return Ok(());
    }
    let store = state.store(&*host, dim)?;
    let mut actions = vec![AnnotationUpdateAction::ClearAll];
    actions.extend(store.entries.iter().map(|(uuid, data)| AnnotationUpdateAction::Create {
        uuid: *uuid,
        data: data.clone(),
    }));
    host.send_payload(player, CHANNEL_ANNOTATION_UPDATE, write_action_list(&actions));
    Ok(())
}

/// Handles a client's marker NBT request (`MarkerNbtRequestPacketListener`).
pub fn handle_marker_nbt_request<H: AnnotationHost>(
    host: &mut H,
    player: u128,
    payload: &[u8],
) -> Result<(), AnnotationError> {
    if !host.can(player, NODE_ENTITY_REQUEST_DATA) {
        return Ok(());
    }
    let Some(dim) = host.player_dimension(player) else {
        return Ok(());
    };
    if !host.can_modify_world(player, &dim) {
        return Ok(());
    }
    let mut r = Reader::new(payload);
    let uuid = r.read_uuid()?;
    // The request also carries a reason varint, which Java reads and ignores.
    let _reason = r.read_varint()?;

    if let Some(data) = host.marker_custom_data(&dim, uuid) {
        let mut w = Writer::new();
        w.write_uuid(uuid);
        w.write_nbt(&data)?;
        host.send_payload(player, CHANNEL_MARKER_NBT_RESPONSE, w.into_bytes());
    }
    Ok(())
}

/// Player join: sends the marker snapshot (if any) and, for players with Axiom, the annotations.
pub fn on_player_join<H: AnnotationHost>(
    host: &mut H,
    state: &mut AnnotationState,
    dim: &str,
    player: u128,
) -> Result<(), AnnotationError> {
    let snapshot: Vec<MarkerData> = state
        .markers
        .get(dim)
        .map(|m| m.previous.values().cloned().collect())
        .unwrap_or_default();
    if !snapshot.is_empty() {
        host.send_payload(player, CHANNEL_MARKER_DATA, encode_marker_data(&snapshot, &[]));
    }
    if host.can_use_axiom(player) {
        send_all(host, state, dim, player)?;
    }
    Ok(())
}

/// One tick of marker sync (`WorldExtension.tickMarkers`). Sends changed markers and removed uuids to
/// the Axiom users in the dimension. Nothing is sent when nothing changed.
pub fn tick_markers<H: AnnotationHost>(host: &mut H, state: &mut AnnotationState, dim: &str) {
    if !host.send_markers() {
        return;
    }
    let markers = state.markers.entry(dim.to_owned()).or_default();

    let mut changed = Vec::new();
    let mut present = HashSet::new();
    for entity in host.markers(dim) {
        let current = MarkerData::create_from(entity.uuid, entity.position, &entity.data);
        if markers.previous.get(&entity.uuid) != Some(&current) {
            markers.previous.insert(entity.uuid, current.clone());
            changed.push(current);
        }
        present.insert(entity.uuid);
    }

    let removed: Vec<u128> = markers.previous.keys().filter(|id| !present.contains(id)).copied().collect();
    for uuid in &removed {
        markers.previous.remove(uuid);
    }
    if changed.is_empty() && removed.is_empty() {
        return;
    }

    let bytes = encode_marker_data(&changed, &removed);
    let recipients: Vec<u128> = host
        .world_players(dim)
        .into_iter()
        .filter(|p| host.can_use_axiom(*p))
        .collect();
    for player in recipients {
        host.send_payload(player, CHANNEL_MARKER_DATA, bytes.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::NbtCompound;

    struct FakeHost {
        allow: bool,
        send_markers: bool,
        folder: Option<PathBuf>,
        dim: String,
        /// (player, may use axiom, has the permissions, online in the dimension)
        players: Vec<(u128, bool, bool)>,
        modifiable: bool,
        markers: Vec<MarkerEntity>,
        sent: Vec<(u128, String, Vec<u8>)>,
    }

    impl FakeHost {
        fn new(folder: Option<PathBuf>) -> Self {
            Self {
                allow: true,
                send_markers: true,
                folder,
                dim: "minecraft:overworld".into(),
                players: vec![(1, true, true), (2, false, true)],
                modifiable: true,
                markers: Vec::new(),
                sent: Vec::new(),
            }
        }

        fn sent_on(&self, channel: &str) -> Vec<(u128, Vec<u8>)> {
            self.sent
                .iter()
                .filter(|s| s.1 == channel)
                .map(|s| (s.0, s.2.clone()))
                .collect()
        }

        fn player(&self, id: u128) -> Option<&(u128, bool, bool)> {
            self.players.iter().find(|p| p.0 == id)
        }
    }

    impl AnnotationHost for FakeHost {
        fn allow_annotations(&self) -> bool {
            self.allow
        }
        fn send_markers(&self) -> bool {
            self.send_markers
        }
        fn world_folder(&self, _dim: &str) -> Option<PathBuf> {
            self.folder.clone()
        }
        fn player_dimension(&self, player: u128) -> Option<String> {
            self.player(player).map(|_| self.dim.clone())
        }
        fn world_players(&self, _dim: &str) -> Vec<u128> {
            self.players.iter().map(|p| p.0).collect()
        }
        fn can_use_axiom(&self, player: u128) -> bool {
            self.player(player).is_some_and(|p| p.1)
        }
        fn has_permission(&self, player: u128, _node: &str) -> bool {
            self.player(player).is_some_and(|p| p.2)
        }
        fn can_modify_world(&self, _player: u128, _dim: &str) -> bool {
            self.modifiable
        }
        fn markers(&mut self, _dim: &str) -> Vec<MarkerEntity> {
            self.markers.clone()
        }
        fn marker_custom_data(&mut self, _dim: &str, uuid: u128) -> Option<Nbt> {
            self.markers.iter().find(|m| m.uuid == uuid).map(|m| m.data.clone())
        }
        fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
            self.sent.push((player, channel.to_owned(), data));
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("horizon-axiom-annotation-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn box_at(n: i32) -> AnnotationData {
        AnnotationData::BoxOutline { from: [n, 0, 0], to: [n + 1, 1, 1], colour: 0xFF00_FF00u32 as i32 }
    }

    fn create(uuid: u128, n: i32) -> AnnotationUpdateAction {
        AnnotationUpdateAction::Create { uuid, data: box_at(n) }
    }

    fn marker_at(uuid: u128, name: &str) -> MarkerEntity {
        let mut tag = NbtCompound::new();
        tag.insert("name".into(), Nbt::String(name.into()));
        MarkerEntity { uuid, position: [1.0, 2.0, 3.0], data: Nbt::Compound(tag) }
    }

    fn decode_marker_payload(bytes: &[u8]) -> (Vec<MarkerData>, Vec<u128>) {
        let mut r = Reader::new(bytes);
        let n = r.read_varint().unwrap();
        let changed = (0..n).map(|_| MarkerData::read(&mut r).unwrap()).collect();
        let m = r.read_varint().unwrap();
        let removed = (0..m).map(|_| r.read_uuid().unwrap()).collect();
        assert_eq!(r.remaining(), 0);
        (changed, removed)
    }

    #[test]
    fn update_packet_applies_persists_and_forwards_to_axiom_users() {
        let dir = temp_dir("update");
        let mut host = FakeHost::new(Some(dir.clone()));
        let mut state = AnnotationState::default();
        let actions = vec![create(7, 1), AnnotationUpdateAction::Delete { uuid: 99 }];
        let payload = write_action_list(&actions);

        handle_update_packet(&mut host, &mut state, 1, &payload).unwrap();

        // Only player 1 has Axiom. The delete of an unknown uuid is forwarded anyway.
        let sent = host.sent_on(CHANNEL_ANNOTATION_UPDATE);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 1);
        assert_eq!(sent[0].1, payload);

        // A fresh store reads the file back.
        let reloaded = AnnotationStore::load(Some(dir.clone())).unwrap();
        assert_eq!(reloaded.entries, vec![(7, box_at(1))]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn update_packet_without_permission_is_ignored() {
        let mut host = FakeHost::new(None);
        host.players[0].2 = false;
        let mut state = AnnotationState::default();
        handle_update_packet(&mut host, &mut state, 1, &write_action_list(&[create(1, 0)])).unwrap();
        assert!(host.sent.is_empty());
        assert!(!state.stores.contains_key("minecraft:overworld"));
    }

    #[test]
    fn update_packet_is_ignored_when_annotations_are_off_or_world_is_locked() {
        let mut host = FakeHost::new(None);
        host.allow = false;
        let mut state = AnnotationState::default();
        handle_update_packet(&mut host, &mut state, 1, &write_action_list(&[create(1, 0)])).unwrap();
        assert!(host.sent.is_empty());

        host.allow = true;
        host.modifiable = false;
        handle_update_packet(&mut host, &mut state, 1, &write_action_list(&[create(1, 0)])).unwrap();
        assert!(host.sent.is_empty());
    }

    #[test]
    fn malformed_update_packet_is_an_error() {
        let mut host = FakeHost::new(None);
        let mut state = AnnotationState::default();
        // Count 2, but only one action follows.
        let mut bytes = write_action_list(&[create(1, 0)]);
        bytes[0] = 2;
        assert!(matches!(
            handle_update_packet(&mut host, &mut state, 1, &bytes),
            Err(AnnotationError::Buf(_))
        ));
    }

    #[test]
    fn move_and_rotate_change_stored_text_only() {
        let mut host = FakeHost::new(None);
        let mut state = AnnotationState::default();
        let text = AnnotationData::Text {
            text: "a".into(),
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            direction: 0,
            fallback_yaw: 0.0,
            scale: 1.0,
            billboard_mode: 0,
            colour: 0,
            shadow: false,
        };
        let actions = vec![
            AnnotationUpdateAction::Create { uuid: 1, data: text },
            create(2, 5),
            AnnotationUpdateAction::Move { uuid: 1, to: [4.0, 5.0, 6.0] },
            AnnotationUpdateAction::Move { uuid: 2, to: [9.0, 9.0, 9.0] },
            AnnotationUpdateAction::Rotate { uuid: 1, to: [0.0, 1.0, 0.0, 0.0] },
        ];
        apply_updates(&mut host, &mut state, "minecraft:overworld", &actions).unwrap();

        let store = &state.stores["minecraft:overworld"];
        match &store.entries[0].1 {
            AnnotationData::Text { position, rotation, .. } => {
                assert_eq!(*position, [4.0, 5.0, 6.0]);
                assert_eq!(*rotation, [0.0, 1.0, 0.0, 0.0]);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(store.entries[1].1, box_at(5));
    }

    #[test]
    fn send_all_clears_then_recreates_for_joining_axiom_user() {
        let mut host = FakeHost::new(None);
        let mut state = AnnotationState::default();
        apply_updates(&mut host, &mut state, "minecraft:overworld", &[create(1, 0), create(2, 3)]).unwrap();
        host.sent.clear();

        on_player_join(&mut host, &mut state, "minecraft:overworld", 1).unwrap();
        let sent = host.sent_on(CHANNEL_ANNOTATION_UPDATE);
        assert_eq!(sent.len(), 1);
        let actions = read_action_list(&mut Reader::new(&sent[0].1)).unwrap();
        assert_eq!(actions, vec![AnnotationUpdateAction::ClearAll, create(1, 0), create(2, 3)]);

        // Player 2 has no Axiom: no annotation list.
        host.sent.clear();
        on_player_join(&mut host, &mut state, "minecraft:overworld", 2).unwrap();
        assert!(host.sent_on(CHANNEL_ANNOTATION_UPDATE).is_empty());
    }

    #[test]
    fn annotations_are_disabled_by_config() {
        let mut host = FakeHost::new(None);
        host.allow = false;
        let mut state = AnnotationState::default();
        send_all(&mut host, &mut state, "minecraft:overworld", 1).unwrap();
        apply_updates(&mut host, &mut state, "minecraft:overworld", &[create(1, 0)]).unwrap();
        assert!(host.sent.is_empty());
        assert!(state.stores.is_empty());
    }

    #[test]
    fn marker_request_answers_with_uuid_and_nbt() {
        let mut host = FakeHost::new(None);
        let marker = marker_at(55, "arena");
        host.markers.push(marker.clone());

        let mut w = Writer::new();
        w.write_uuid(55);
        w.write_varint(0);
        handle_marker_nbt_request(&mut host, 1, &w.into_bytes()).unwrap();

        let sent = host.sent_on(CHANNEL_MARKER_NBT_RESPONSE);
        assert_eq!(sent.len(), 1);
        let mut r = Reader::new(&sent[0].1);
        assert_eq!(r.read_uuid().unwrap(), 55);
        assert_eq!(r.read_nbt().unwrap(), marker.data);
        assert_eq!(r.remaining(), 0);

        // Unknown uuid: no response.
        host.sent.clear();
        let mut w = Writer::new();
        w.write_uuid(56);
        w.write_varint(0);
        handle_marker_nbt_request(&mut host, 1, &w.into_bytes()).unwrap();
        assert!(host.sent.is_empty());

        // Player without permission: nothing.
        host.players[0].2 = false;
        let mut w = Writer::new();
        w.write_uuid(55);
        w.write_varint(0);
        handle_marker_nbt_request(&mut host, 1, &w.into_bytes()).unwrap();
        assert!(host.sent.is_empty());
    }

    #[test]
    fn marker_tick_sends_changes_and_removals_only() {
        let mut host = FakeHost::new(None);
        let mut state = AnnotationState::default();
        host.markers.push(marker_at(10, "a"));
        host.markers.push(marker_at(11, "b"));

        tick_markers(&mut host, &mut state, "minecraft:overworld");
        let sent = host.sent_on(CHANNEL_MARKER_DATA);
        assert_eq!(sent.len(), 1, "only the Axiom user gets marker data");
        let (changed, removed) = decode_marker_payload(&sent[0].1);
        assert_eq!(changed.len(), 2);
        assert_eq!(changed[0].name.as_deref(), Some("a"));
        assert!(removed.is_empty());

        // Nothing changed: no packet.
        host.sent.clear();
        tick_markers(&mut host, &mut state, "minecraft:overworld");
        assert!(host.sent.is_empty());

        // Marker 10 disappears, marker 11 moves.
        host.markers.retain(|m| m.uuid != 10);
        host.markers[0].position = [9.0, 9.0, 9.0];
        tick_markers(&mut host, &mut state, "minecraft:overworld");
        let (changed, removed) = decode_marker_payload(&host.sent_on(CHANNEL_MARKER_DATA)[0].1);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].uuid, 11);
        assert_eq!(changed[0].position, [9.0, 9.0, 9.0]);
        assert_eq!(removed, vec![10]);

        // The join snapshot holds the markers still known.
        host.sent.clear();
        on_player_join(&mut host, &mut state, "minecraft:overworld", 2).unwrap();
        let (changed, removed) = decode_marker_payload(&host.sent_on(CHANNEL_MARKER_DATA)[0].1);
        assert_eq!(changed.iter().map(|m| m.uuid).collect::<Vec<_>>(), vec![11]);
        assert!(removed.is_empty());
    }

    #[test]
    fn marker_sync_can_be_disabled() {
        let mut host = FakeHost::new(None);
        host.send_markers = false;
        host.markers.push(marker_at(10, "a"));
        let mut state = AnnotationState::default();
        tick_markers(&mut host, &mut state, "minecraft:overworld");
        assert!(host.sent.is_empty());
        assert!(state.markers.is_empty());
    }

    #[test]
    fn store_file_roundtrips_and_skips_bad_entries() {
        let dir = temp_dir("store");
        let entries = vec![(1u128, box_at(1)), (2u128, box_at(2))];
        {
            let mut store = AnnotationStore::load(Some(dir.clone())).unwrap();
            store.entries = entries.clone();
            store.persist().unwrap();
        }
        assert_eq!(AnnotationStore::load(Some(dir.clone())).unwrap().entries, entries);

        // Rewrite the file with a broken second entry (unknown annotation type 77).
        let mut good = Writer::new();
        box_at(1).write(&mut good);
        let mut w = Writer::new();
        w.write_varint(2);
        w.write_uuid(1);
        w.write_byte_array(&good.into_bytes());
        w.write_uuid(2);
        w.write_byte_array(&[77u8, 0]);
        fs::write(dir.join(ANNOTATION_DIR).join(ANNOTATION_FILE), w.into_bytes()).unwrap();
        assert_eq!(AnnotationStore::load(Some(dir.clone())).unwrap().entries, vec![(1, box_at(1))]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_file_header_is_an_error_not_an_empty_store() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(dir.join(ANNOTATION_DIR)).unwrap();
        fs::write(dir.join(ANNOTATION_DIR).join(ANNOTATION_FILE), [0xFFu8, 0xFF, 0xFF, 0xFF, 0x0F, 0x01]).unwrap();
        assert!(AnnotationStore::load(Some(dir.clone())).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn in_memory_store_without_folder_persists_nothing() {
        let store = AnnotationStore { folder: None, entries: vec![(1, box_at(0))] };
        store.persist().unwrap();
    }
}
