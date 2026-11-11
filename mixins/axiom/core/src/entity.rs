// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/SpawnEntityPacketListener.java,
// packet/impl/ManipulateEntityPacketListener.java, packet/impl/DeleteEntityPacketListener.java, NbtSanitization.java,
// event/AxiomSpawnEntityEvent.java, event/AxiomManipulateEntityEvent.java, event/AxiomAfterManipulateEntityEvent.java,
// event/AxiomRemoveEntityEvent.java, paperapi/entity/ImplAxiomHiddenEntities.java, paperapi/AxiomEntityAPI.java
//
//! Entity spawn / manipulate / delete packets, NBT sanitizing, and the hidden-gizmo list.
//!
//! Wire layouts (FriendlyByteBuf conventions, as in `buf`):
//! - spawn:      varint count, then per entry: uuid, f64 x, f64 y, f64 z, f32 yaw, f32 pitch,
//!               bool + uuid (copy-from, nullable), NBT (protocol-dependent, see `EntityHost::read_tag_unknown`).
//! - manipulate: varint count, then per entry: uuid, i8 flags, [if flags >= 0: u8 relative bits as i8 flags
//!               (see `Relative`), f64 x, f64 y, f64 z, f32 yaw, f32 pitch], NBT, varint passenger mode,
//!               [if mode is ADD_LIST or REMOVE_LIST: varint count + uuids].
//! - delete:     varint count, then uuids.
//!
//! No Horizon types. The game glue implements `EntityHost`; everything else is plain data and logic.

use std::collections::{BTreeMap, BTreeSet};

use crate::buf::{BufError, Nbt, Reader, Writer};

/// Channel for the "ignore display entities" payload (`axiom:ignore_display_entities`).
pub const IGNORE_DISPLAY_CHANNEL: &str = "axiom:ignore_display_entities";
/// Permission nodes (`AxiomPermission`, entity group).
pub const PERM_ENTITY_SPAWN: &str = "axiom.entity.spawn";
pub const PERM_ENTITY_MANIPULATE: &str = "axiom.entity.manipulate";
pub const PERM_ENTITY_DELETE: &str = "axiom.entity.delete";

/// Vanilla entity type ids that matter here.
pub const TYPE_PLAYER: &str = "minecraft:player";
pub const TYPE_MARKER: &str = "minecraft:marker";

/// Keys kept at the root of an entity by `sanitize_entity`, in the order of the Java `Set.of` (`NbtSanitization`).
/// Generated from the Java source; `allowed_keys_count_matches_java` guards the count.
const ALLOWED_KEYS: &[&str] = &[
    "id",
    "Pos",
    "Rotation",
    "Invulnerable",
    "CustomName",
    "CustomNameVisible",
    "Silent",
    "NoGravity",
    "Glowing",
    "Tags",
    "Passengers",
    "ArmorItems",
    "HandItems",
    "Small",
    "ShowArms",
    "DisabledSlots",
    "NoBasePlate",
    "Marker",
    "Pose",
    "data",
    "transformation",
    "interpolation_duration",
    "start_interpolation",
    "teleport_duration",
    "billboard",
    "view_range",
    "shadow_radius",
    "shadow_strength",
    "width",
    "height",
    "glow_color_override",
    "brightness",
    "line_width",
    "text_opacity",
    "background",
    "shadow",
    "see_through",
    "default_background",
    "alignment",
    "text",
    "block_state",
    "item",
    "item_display",
];

/// A position as Java's `Vec3` (x, y, z).
pub type Pos = (f64, f64, f64);

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

/// `Math.round(float)`: floor(x + 1/2), computed in f64 so that `0.49999997f` rounds to 0 as in Java.
pub fn java_round(f: f32) -> i32 {
    (f as f64 + 0.5).floor() as i32
}

/// `BlockPos.containing(x, y, z)`: per-axis floor.
pub fn block_of(p: Pos) -> (i32, i32, i32) {
    (p.0.floor() as i32, p.1.floor() as i32, p.2.floor() as i32)
}

/// `net.minecraft.world.level.block.Rotation` values, in vanilla enum order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    None,
    Clockwise90,
    Clockwise180,
    CounterClockwise90,
}

impl Turn {
    /// `ROTATION_VALUES[rotations & 3]`.
    pub fn from_quarter_turns(q: i32) -> Turn {
        match q & 3 {
            0 => Turn::None,
            1 => Turn::Clockwise90,
            2 => Turn::Clockwise180,
            _ => Turn::CounterClockwise90,
        }
    }
}

/// Entity classes that extend `HangingEntity` in vanilla 26.3.
pub fn is_hanging_type(type_id: &str) -> bool {
    matches!(
        type_id,
        "minecraft:painting"
            | "minecraft:item_frame"
            | "minecraft:glow_item_frame"
            | "minecraft:leash_knot"
    )
}

/// `ItemFrame` and its subclass `GlowItemFrame`.
pub fn is_item_frame_type(type_id: &str) -> bool {
    matches!(
        type_id,
        "minecraft:item_frame" | "minecraft:glow_item_frame"
    )
}

/// The pose correction Axiom applies to hanging entities before moving them.
/// Returns the quarter turn for `HangingEntity.rotate` and the item-frame rotation delta (only used for
/// item frames whose facing is on the Y axis). Both use `changedYaw = targetYaw - currentYaw`.
pub fn hanging_adjust(current_yaw: f32, target_yaw: f32) -> (Turn, i32) {
    let changed = target_yaw - current_yaw;
    let turn = Turn::from_quarter_turns(java_round(changed / 90.0));
    (turn, java_round(changed / 45.0))
}

/// Relative-movement bits, in vanilla `Relative` ordinal order (bit = 1 << ordinal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relative {
    X,
    Y,
    Z,
    YRot,
    XRot,
}

/// Set of `Relative` flags as sent on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelativeSet(u8);

impl RelativeSet {
    pub fn from_bits(bits: u8) -> Self {
        RelativeSet(bits)
    }
    pub fn contains(self, r: Relative) -> bool {
        self.0 & (1 << r as u8) != 0
    }
}

/// Entity type filter from `canEntityBeManipulated`. Config ids that the registry does not know must be
/// dropped by the glue (Java uses `ifPresent`), so an unknown-only whitelist behaves as an empty one.
#[derive(Clone, Debug, Default)]
pub struct EntityTypeFilter {
    whitelist: BTreeSet<String>,
    blacklist: BTreeSet<String>,
}

impl EntityTypeFilter {
    pub fn new<W, B>(whitelist: W, blacklist: B) -> Self
    where
        W: IntoIterator<Item = String>,
        B: IntoIterator<Item = String>,
    {
        EntityTypeFilter {
            whitelist: whitelist.into_iter().collect(),
            blacklist: blacklist.into_iter().collect(),
        }
    }

    /// `AxiomPaper.canEntityBeManipulated`: players never; whitelist if non-empty; never blacklisted.
    pub fn allows(&self, type_id: &str) -> bool {
        if type_id == TYPE_PLAYER {
            return false;
        }
        if !self.whitelist.is_empty() && !self.whitelist.contains(type_id) {
            return false;
        }
        !self.blacklist.contains(type_id)
    }
}

// ---------------------------------------------------------------------------------------------
// NBT helpers
// ---------------------------------------------------------------------------------------------

const MODIFY_KEY: &str = "axiom:modify";

fn empty_compound() -> Nbt {
    Nbt::Compound(BTreeMap::new())
}

/// `NbtSanitization.sanitizeEntity`: keep only `ALLOWED_KEYS` at the root, then sanitize every compound
/// in the root `Passengers` list (recursively). Nested compounds under allowed keys are not touched.
/// `disabled` mirrors the `disable-entity-sanitization` config flag.
pub fn sanitize_entity(root: &mut Nbt, disabled: bool) {
    if disabled {
        return;
    }
    sanitize_inner(root);
}

fn sanitize_inner(root: &mut Nbt) {
    let Nbt::Compound(map) = root else {
        return;
    };
    map.retain(|key, _| ALLOWED_KEYS.contains(&key.as_str()));
    if let Some(Nbt::List(items)) = map.get_mut("Passengers") {
        for item in items.iter_mut() {
            if matches!(item, Nbt::Compound(_)) {
                sanitize_inner(item);
            }
        }
    }
}

/// Vanilla `CompoundTag.merge(other)`: deep merge, `other` wins on leaves, copies values.
/// Used for the copy-from snapshot on spawn. (Recalled from vanilla; verify against 26.3 source.)
pub fn merge_vanilla(target: &mut BTreeMap<String, Nbt>, other: &BTreeMap<String, Nbt>) {
    for (key, value) in other {
        match (value, target.get_mut(key)) {
            (Nbt::Compound(src), Some(Nbt::Compound(dst))) => merge_vanilla(dst, src),
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Axiom's own `ManipulateEntityPacketListener.merge(left, right)`.
/// - `axiom:modify` at this level: the right side replaces the left completely (key removed from it).
/// - an empty compound in right removes the key from left.
/// - a compound present in left is merged recursively; otherwise the right compound is copied in
///   with a top-level `axiom:modify` stripped.
/// - anything else replaces the left value.
pub fn axiom_merge(
    mut left: BTreeMap<String, Nbt>,
    mut right: BTreeMap<String, Nbt>,
) -> BTreeMap<String, Nbt> {
    if right.contains_key(MODIFY_KEY) {
        right.remove(MODIFY_KEY);
        return right;
    }
    for (key, tag) in right {
        match tag {
            Nbt::Compound(compound) => {
                if compound.is_empty() {
                    left.remove(&key);
                    continue;
                }
                match left.get_mut(&key) {
                    Some(Nbt::Compound(child)) => {
                        let taken = std::mem::take(child);
                        *child = axiom_merge(taken, compound);
                    }
                    _ => {
                        let mut copied = compound;
                        copied.remove(MODIFY_KEY);
                        left.insert(key, Nbt::Compound(copied));
                    }
                }
            }
            other => {
                left.insert(key, other);
            }
        }
    }
    left
}

// ---------------------------------------------------------------------------------------------
// Host trait: everything the engine must provide
// ---------------------------------------------------------------------------------------------

/// One entity of a freshly loaded (detached) tree, as `EntityType.loadEntityRecursive` builds it.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityNode {
    pub uuid: u128,
    pub type_id: String,
    /// Yaw as loaded from the NBT `Rotation` (before any pose change).
    pub yaw: f32,
    pub passengers: Vec<EntityNode>,
}

/// Engine operations used by the entity packets. Players and entities are identified by UUID.
/// Dimensions are ids such as `minecraft:overworld`. Booleans named `cancelled` are the event result.
pub trait EntityHost {
    // ----- permissions, config, connection -----
    /// `canUseAxiom(player)`: the player has an active Axiom session.
    fn is_axiom_active(&self, player: u128) -> bool;
    /// `hasPermission(player, node)`: includes the OP bypass, does not check the session.
    fn has_axiom_permission(&self, player: u128, node: &str) -> bool;
    /// `canModifyWorld(player, world)`: world regexes plus `AxiomModifyWorldEvent`.
    fn can_modify_world(&self, player: u128, dim: &str) -> bool;
    /// `getPacketCollectionReadLimit()`.
    fn packet_collection_read_limit(&self) -> usize;
    /// Config `disable-entity-sanitization`.
    fn disable_entity_sanitization(&self) -> bool;
    /// Whitelist and blacklist from config.
    fn entity_type_filter(&self) -> &EntityTypeFilter;
    /// `UnknownVersionHelper.readTagUnknown`: read one entry's NBT in the sending player's protocol.
    /// `Ok(None)` for TAG_End (Java null). Only a compound is valid when `Some`.
    fn read_tag_unknown(&self, player: u128, reader: &mut Reader) -> Result<Option<Nbt>, BufError>;
    /// Online players with an active Axiom session (used by the hidden-display resend).
    fn active_axiom_players(&self) -> Vec<u128>;
    /// Send a plugin-channel payload to one player.
    fn send_payload(&mut self, player: u128, channel: &str, data: &[u8]);

    // ----- world and integrations -----
    /// `canPlaceBlock(player, location)` (Integration hook).
    fn can_place_block(&self, player: u128, dim: &str, x: i32, y: i32, z: i32) -> bool;
    /// `canBreakBlock(player, block)` (Integration hook).
    fn can_break_block(&self, player: u128, dim: &str, x: i32, y: i32, z: i32) -> bool;
    /// `Level.isInSpawnableBounds(BlockPos)`: horizontal and vertical world limits.
    fn is_in_spawnable_bounds(&self, x: i32, y: i32, z: i32) -> bool;

    // ----- events (return true when cancelled) -----
    fn fire_spawn(&mut self, player: u128, uuid: u128) -> bool;
    fn fire_manipulate(&mut self, player: u128, uuid: u128) -> bool;
    fn fire_after_manipulate(&mut self, player: u128, uuid: u128);
    fn fire_remove(&mut self, player: u128, uuid: u128) -> bool;

    // ----- entity lookup and state -----
    /// `ServerLevel.getEntity(uuid) != null` in `dim`.
    fn has_entity(&self, dim: &str, uuid: u128) -> bool;
    /// Vanilla type id of an entity, or None if unknown.
    fn entity_type_id(&self, uuid: u128) -> Option<String>;
    /// `EntityType.canSerialize()`.
    fn type_can_serialize(&self, type_id: &str) -> bool;
    fn position(&self, uuid: u128) -> Pos;
    fn yaw(&self, uuid: u128) -> f32;
    fn pitch(&self, uuid: u128) -> f32;
    fn is_passenger(&self, uuid: u128) -> bool;
    /// `getVehicle()`.
    fn vehicle(&self, uuid: u128) -> Option<u128>;
    /// Direct passengers only (`getPassengers()`).
    fn passengers(&self, uuid: u128) -> Vec<u128>;
    fn is_removed(&self, uuid: u128) -> bool;
    /// `ItemFrame.getDirection().getAxis() == Y`.
    fn item_frame_on_y_axis(&self, uuid: u128) -> bool;
    /// `Entity.saveAsPassenger` into a compound; None if the entity is missing or the save reports false.
    fn entity_as_passenger_nbt(&mut self, dim: &str, uuid: u128) -> Option<Nbt>;

    // ----- mutations -----
    /// Vanilla `EntityType.loadEntityRecursive` *without* any filtering: builds the tree detached (not in the
    /// level). The root gets `root_uuid`, every passenger gets a fresh random UUID. Passengers ride the root
    /// as vanilla does. None if the root id is unknown or the tag cannot be loaded.
    fn load_entity_tree(&mut self, dim: &str, tag: &Nbt, root_uuid: u128) -> Option<EntityNode>;
    /// `HangingEntity.rotate(turn)`.
    fn set_hanging_turn(&mut self, uuid: u128, turn: Turn);
    /// `itemFrame.setRotation(itemFrame.getRotation() - delta)`, with vanilla wrapping.
    fn adjust_item_frame_rotation(&mut self, uuid: u128, delta: i32);
    /// `snapTo(x, y, z, yaw, pitch)`.
    fn snap_to(&mut self, uuid: u128, pos: Pos, yaw: f32, pitch: f32);
    /// `setYHeadRot(yaw)`.
    fn set_head_yaw(&mut self, uuid: u128, yaw: f32);
    /// `setYRot(yaw)`.
    fn set_yaw(&mut self, uuid: u128, yaw: f32);
    /// `setXRot(pitch)`.
    fn set_pitch(&mut self, uuid: u128, pitch: f32);
    /// `setPosRaw(x, y, z)`.
    fn set_pos_raw(&mut self, uuid: u128, pos: Pos);
    /// `saveWithoutId(output)`.
    fn save_without_id(&mut self, uuid: u128) -> Nbt;
    /// `entity.load(input)`.
    fn load_nbt(&mut self, uuid: u128, tag: &Nbt);
    /// `ejectPassengers()`.
    fn eject_passengers(&mut self, uuid: u128);
    /// `startRiding(vehicle, force = true, triggerPassengerEvents = false)`.
    fn start_riding(&mut self, passenger: u128, vehicle: u128);
    /// `stopRiding()`.
    fn stop_riding(&mut self, passenger: u128);
    /// `tryAddFreshEntityWithPassengers(entity)`: true if the tree was added to the level.
    fn add_fresh_entity_with_passengers(&mut self, dim: &str, uuid: u128) -> bool;
    /// `discard()` / `remove(DISCARDED)`. Also used on detached trees that were never added.
    fn remove(&mut self, uuid: u128);
}

// ---------------------------------------------------------------------------------------------
// Wire decoding
// ---------------------------------------------------------------------------------------------

type ReadTag<'a> = &'a mut dyn FnMut(&mut Reader) -> Result<Option<Nbt>, BufError>;

#[derive(Clone, Debug, PartialEq)]
pub struct SpawnEntry {
    pub uuid: u128,
    pub pos: Pos,
    pub yaw: f32,
    pub pitch: f32,
    pub copy_from: Option<u128>,
    /// None when the client sent TAG_End (Java: an empty compound is used).
    pub tag: Option<Nbt>,
}

fn read_spawn_entry(r: &mut Reader, read_tag: ReadTag) -> Result<SpawnEntry, BufError> {
    let uuid = r.read_uuid()?;
    let pos = (r.read_f64()?, r.read_f64()?, r.read_f64()?);
    let yaw = r.read_f32()?;
    let pitch = r.read_f32()?;
    let copy_from = if r.read_bool()? {
        Some(r.read_uuid()?)
    } else {
        None
    };
    let tag = read_tag(r)?;
    Ok(SpawnEntry {
        uuid,
        pos,
        yaw,
        pitch,
        copy_from,
        tag,
    })
}

pub fn read_spawn_entries(
    r: &mut Reader,
    limit: usize,
    read_tag: ReadTag,
) -> Result<Vec<SpawnEntry>, BufError> {
    r.read_list(limit, |r| read_spawn_entry(r, &mut *read_tag))
}

/// Passenger change requested by a manipulate entry. Ordinals match the Java enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassengerMode {
    None,
    RemoveAll,
    AddList,
    RemoveList,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub relative: RelativeSet,
    pub pos: Pos,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManipulateEntry {
    pub uuid: u128,
    /// Present when the flags byte is non-negative.
    pub placement: Option<Placement>,
    pub merge: Option<Nbt>,
    pub passenger_mode: PassengerMode,
    pub passengers: Vec<u128>,
}

fn read_manipulate_entry(
    r: &mut Reader,
    limit: usize,
    read_tag: ReadTag,
) -> Result<ManipulateEntry, BufError> {
    let uuid = r.read_uuid()?;
    let flags = r.read_i8()?;
    let placement = if flags >= 0 {
        let relative = RelativeSet::from_bits(flags as u8);
        let pos = (r.read_f64()?, r.read_f64()?, r.read_f64()?);
        let yaw = r.read_f32()?;
        let pitch = r.read_f32()?;
        Some(Placement {
            relative,
            pos,
            yaw,
            pitch,
        })
    } else {
        None
    };
    let merge = read_tag(r)?;
    let passenger_mode = r.read_enum(&[
        PassengerMode::None,
        PassengerMode::RemoveAll,
        PassengerMode::AddList,
        PassengerMode::RemoveList,
    ])?;
    let passengers = match passenger_mode {
        PassengerMode::AddList | PassengerMode::RemoveList => {
            r.read_list(limit, |r| r.read_uuid())?
        }
        _ => Vec::new(),
    };
    Ok(ManipulateEntry {
        uuid,
        placement,
        merge,
        passenger_mode,
        passengers,
    })
}

pub fn read_manipulate_entries(
    r: &mut Reader,
    limit: usize,
    read_tag: ReadTag,
) -> Result<Vec<ManipulateEntry>, BufError> {
    r.read_list(limit, |r| read_manipulate_entry(r, limit, &mut *read_tag))
}

pub fn read_delete_uuids(r: &mut Reader, limit: usize) -> Result<Vec<u128>, BufError> {
    r.read_list(limit, |r| r.read_uuid())
}

// ---------------------------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------------------------

/// `canUseAxiom(player, node)`: active session and the permission node (OP bypass lives in the host).
fn can_use<H: EntityHost + ?Sized>(host: &H, player: u128, node: &str) -> bool {
    host.is_axiom_active(player) && host.has_axiom_permission(player, node)
}

/// Permission and world checks run before the payload is read, as in Java. A refused packet is ignored.
fn gate<H: EntityHost + ?Sized>(host: &H, player: u128, dim: &str, node: &str) -> bool {
    can_use(host, player, node) && host.can_modify_world(player, dim)
}

/// Handle `axiom:spawn_entity` (SpawnEntityPacketListener).
/// One entity of a tree built by [`build_detached_tree`]: its UUID, the UUID of the entity it rides in the tree
/// (None for the root) and its data as saved, without the `Passengers` list.
#[derive(Debug, Clone, PartialEq)]
pub struct DetachedEntity {
    pub uuid: u128,
    pub parent: Option<u128>,
    pub data: Nbt,
}

/// Java `EntityType.loadEntityRecursive` for a spawn: the tree of `tag` as detached entities. The root gets
/// `root_uuid`; every passenger gets a UUID from `fresh`, and a passenger without a valid `id` is left out, as Java
/// does. Returns the tree (types and yaw, for pruning and placing) and the flat list, root first, in which each
/// entity names its parent. None when the root has no `id`.
pub fn build_detached_tree(
    tag: &Nbt,
    root_uuid: u128,
    fresh: &mut dyn FnMut() -> u128,
) -> Option<(EntityNode, Vec<DetachedEntity>)> {
    let mut flat = Vec::new();
    let node = detach(tag, root_uuid, None, fresh, &mut flat)?;
    Some((node, flat))
}

fn detach(
    tag: &Nbt,
    uuid: u128,
    parent: Option<u128>,
    fresh: &mut dyn FnMut() -> u128,
    flat: &mut Vec<DetachedEntity>,
) -> Option<EntityNode> {
    let Nbt::Compound(map) = tag else {
        return None;
    };
    let Some(Nbt::String(type_id)) = map.get("id") else {
        return None;
    };
    let yaw = match map.get("Rotation") {
        Some(Nbt::List(items)) => match items.first() {
            Some(Nbt::Float(yaw)) => *yaw,
            _ => 0.0,
        },
        _ => 0.0,
    };
    let mut data = map.clone();
    let passengers_tag = data.remove("Passengers");
    flat.push(DetachedEntity {
        uuid,
        parent,
        data: Nbt::Compound(data),
    });
    let mut passengers = Vec::new();
    if let Some(Nbt::List(items)) = passengers_tag {
        for item in &items {
            // Only an entity that loads asks for a UUID.
            if !matches!(item, Nbt::Compound(m) if matches!(m.get("id"), Some(Nbt::String(_)))) {
                continue;
            }
            let child = fresh();
            if let Some(node) = detach(item, child, Some(uuid), fresh, flat) {
                passengers.push(node);
            }
        }
    }
    Some(EntityNode {
        uuid,
        type_id: type_id.clone(),
        yaw,
        passengers,
    })
}

pub fn handle_spawn<H: EntityHost>(
    host: &mut H,
    player: u128,
    dim: &str,
    payload: &[u8],
) -> Result<(), BufError> {
    if !gate(host, player, dim, PERM_ENTITY_SPAWN) {
        return Ok(());
    }
    let limit = host.packet_collection_read_limit();
    let entries = {
        let mut r = Reader::new(payload);
        let host_ref: &H = host;
        read_spawn_entries(&mut r, limit, &mut |r| host_ref.read_tag_unknown(player, r))?
    };
    for entry in entries {
        spawn_one(host, player, dim, entry);
    }
    Ok(())
}

fn spawn_one<H: EntityHost>(host: &mut H, player: u128, dim: &str, entry: SpawnEntry) {
    let pos = entry.pos;
    let block = block_of(pos);
    if !host.is_in_spawnable_bounds(block.0, block.1, block.2) {
        return;
    }
    if !host.can_place_block(player, dim, block.0, block.1, block.2) {
        return;
    }
    if host.has_entity(dim, entry.uuid) {
        return;
    }

    let mut tag = entry.tag.unwrap_or_else(empty_compound);
    sanitize_entity(&mut tag, host.disable_entity_sanitization());

    // Copy-from is merged after sanitizing, so the copied snapshot is not sanitized (as in Java).
    if let Some(src) = entry.copy_from {
        if let Some(mut saved) = host.entity_as_passenger_nbt(dim, src) {
            if let Nbt::Compound(saved_map) = &mut saved {
                saved_map.remove("Dimension");
                if let Nbt::Compound(target) = &mut tag {
                    merge_vanilla(target, saved_map);
                }
            }
        }
    }

    if !matches!(&tag, Nbt::Compound(m) if m.contains_key("id")) {
        return;
    }

    let Some(root) = host.load_entity_tree(dim, &tag, entry.uuid) else {
        return;
    };
    let filter = host.entity_type_filter().clone();
    let Some(root) = prune_tree(host, root, &filter) else {
        return;
    };
    place_tree(host, &root, entry.pos, entry.yaw, entry.pitch);

    let root_uuid = root.uuid;
    if !host.add_fresh_entity_with_passengers(dim, root_uuid) {
        // Java leaves the detached tree alone; removing it here only frees the glue's bookkeeping.
        remove_tree(host, &root);
        return;
    }
    let cancelled = host.fire_spawn(player, root_uuid);
    if cancelled || host.is_removed(root_uuid) {
        discard_with_passengers(host, root_uuid);
    }
}

/// The per-entity predicate of `loadEntityRecursive`: drop disallowed entities together with their
/// passenger subtree; a disallowed root drops everything.
fn prune_tree<H: EntityHost>(
    host: &mut H,
    node: EntityNode,
    filter: &EntityTypeFilter,
) -> Option<EntityNode> {
    if !filter.allows(&node.type_id) {
        remove_tree(host, &node);
        return None;
    }
    let mut kept = node;
    let children = std::mem::take(&mut kept.passengers);
    kept.passengers = children
        .into_iter()
        .filter_map(|c| prune_tree(host, c, filter))
        .collect();
    Some(kept)
}

/// Pose every kept entity to the spawn position, in pre-order (root first), as the Java predicate does.
fn place_tree<H: EntityHost>(host: &mut H, node: &EntityNode, pos: Pos, yaw: f32, pitch: f32) {
    if is_hanging_type(&node.type_id) {
        let (turn, delta) = hanging_adjust(node.yaw, yaw);
        host.set_hanging_turn(node.uuid, turn);
        if is_item_frame_type(&node.type_id) && host.item_frame_on_y_axis(node.uuid) {
            host.adjust_item_frame_rotation(node.uuid, delta);
        }
    }
    host.snap_to(node.uuid, pos, yaw, pitch);
    let body_yaw = host.yaw(node.uuid);
    host.set_head_yaw(node.uuid, body_yaw);
    for child in &node.passengers {
        place_tree(host, child, pos, yaw, pitch);
    }
}

fn remove_tree<H: EntityHost>(host: &mut H, node: &EntityNode) {
    for child in &node.passengers {
        remove_tree(host, child);
    }
    host.remove(node.uuid);
}

/// Discard every indirect passenger, then the entity itself (`getIndirectPassengers` + `discard`).
fn discard_with_passengers<H: EntityHost>(host: &mut H, uuid: u128) {
    for p in indirect_passengers(host, uuid) {
        host.remove(p);
    }
    host.remove(uuid);
}

fn indirect_passengers<H: EntityHost>(host: &H, uuid: u128) -> Vec<u128> {
    let mut out = Vec::new();
    let mut stack = host.passengers(uuid);
    while let Some(p) = stack.pop() {
        out.push(p);
        stack.extend(host.passengers(p));
    }
    out
}

/// `getSelfAndPassengers().anyMatch(e -> e == target)`.
fn subtree_contains<H: EntityHost>(host: &H, root: u128, target: u128) -> bool {
    root == target || indirect_passengers(host, root).contains(&target)
}

fn type_of<H: EntityHost + ?Sized>(host: &H, uuid: u128) -> Option<String> {
    host.entity_type_id(uuid)
}

fn has_direct_player_passenger<H: EntityHost + ?Sized>(host: &H, uuid: u128) -> bool {
    host.passengers(uuid)
        .into_iter()
        .any(|p| type_of(host, p).as_deref() == Some(TYPE_PLAYER))
}

/// `canManipulatePassengers`: serializable and neither a player nor a marker. False for unknown entities.
fn can_manipulate_passengers<H: EntityHost + ?Sized>(host: &H, uuid: u128) -> bool {
    match type_of(host, uuid) {
        Some(t) => host.type_can_serialize(&t) && t != TYPE_PLAYER && t != TYPE_MARKER,
        None => false,
    }
}

/// `passenger.hasPassenger(cannotManipulatePassengers)` (direct passengers).
fn has_direct_unmanipulable_passenger<H: EntityHost + ?Sized>(host: &H, uuid: u128) -> bool {
    host.passengers(uuid)
        .into_iter()
        .any(|p| !can_manipulate_passengers(host, p))
}

/// Handle `axiom:manipulate_entity` (ManipulateEntityPacketListener).
pub fn handle_manipulate<H: EntityHost>(
    host: &mut H,
    player: u128,
    dim: &str,
    payload: &[u8],
) -> Result<(), BufError> {
    if !gate(host, player, dim, PERM_ENTITY_MANIPULATE) {
        return Ok(());
    }
    let limit = host.packet_collection_read_limit();
    let entries = {
        let mut r = Reader::new(payload);
        let host_ref: &H = host;
        read_manipulate_entries(&mut r, limit, &mut |r| host_ref.read_tag_unknown(player, r))?
    };
    for entry in entries {
        manipulate_one(host, player, dim, entry);
    }
    Ok(())
}

fn manipulate_one<H: EntityHost>(host: &mut H, player: u128, dim: &str, entry: ManipulateEntry) {
    let uuid = entry.uuid;
    if !host.has_entity(dim, uuid) {
        return;
    }
    let Some(type_id) = type_of(host, uuid) else {
        return;
    };
    if type_id == TYPE_PLAYER || has_direct_player_passenger(host, uuid) {
        return;
    }
    if !host.entity_type_filter().allows(&type_id) {
        return;
    }

    let position = host.position(uuid);
    let b = block_of(position);
    if !host.can_place_block(player, dim, b.0, b.1, b.2) {
        return;
    }
    if host.fire_manipulate(player, uuid) {
        return;
    }

    if let Some(merge) = entry.merge {
        if matches!(&merge, Nbt::Compound(m) if !m.is_empty()) {
            let mut merge = merge;
            sanitize_entity(&mut merge, host.disable_entity_sanitization());
            if let (Nbt::Compound(right), Nbt::Compound(current)) =
                (merge, host.save_without_id(uuid))
            {
                let merged = axiom_merge(current, right);
                host.load_nbt(uuid, &Nbt::Compound(merged));
            }
        }
    }

    host.set_pos_raw(uuid, position);

    if let Some(p) = entry.placement {
        let current = host.position(uuid);
        let new_x = if p.relative.contains(Relative::X) {
            current.0 + p.pos.0
        } else {
            p.pos.0
        };
        let new_y = if p.relative.contains(Relative::Y) {
            current.1 + p.pos.1
        } else {
            p.pos.1
        };
        let new_z = if p.relative.contains(Relative::Z) {
            current.2 + p.pos.2
        } else {
            p.pos.2
        };
        let new_yaw = if p.relative.contains(Relative::YRot) {
            host.yaw(uuid) + p.yaw
        } else {
            p.yaw
        };
        let new_pitch = if p.relative.contains(Relative::XRot) {
            host.pitch(uuid) + p.pitch
        } else {
            p.pitch
        };

        if is_hanging_type(&type_id) {
            let (turn, delta) = hanging_adjust(host.yaw(uuid), new_yaw);
            host.set_hanging_turn(uuid, turn);
            if is_item_frame_type(&type_id) && host.item_frame_on_y_axis(uuid) {
                host.adjust_item_frame_rotation(uuid, delta);
            }
        }

        let target = (new_x, new_y, new_z);
        let tb = block_of(target);
        if host.can_place_block(player, dim, tb.0, tb.1, tb.2) {
            if host.is_passenger(uuid) {
                host.set_yaw(uuid, new_yaw);
                host.set_pitch(uuid, new_pitch);
            } else {
                host.snap_to(uuid, target, new_yaw, new_pitch);
            }
        }
        host.set_head_yaw(uuid, new_yaw);
    }

    if can_manipulate_passengers(host, uuid) {
        match entry.passenger_mode {
            PassengerMode::None => {}
            PassengerMode::RemoveAll => host.eject_passengers(uuid),
            PassengerMode::AddList => {
                for p in entry.passengers {
                    add_passenger(host, player, dim, uuid, p);
                }
            }
            PassengerMode::RemoveList => {
                for p in entry.passengers {
                    remove_passenger(host, dim, uuid, p);
                }
            }
        }
    }

    host.fire_after_manipulate(player, uuid);
}

fn add_passenger<H: EntityHost>(host: &mut H, player: u128, dim: &str, vehicle: u128, p: u128) {
    if !host.has_entity(dim, p) || host.is_passenger(p) {
        return;
    }
    if !can_manipulate_passengers(host, p) || has_direct_unmanipulable_passenger(host, p) {
        return;
    }
    let Some(t) = type_of(host, p) else {
        return;
    };
    if !host.entity_type_filter().allows(&t) {
        return;
    }
    // Refuse a mount loop: the vehicle must not be the passenger itself or one of its passengers.
    if subtree_contains(host, p, vehicle) {
        return;
    }
    let b = block_of(host.position(p));
    if !host.can_place_block(player, dim, b.0, b.1, b.2) {
        return;
    }
    host.start_riding(p, vehicle);
}

fn remove_passenger<H: EntityHost>(host: &mut H, dim: &str, vehicle: u128, p: u128) {
    if p == vehicle || !host.has_entity(dim, p) {
        return;
    }
    if !can_manipulate_passengers(host, p) || has_direct_unmanipulable_passenger(host, p) {
        return;
    }
    let Some(t) = type_of(host, p) else {
        return;
    };
    if !host.entity_type_filter().allows(&t) {
        return;
    }
    if host.vehicle(p) == Some(vehicle) {
        host.stop_riding(p);
    }
}

/// Handle `axiom:delete_entity` (DeleteEntityPacketListener).
pub fn handle_delete<H: EntityHost>(
    host: &mut H,
    player: u128,
    dim: &str,
    payload: &[u8],
) -> Result<(), BufError> {
    if !gate(host, player, dim, PERM_ENTITY_DELETE) {
        return Ok(());
    }
    let limit = host.packet_collection_read_limit();
    let uuids = {
        let mut r = Reader::new(payload);
        read_delete_uuids(&mut r, limit)?
    };
    for uuid in uuids {
        if !host.has_entity(dim, uuid) {
            continue;
        }
        let Some(type_id) = type_of(host, uuid) else {
            continue;
        };
        if type_id == TYPE_PLAYER || has_direct_player_passenger(host, uuid) {
            continue;
        }
        if !host.entity_type_filter().allows(&type_id) {
            continue;
        }
        let b = block_of(host.position(uuid));
        if !host.can_break_block(player, dim, b.0, b.1, b.2) {
            continue;
        }
        if host.fire_remove(player, uuid) {
            continue;
        }
        host.remove(uuid);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Hidden entities (ImplAxiomHiddenEntities / AxiomEntityAPI)
// ---------------------------------------------------------------------------------------------

/// Gizmo hiding state. Java keeps weak references to marker and display objects; here entities are
/// identified by UUID (markers) and by an opaque `key` chosen by the glue (displays, which may be custom
/// objects). The glue calls `forget_marker` / `forget_display` when the object goes away.
#[derive(Debug, Default)]
pub struct HiddenEntities {
    markers: BTreeSet<u128>,
    displays: BTreeMap<u64, u128>,
    last_sent: BTreeSet<u128>,
    resend_pending: bool,
    sent_to_a_player: bool,
}

impl HiddenEntities {
    pub fn new() -> Self {
        Self::default()
    }

    /// `AxiomEntityAPI.hideMarkerGizmo`.
    pub fn hide_marker_gizmo(&mut self, uuid: u128) {
        self.markers.insert(uuid);
    }

    pub fn is_marker_hidden(&self, uuid: u128) -> bool {
        self.markers.contains(&uuid)
    }

    pub fn forget_marker(&mut self, uuid: u128) {
        self.markers.remove(&uuid);
    }

    /// `AxiomEntityAPI.hideDisplayGizmo` and `hideCustomDisplayGizmo`: `key` identifies the owning object,
    /// `uuid` is the display entity's UUID that gets sent to clients.
    pub fn hide_display_gizmo(&mut self, key: u64, uuid: u128) {
        self.displays.insert(key, uuid);
        if self.sent_to_a_player && !self.last_sent.contains(&uuid) {
            self.resend_pending = true;
        }
    }

    pub fn forget_display(&mut self, key: u64) {
        self.displays.remove(&key);
    }

    /// `ImplAxiomHiddenEntities.tick()`: resend the list to all Axiom-active players when it grew.
    pub fn tick<H: EntityHost>(&mut self, host: &mut H) {
        if !self.resend_pending {
            return;
        }
        self.resend_pending = false;
        self.last_sent = self.displays.values().copied().collect();
        let players = host.active_axiom_players();
        if players.is_empty() {
            self.sent_to_a_player = false;
        } else {
            self.send_all(host, &players);
        }
    }

    /// `ImplAxiomHiddenEntities.sendAll(players)`.
    pub fn send_all<H: EntityHost>(&mut self, host: &mut H, players: &[u128]) {
        if players.is_empty() {
            return;
        }
        self.sent_to_a_player = true;
        if self.displays.is_empty() {
            return;
        }
        let mut w = Writer::new();
        w.write_varint(self.displays.len() as i32);
        for uuid in self.displays.values() {
            w.write_uuid(*uuid);
        }
        let data = w.into_bytes();
        for player in players {
            host.send_payload(*player, IGNORE_DISPLAY_CHANNEL, &data);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn compound(pairs: &[(&str, Nbt)]) -> Nbt {
        Nbt::Compound(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    fn keys(n: &Nbt) -> Vec<String> {
        match n {
            Nbt::Compound(m) => m.keys().cloned().collect(),
            _ => panic!("not a compound"),
        }
    }

    #[test]
    fn allowed_keys_count_matches_java() {
        // NbtSanitization.java declares 43 keys in Set.of(...).
        assert_eq!(ALLOWED_KEYS.len(), 43);
        let mut sorted = ALLOWED_KEYS.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 43, "duplicate allow-list key");
    }

    #[test]
    fn sanitize_strips_unknown_root_keys_and_keeps_allowed() {
        let mut t = compound(&[
            ("id", Nbt::String("minecraft:armor_stand".into())),
            ("Pos", Nbt::List(vec![])),
            ("UUID", Nbt::IntArray(vec![1, 2, 3, 4])),
            ("axiom:modify", Nbt::Byte(1)),
            ("Health", Nbt::Float(20.0)),
            ("Small", Nbt::Byte(1)),
        ]);
        sanitize_entity(&mut t, false);
        assert_eq!(keys(&t), vec!["Pos", "Small", "id"]);
    }

    #[test]
    fn sanitize_recurses_into_passengers_only() {
        let inner = compound(&[("id", Nbt::String("a".into())), ("Health", Nbt::Int(1))]);
        let mut t = compound(&[
            ("id", Nbt::String("b".into())),
            ("Passengers", Nbt::List(vec![inner])),
            ("Owner", Nbt::Int(5)),
        ]);
        sanitize_entity(&mut t, false);
        assert_eq!(keys(&t), vec!["Passengers", "id"]);
        if let Some(Nbt::List(list)) = t.get("Passengers") {
            assert_eq!(keys(&list[0]), vec!["id"]);
        } else {
            panic!("missing passengers");
        }
    }

    #[test]
    fn sanitize_keeps_nested_keys_under_allowed_roots() {
        let mut t = compound(&[(
            "transformation",
            compound(&[("scale", Nbt::Int(2)), ("axiom:modify", Nbt::Byte(1))]),
        )]);
        sanitize_entity(&mut t, false);
        match t.get("transformation") {
            Some(Nbt::Compound(m)) => assert_eq!(m.len(), 2),
            _ => panic!("transformation lost"),
        }
    }

    #[test]
    fn sanitize_disabled_keeps_everything() {
        let mut t = compound(&[("Health", Nbt::Int(1))]);
        sanitize_entity(&mut t, true);
        assert_eq!(keys(&t), vec!["Health"]);
    }

    #[test]
    fn java_round_is_half_up() {
        assert_eq!(java_round(0.5), 1);
        assert_eq!(java_round(-0.5), 0);
        assert_eq!(java_round(-1.5), -1);
        assert_eq!(java_round(0.49999997), 0);
        assert_eq!(java_round(2.5), 3);
    }

    #[test]
    fn block_of_floors_negative_coordinates() {
        assert_eq!(block_of((-0.5, 64.0, -0.0)), (-1, 64, 0));
    }

    #[test]
    fn hanging_adjust_turns_and_frame_delta() {
        assert_eq!(hanging_adjust(0.0, 90.0), (Turn::Clockwise90, 2));
        assert_eq!(hanging_adjust(0.0, -90.0), (Turn::CounterClockwise90, -2));
        assert_eq!(hanging_adjust(10.0, 10.0), (Turn::None, 0));
        assert_eq!(hanging_adjust(0.0, 180.0), (Turn::Clockwise180, 4));
        // -270 quarter turns: (-3) & 3 == 1 -> clockwise 90
        assert_eq!(hanging_adjust(0.0, 270.0).0, Turn::CounterClockwise90);
    }

    #[test]
    fn relative_bits_follow_vanilla_ordinals() {
        let s = RelativeSet::from_bits(0b1_1001); // bits 0, 3, 4 = X, YRot, XRot
        assert!(s.contains(Relative::X));
        assert!(!s.contains(Relative::Y));
        assert!(s.contains(Relative::YRot));
        assert!(s.contains(Relative::XRot));
        assert!(!RelativeSet::from_bits(0b0_1001).contains(Relative::XRot));
    }

    #[test]
    fn vanilla_merge_is_deep() {
        let mut target = BTreeMap::new();
        target.insert(
            "a".to_string(),
            compound(&[("x", Nbt::Int(1)), ("y", Nbt::Int(2))]),
        );
        let mut other = BTreeMap::new();
        other.insert("a".to_string(), compound(&[("x", Nbt::Int(9))]));
        merge_vanilla(&mut target, &other);
        assert_eq!(
            target["a"],
            compound(&[("x", Nbt::Int(9)), ("y", Nbt::Int(2))])
        );
    }

    fn map(n: Nbt) -> BTreeMap<String, Nbt> {
        match n {
            Nbt::Compound(m) => m,
            _ => panic!(),
        }
    }

    #[test]
    fn axiom_merge_modify_replaces_whole_level() {
        let left = map(compound(&[("Health", Nbt::Int(1)), ("Pos", Nbt::Int(2))]));
        let right = map(compound(&[
            ("axiom:modify", Nbt::Byte(1)),
            ("Small", Nbt::Byte(1)),
        ]));
        let out = axiom_merge(left, right);
        assert_eq!(out.keys().cloned().collect::<Vec<_>>(), vec!["Small"]);
    }

    #[test]
    fn axiom_merge_empty_compound_removes_and_nested_modify_is_stripped() {
        let left = map(compound(&[
            ("gone", Nbt::Int(1)),
            ("sub", compound(&[("keep", Nbt::Int(1))])),
        ]));
        let right = map(compound(&[
            ("gone", compound(&[])),
            ("sub", compound(&[("new", Nbt::Int(2))])),
            (
                "fresh",
                compound(&[("axiom:modify", Nbt::Byte(1)), ("v", Nbt::Int(3))]),
            ),
        ]));
        let out = axiom_merge(left, right);
        assert!(!out.contains_key("gone"));
        assert_eq!(
            out["sub"],
            compound(&[("keep", Nbt::Int(1)), ("new", Nbt::Int(2))])
        );
        assert_eq!(out["fresh"], compound(&[("v", Nbt::Int(3))]));
    }

    #[test]
    fn entity_type_filter_rules() {
        let f = EntityTypeFilter::new(Vec::<String>::new(), vec!["minecraft:tnt".to_string()]);
        assert!(!f.allows(TYPE_PLAYER));
        assert!(!f.allows("minecraft:tnt"));
        assert!(f.allows("minecraft:armor_stand"));
        let w = EntityTypeFilter::new(vec!["minecraft:marker".to_string()], Vec::<String>::new());
        assert!(w.allows("minecraft:marker"));
        assert!(!w.allows("minecraft:armor_stand"));
    }

    fn uuid_bytes(out: &mut Vec<u8>, u: u128) {
        out.extend_from_slice(&u.to_be_bytes());
    }

    fn varint(out: &mut Vec<u8>, mut v: u32) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }

    #[test]
    fn read_list_enforces_limit_and_rejects_negative() {
        let mut big = Vec::new();
        varint(&mut big, 5);
        let mut r = Reader::new(&big);
        assert!(matches!(
            read_delete_uuids(&mut r, 4),
            Err(BufError::CollectionTooLarge { count: 5, limit: 4 })
        ));

        let mut neg = Vec::new();
        varint(&mut neg, u32::MAX);
        let mut r = Reader::new(&neg);
        assert!(matches!(
            read_delete_uuids(&mut r, 4),
            Err(BufError::NegativeLength(-1))
        ));
    }

    #[test]
    fn decode_spawn_entry_with_copy_and_null_tag() {
        let mut p = Vec::new();
        varint(&mut p, 1);
        uuid_bytes(&mut p, 7);
        for v in [1.5f64, 2.0, -3.25] {
            p.extend_from_slice(&v.to_be_bytes());
        }
        p.extend_from_slice(&90.0f32.to_be_bytes());
        p.extend_from_slice(&(-10.0f32).to_be_bytes());
        p.push(1);
        uuid_bytes(&mut p, 8);
        let mut r = Reader::new(&p);
        let entries = read_spawn_entries(&mut r, 10, &mut |_| Ok(None)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uuid, 7);
        assert_eq!(entries[0].pos, (1.5, 2.0, -3.25));
        assert_eq!(entries[0].yaw, 90.0);
        assert_eq!(entries[0].copy_from, Some(8));
        assert!(entries[0].tag.is_none());
    }

    #[test]
    fn decode_manipulate_flags_and_passenger_mode() {
        let mut p = Vec::new();
        varint(&mut p, 2);
        // entry 1: absolute placement (flags = 0b1000 with bit 7 clear), AddList with 1 uuid
        uuid_bytes(&mut p, 1);
        p.push(0b0000_1000u8);
        for v in [0.0f64, 64.0, 0.0] {
            p.extend_from_slice(&v.to_be_bytes());
        }
        p.extend_from_slice(&0.0f32.to_be_bytes());
        p.extend_from_slice(&0.0f32.to_be_bytes());
        varint(&mut p, 2);
        varint(&mut p, 1);
        uuid_bytes(&mut p, 99);
        // entry 2: negative flags = no placement, RemoveAll
        uuid_bytes(&mut p, 2);
        p.push(0xff);
        varint(&mut p, 1);
        let mut r = Reader::new(&p);
        let entries =
            read_manipulate_entries(&mut r, 10, &mut |_| Ok(Some(empty_compound()))).unwrap();
        assert_eq!(
            entries[0].placement.as_ref().unwrap().relative,
            RelativeSet::from_bits(8)
        );
        assert_eq!(entries[0].passenger_mode, PassengerMode::AddList);
        assert_eq!(entries[0].passengers, vec![99]);
        assert!(entries[1].placement.is_none());
        assert_eq!(entries[1].passenger_mode, PassengerMode::RemoveAll);
    }

    /// Minimal in-memory engine for the handler tests.
    #[derive(Default)]
    struct Mock {
        ents: BTreeMap<u128, MockEnt>,
        sent: Vec<(u128, String, Vec<u8>)>,
        cancel_spawn: bool,
        cancel_remove: bool,
        filter: EntityTypeFilter,
        next: u128,
        perms: bool,
        blocked_positions: Vec<(i32, i32, i32)>,
    }

    #[derive(Clone, Debug, Default)]
    struct MockEnt {
        type_id: String,
        pos: Pos,
        yaw: f32,
        pitch: f32,
        passengers: Vec<u128>,
        vehicle: Option<u128>,
        removed: bool,
        log: Vec<String>,
    }

    impl Mock {
        fn add(&mut self, uuid: u128, type_id: &str, pos: Pos) {
            self.ents.insert(
                uuid,
                MockEnt {
                    type_id: type_id.into(),
                    pos,
                    ..Default::default()
                },
            );
        }
        fn e(&self, u: u128) -> &MockEnt {
            &self.ents[&u]
        }
        fn em(&mut self, u: u128) -> &mut MockEnt {
            self.ents.get_mut(&u).unwrap()
        }
    }

    impl EntityHost for Mock {
        fn is_axiom_active(&self, _p: u128) -> bool {
            true
        }
        fn has_axiom_permission(&self, _p: u128, _n: &str) -> bool {
            self.perms
        }
        fn can_modify_world(&self, _p: u128, _d: &str) -> bool {
            true
        }
        fn packet_collection_read_limit(&self) -> usize {
            1024
        }
        fn disable_entity_sanitization(&self) -> bool {
            false
        }
        fn entity_type_filter(&self) -> &EntityTypeFilter {
            &self.filter
        }
        fn read_tag_unknown(&self, _p: u128, r: &mut Reader) -> Result<Option<Nbt>, BufError> {
            let _ = r;
            Ok(Some(compound(&[(
                "id",
                Nbt::String("minecraft:armor_stand".into()),
            )])))
        }
        fn active_axiom_players(&self) -> Vec<u128> {
            vec![1, 2]
        }
        fn send_payload(&mut self, player: u128, channel: &str, data: &[u8]) {
            self.sent.push((player, channel.into(), data.to_vec()));
        }
        fn can_place_block(&self, _p: u128, _d: &str, x: i32, y: i32, z: i32) -> bool {
            !self.blocked_positions.contains(&(x, y, z))
        }
        fn can_break_block(&self, _p: u128, _d: &str, _x: i32, _y: i32, _z: i32) -> bool {
            true
        }
        fn is_in_spawnable_bounds(&self, _x: i32, _y: i32, _z: i32) -> bool {
            true
        }
        fn fire_spawn(&mut self, _p: u128, _u: u128) -> bool {
            self.cancel_spawn
        }
        fn fire_manipulate(&mut self, _p: u128, _u: u128) -> bool {
            false
        }
        fn fire_after_manipulate(&mut self, _p: u128, _u: u128) {}
        fn fire_remove(&mut self, _p: u128, _u: u128) -> bool {
            self.cancel_remove
        }
        fn has_entity(&self, _d: &str, u: u128) -> bool {
            self.ents.get(&u).map_or(false, |e| !e.removed)
        }
        fn entity_type_id(&self, u: u128) -> Option<String> {
            self.ents.get(&u).map(|e| e.type_id.clone())
        }
        fn type_can_serialize(&self, t: &str) -> bool {
            t != "minecraft:bogus_unserializable"
        }
        fn position(&self, u: u128) -> Pos {
            self.e(u).pos
        }
        fn yaw(&self, u: u128) -> f32 {
            self.e(u).yaw
        }
        fn pitch(&self, u: u128) -> f32 {
            self.e(u).pitch
        }
        fn is_passenger(&self, u: u128) -> bool {
            self.e(u).vehicle.is_some()
        }
        fn vehicle(&self, u: u128) -> Option<u128> {
            self.e(u).vehicle
        }
        fn passengers(&self, u: u128) -> Vec<u128> {
            self.ents
                .get(&u)
                .map(|e| e.passengers.clone())
                .unwrap_or_default()
        }
        fn is_removed(&self, u: u128) -> bool {
            self.e(u).removed
        }
        fn item_frame_on_y_axis(&self, _u: u128) -> bool {
            false
        }
        fn entity_as_passenger_nbt(&mut self, _d: &str, _u: u128) -> Option<Nbt> {
            None
        }
        fn load_entity_tree(&mut self, _d: &str, tag: &Nbt, root_uuid: u128) -> Option<EntityNode> {
            let Nbt::Compound(m) = tag else { return None };
            let Some(Nbt::String(id)) = m.get("id") else {
                return None;
            };
            let yaw = match m.get("Rotation") {
                Some(Nbt::List(v)) => match v.first() {
                    Some(Nbt::Float(f)) => *f,
                    _ => 0.0,
                },
                _ => 0.0,
            };
            self.add(root_uuid, id, (0.0, 0.0, 0.0));
            self.em(root_uuid).yaw = yaw;
            let mut children = Vec::new();
            if let Some(Nbt::List(list)) = m.get("Passengers") {
                for child in list {
                    let fresh = self.next + 1000;
                    self.next += 1;
                    if let Some(node) = self.load_entity_tree(_d, child, fresh) {
                        children.push(node);
                    }
                }
            }
            let ids: Vec<u128> = children.iter().map(|c| c.uuid).collect();
            for c in &ids {
                self.em(*c).vehicle = Some(root_uuid);
            }
            self.em(root_uuid).passengers = ids;
            Some(EntityNode {
                uuid: root_uuid,
                type_id: id.clone(),
                yaw,
                passengers: children,
            })
        }
        fn set_hanging_turn(&mut self, u: u128, t: Turn) {
            self.em(u).log.push(format!("turn:{:?}", t));
        }
        fn adjust_item_frame_rotation(&mut self, u: u128, d: i32) {
            self.em(u).log.push(format!("frame:{}", d));
        }
        fn snap_to(&mut self, u: u128, pos: Pos, yaw: f32, pitch: f32) {
            let e = self.em(u);
            e.pos = pos;
            e.yaw = yaw;
            e.pitch = pitch;
        }
        fn set_head_yaw(&mut self, _u: u128, _yaw: f32) {}
        fn set_yaw(&mut self, u: u128, yaw: f32) {
            self.em(u).yaw = yaw;
        }
        fn set_pitch(&mut self, u: u128, pitch: f32) {
            self.em(u).pitch = pitch;
        }
        fn set_pos_raw(&mut self, u: u128, pos: Pos) {
            self.em(u).pos = pos;
        }
        fn save_without_id(&mut self, _u: u128) -> Nbt {
            compound(&[("Health", Nbt::Int(20))])
        }
        fn load_nbt(&mut self, _u: u128, _tag: &Nbt) {}
        fn eject_passengers(&mut self, u: u128) {
            let ps = std::mem::take(&mut self.em(u).passengers);
            for p in ps {
                self.em(p).vehicle = None;
            }
        }
        fn start_riding(&mut self, p: u128, v: u128) {
            self.em(p).vehicle = Some(v);
            self.em(v).passengers.push(p);
        }
        fn stop_riding(&mut self, p: u128) {
            if let Some(v) = self.em(p).vehicle.take() {
                self.em(v).passengers.retain(|x| *x != p);
            }
        }
        fn add_fresh_entity_with_passengers(&mut self, _d: &str, _u: u128) -> bool {
            true
        }
        fn remove(&mut self, u: u128) {
            if let Some(e) = self.ents.get_mut(&u) {
                e.removed = true;
            }
        }
    }

    fn mock_spawn_payload(uuid: u128) -> Vec<u8> {
        let mut p = Vec::new();
        varint(&mut p, 1);
        uuid_bytes(&mut p, uuid);
        for v in [5.5f64, 70.0, 5.5] {
            p.extend_from_slice(&v.to_be_bytes());
        }
        p.extend_from_slice(&0.0f32.to_be_bytes());
        p.extend_from_slice(&0.0f32.to_be_bytes());
        p.push(0); // no copy-from; the NBT comes from the mock's read_tag_unknown
        p
    }

    #[test]
    fn spawn_requires_permission() {
        let mut m = Mock {
            perms: false,
            ..Default::default()
        };
        handle_spawn(&mut m, 1, "minecraft:overworld", &mock_spawn_payload(50)).unwrap();
        assert!(!m.has_entity("", 50));
    }

    #[test]
    fn spawn_places_entity_and_cancel_discards_it() {
        let mut m = Mock {
            perms: true,
            ..Default::default()
        };
        handle_spawn(&mut m, 1, "minecraft:overworld", &mock_spawn_payload(50)).unwrap();
        assert!(m.has_entity("", 50));
        assert_eq!(m.e(50).pos, (5.5, 70.0, 5.5));

        let mut m = Mock {
            perms: true,
            cancel_spawn: true,
            ..Default::default()
        };
        handle_spawn(&mut m, 1, "minecraft:overworld", &mock_spawn_payload(51)).unwrap();
        assert!(!m.has_entity("", 51));
    }

    #[test]
    fn manipulate_mount_loop_and_passenger_rules() {
        let mut m = Mock {
            perms: true,
            ..Default::default()
        };
        m.add(10, "minecraft:armor_stand", (0.0, 64.0, 0.0));
        m.add(11, "minecraft:armor_stand", (0.0, 64.0, 0.0));
        m.add(12, "minecraft:marker", (0.0, 64.0, 0.0));
        // 11 is a passenger of 10, so making 10 ride 11 would loop.
        m.start_riding(11, 10);
        let e = ManipulateEntry {
            uuid: 11,
            placement: None,
            merge: None,
            passenger_mode: PassengerMode::AddList,
            passengers: vec![10],
        };
        manipulate_one(&mut m, 1, "minecraft:overworld", e);
        assert_eq!(m.e(10).vehicle, None);
        assert_eq!(m.e(11).vehicle, Some(10));

        // Markers never ride or get ridden.
        let e = ManipulateEntry {
            uuid: 10,
            placement: None,
            merge: None,
            passenger_mode: PassengerMode::AddList,
            passengers: vec![12],
        };
        manipulate_one(&mut m, 1, "minecraft:overworld", e);
        assert_eq!(m.e(12).vehicle, None);

        // RemoveList detaches a passenger of this entity.
        let e = ManipulateEntry {
            uuid: 10,
            placement: None,
            merge: None,
            passenger_mode: PassengerMode::RemoveList,
            passengers: vec![11],
        };
        manipulate_one(&mut m, 1, "minecraft:overworld", e);
        assert_eq!(m.e(11).vehicle, None);
    }

    #[test]
    fn manipulate_relative_move_and_hanging_turn() {
        let mut m = Mock {
            perms: true,
            ..Default::default()
        };
        m.add(20, "minecraft:item_frame", (1.0, 2.0, 3.0));
        m.em(20).yaw = 0.0;
        let e = ManipulateEntry {
            uuid: 20,
            placement: Some(Placement {
                relative: RelativeSet::from_bits(0b0_0001 | 0b0_1000), // X and YRot relative
                pos: (1.0, 2.0, 3.0),
                yaw: 90.0,
                pitch: 0.0,
            }),
            merge: None,
            passenger_mode: PassengerMode::None,
            passengers: vec![],
        };
        manipulate_one(&mut m, 1, "minecraft:overworld", e);
        assert_eq!(m.e(20).pos, (2.0, 2.0, 3.0)); // x relative, y and z absolute
        assert_eq!(m.e(20).yaw, 90.0);
        assert!(m.e(20).log.contains(&"turn:Clockwise90".to_string()));
    }

    #[test]
    fn delete_respects_player_passenger_and_cancel() {
        let mut m = Mock {
            perms: true,
            ..Default::default()
        };
        m.add(30, "minecraft:armor_stand", (0.0, 0.0, 0.0));
        m.add(31, "minecraft:armor_stand", (0.0, 0.0, 0.0));
        m.add(32, "minecraft:player", (0.0, 0.0, 0.0));
        m.start_riding(32, 31);
        let mut p = Vec::new();
        varint(&mut p, 2);
        uuid_bytes(&mut p, 30);
        uuid_bytes(&mut p, 31);
        handle_delete(&mut m, 1, "minecraft:overworld", &p).unwrap();
        assert!(m.e(30).removed);
        assert!(
            !m.e(31).removed,
            "entity carrying a player must not be deleted"
        );
    }

    #[test]
    fn delete_cancelled_by_event_keeps_entity() {
        let mut m = Mock {
            perms: true,
            cancel_remove: true,
            ..Default::default()
        };
        m.add(40, "minecraft:armor_stand", (0.0, 0.0, 0.0));
        let mut p = Vec::new();
        varint(&mut p, 1);
        uuid_bytes(&mut p, 40);
        handle_delete(&mut m, 1, "minecraft:overworld", &p).unwrap();
        assert!(!m.e(40).removed);
    }

    #[test]
    fn hidden_displays_resend_only_to_active_players_on_growth() {
        let mut m = Mock::default();
        let mut h = HiddenEntities::new();
        h.hide_display_gizmo(1, 100);
        h.tick(&mut m);
        assert!(
            m.sent.is_empty(),
            "nothing sent before a player got the list"
        );

        h.send_all(&mut m, &[1]);
        assert_eq!(m.sent.len(), 1);
        assert_eq!(m.sent[0].1, IGNORE_DISPLAY_CHANNEL);
        // varint(1) then uuid 100 as 16 big-endian bytes
        assert_eq!(m.sent[0].2[0], 1);
        assert_eq!(&m.sent[0].2[1..], &100u128.to_be_bytes());

        h.hide_display_gizmo(2, 200);
        h.tick(&mut m);
        assert_eq!(m.sent.len(), 3, "resend to both active players");
        h.forget_display(2);
        h.forget_display(1);
    }

    #[test]
    fn markers_are_tracked_by_uuid_and_never_sent() {
        let mut h = HiddenEntities::new();
        h.hide_marker_gizmo(5);
        assert!(h.is_marker_hidden(5));
        h.forget_marker(5);
        assert!(!h.is_marker_hidden(5));
    }
}

#[cfg(test)]
mod detached_tree_tests {
    use super::*;
    use crate::buf::NbtCompound;

    fn entity(id: &str, rotation: f32, passengers: Vec<Nbt>) -> Nbt {
        let mut map = NbtCompound::new();
        map.insert("id".into(), Nbt::String(id.into()));
        map.insert("Rotation".into(), Nbt::List(vec![Nbt::Float(rotation), Nbt::Float(0.0)]));
        if !passengers.is_empty() {
            map.insert("Passengers".into(), Nbt::List(passengers));
        }
        Nbt::Compound(map)
    }

    /// Fresh UUIDs in the order they are asked for.
    fn counter() -> impl FnMut() -> u128 {
        let mut next = 100u128;
        move || {
            next += 1;
            next
        }
    }

    #[test]
    fn root_gets_its_uuid_and_passengers_fresh_ones_in_order() {
        let tag = entity(
            "minecraft:boat",
            90.0,
            vec![entity("minecraft:zombie", 10.0, vec![entity("minecraft:chicken", 0.0, vec![])])],
        );
        let mut fresh = counter();
        let (node, flat) = build_detached_tree(&tag, 7, &mut fresh).expect("loads");
        assert_eq!(node.uuid, 7);
        assert_eq!(node.type_id, "minecraft:boat");
        assert_eq!(node.yaw, 90.0);
        assert_eq!(node.passengers.len(), 1);
        assert_eq!(node.passengers[0].uuid, 101);
        assert_eq!(node.passengers[0].yaw, 10.0);
        assert_eq!(node.passengers[0].passengers[0].uuid, 102);
        // Flat list: root first, each entity names its parent.
        let uuids: Vec<u128> = flat.iter().map(|e| e.uuid).collect();
        assert_eq!(uuids, vec![7, 101, 102]);
        assert_eq!(flat[0].parent, None);
        assert_eq!(flat[1].parent, Some(7));
        assert_eq!(flat[2].parent, Some(101));
    }

    #[test]
    fn flat_data_drops_the_passengers_list_and_keeps_the_rest() {
        let tag = entity("minecraft:armor_stand", 0.0, vec![entity("minecraft:marker", 0.0, vec![])]);
        let (_, flat) = build_detached_tree(&tag, 1, &mut counter()).expect("loads");
        let Nbt::Compound(root) = &flat[0].data else { panic!("compound") };
        assert!(!root.contains_key("Passengers"));
        assert_eq!(root.get("id"), Some(&Nbt::String("minecraft:armor_stand".into())));
        assert!(root.contains_key("Rotation"));
    }

    #[test]
    fn passenger_without_an_id_is_left_out_and_takes_no_uuid() {
        let bad = Nbt::Compound(NbtCompound::new());
        let tag = entity("minecraft:horse", 0.0, vec![bad, entity("minecraft:pig", 0.0, vec![])]);
        let (node, flat) = build_detached_tree(&tag, 5, &mut counter()).expect("loads");
        assert_eq!(node.passengers.len(), 1);
        assert_eq!(node.passengers[0].type_id, "minecraft:pig");
        // The skipped passenger asked for no UUID, so the pig takes the first fresh one.
        assert_eq!(node.passengers[0].uuid, 101);
        assert_eq!(flat.len(), 2);
    }

    #[test]
    fn root_without_an_id_is_refused() {
        let tag = Nbt::Compound(NbtCompound::new());
        assert!(build_detached_tree(&tag, 1, &mut counter()).is_none());
        assert!(build_detached_tree(&Nbt::Int(3), 1, &mut counter()).is_none());
    }

    #[test]
    fn missing_rotation_reads_as_zero_yaw() {
        let mut map = NbtCompound::new();
        map.insert("id".into(), Nbt::String("minecraft:pig".into()));
        let (node, _) = build_detached_tree(&Nbt::Compound(map), 1, &mut counter()).expect("loads");
        assert_eq!(node.yaw, 0.0);
    }
}
