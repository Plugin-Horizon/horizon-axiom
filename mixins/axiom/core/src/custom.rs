// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd):
//   paperapi/AxiomCustomBlocksAPI.java, paperapi/AxiomCustomDisplayAPI.java,
//   paperapi/AxiomAlreadyRegisteredException.java, paperapi/block/*.java and paperapi/display/*.java
//   (entity part excluded), commands/AxiomDebugCommand.java (as text functions), and the
//   axiom:update_available_dispatch_sends payload of AxiomPaper.java.

//! Custom blocks and custom display items for Axiom, and the payloads that register them.
//!
//! The registries keep the registrations and build the plugin-channel payloads. They never touch
//! the world or the network themselves: the game glue implements [`CustomHost`] (online players,
//! protocol mismatch, block state names, sending) and [`DebugHost`] (values for the debug command).
//!
//! Engine encodings come in already encoded. Item stacks are passed as their body in the item
//! stream codec (`ItemStack.STREAM_CODEC`, without the presence flag). Block states are global
//! block state ids (`u32`).
//!
//! Differences from the Java code: maps are written in key order (BTreeMap) instead of HashMap
//! order, and the WrongThreadException check is the glue's job.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::buf::Writer;

pub const CHANNEL_CUSTOM_BLOCKS: &str = "axiom:custom_blocks";
pub const CHANNEL_REGISTER_CUSTOM_BLOCK: &str = "axiom:register_custom_block_v2";
pub const CHANNEL_REGISTER_CUSTOM_ITEMS: &str = "axiom:register_custom_items";
pub const CHANNEL_UPDATE_DISPATCH_SENDS: &str = "axiom:update_available_dispatch_sends";

/// Account that always passes the debug command check (kept as in AxiomDebugCommand).
pub const DEBUG_COMMAND_UUID: u128 = 0xd0e05de7_6067_454d_beae_c6d19d886191;

/// `AxiomAlreadyRegisteredException` plus the validation RuntimeExceptions of ImplServerCustomBlocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomError {
    AlreadyRegistered(String),
    InvalidId(String),
    InvalidProperty(String),
    WrongStateCount(String),
    MappingOutsideBlock(&'static str),
}

impl fmt::Display for CustomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRegistered(msg)
            | Self::InvalidId(msg)
            | Self::InvalidProperty(msg)
            | Self::WrongStateCount(msg) => f.write_str(msg),
            Self::MappingOutsideBlock(name) => {
                write!(f, "{name} mapping either key or value must be part of custom block")
            }
        }
    }
}

impl std::error::Error for CustomError {}

/// What the game glue must provide for block and display registration.
pub trait CustomHost {
    /// Online players that may use Axiom (Java canUseAxiom).
    fn axiom_players(&self) -> Vec<u128>;
    /// True when the player's client protocol version differs from the server's.
    fn protocol_mismatch(&self, player: u128) -> bool;
    /// Block state as BlockStateParser.serialize prints it, e.g. `minecraft:note_block[note=3]`.
    /// Only used for clients with a protocol mismatch.
    fn block_state_string(&self, state: u32) -> String;
    /// Sends one plugin-channel payload to one player.
    fn send_payload(&mut self, player: u128, channel: &str, data: &[u8]);
}

/// Values for the debug command. Each method answers for the player who ran the command.
pub trait DebugHost {
    fn is_op(&self, player: u128) -> bool;
    fn has_permission(&self, player: u128, node: &str) -> bool;
    fn can_use_axiom(&self, player: u128) -> bool;
    fn is_mismatched_data_version(&self, player: u128) -> bool;
    fn can_modify_world(&self, player: u128) -> bool;
    fn is_client_listening(&self, player: u128, channel: &str) -> bool;
    /// Debug text of the player's restrictions, None when they have none.
    fn restrictions(&self, player: u128) -> Option<String>;
    /// None runs the generic Integration check, Some runs only that provider.
    fn can_break_block(&self, player: u128, provider: Option<Provider>) -> bool;
    fn can_place_block(&self, player: u128, provider: Option<Provider>) -> bool;
    fn is_plot_world(&self, player: u128) -> bool;
    /// Debug text of the current editable plot, "null" when there is none (Java toString).
    fn current_editable_plot(&self, player: u128) -> String;
}

/// Integrations that the debug command can query separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    PlotSquared,
    WorldGuard,
}

/// Placement logic hints, in Java ordinal order (EnumSet iteration order on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlacementLogic {
    Axis,
    Facing,
    FacingOpposite,
    FacingClicked,
    FacingClickedOpposite,
    Waterlogged,
    Half,
}

impl PlacementLogic {
    fn wire_id(self) -> &'static str {
        match self {
            Self::Axis => "axiom:axis",
            Self::Facing => "axiom:facing",
            Self::FacingOpposite => "axiom:facing_opposite",
            Self::FacingClicked => "axiom:facing_clicked",
            Self::FacingClickedOpposite => "axiom:facing_clicked_opposite",
            Self::Waterlogged => "axiom:waterlogged",
            Self::Half => "axiom:half",
        }
    }
}

/// Vanilla block state properties that Axiom knows by identity (BlockStateProperties).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownProperty {
    Axis,
    HorizontalAxis,
    Facing,
    HorizontalFacing,
    Up,
    Down,
    North,
    East,
    South,
    West,
    Waterlogged,
    Half,
    VerticalDirection,
}

impl KnownProperty {
    fn name(self) -> &'static str {
        match self {
            Self::Axis => "axis",
            Self::HorizontalAxis => "horizontal_axis",
            Self::Facing => "facing",
            Self::HorizontalFacing => "horizontal_facing",
            Self::Up => "up",
            Self::Down => "down",
            Self::North => "north",
            Self::East => "east",
            Self::South => "south",
            Self::West => "west",
            Self::Waterlogged => "waterlogged",
            Self::Half => "half",
            Self::VerticalDirection => "vertical_direction",
        }
    }

    /// Wire code (byte) for a known property; the client maps it back to its own property.
    fn wire_code(self) -> u8 {
        match self {
            Self::Axis => 3,
            Self::HorizontalAxis => 4,
            Self::Facing => 5,
            Self::HorizontalFacing => 6,
            Self::Up => 7,
            Self::Down => 8,
            Self::North => 9,
            Self::East => 10,
            Self::South => 11,
            Self::West => 12,
            Self::Waterlogged => 13,
            Self::Half => 14,
            Self::VerticalDirection => 15,
        }
    }

    fn num_values(self) -> usize {
        match self {
            Self::Axis => 3,
            Self::Facing => 6,
            Self::HorizontalFacing => 4,
            Self::HorizontalAxis => 2,
            _ => 2,
        }
    }
}

/// A block state property of a custom block. `Enum` covers both vanilla enum properties and
/// string properties, which go out with the same wire code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxiomProperty {
    Known(KnownProperty),
    Boolean { name: String },
    Int { name: String, min: i32, max: i32 },
    Enum { name: String, values: Vec<String> },
}

impl AxiomProperty {
    pub fn name(&self) -> &str {
        match self {
            Self::Known(k) => k.name(),
            Self::Boolean { name } | Self::Int { name, .. } | Self::Enum { name, .. } => name,
        }
    }

    pub fn num_values(&self) -> usize {
        match self {
            Self::Known(k) => k.num_values(),
            Self::Boolean { .. } => 2,
            Self::Int { min, max, .. } => (i64::from(*max) - i64::from(*min) + 1).max(0) as usize,
            Self::Enum { values, .. } => values.len(),
        }
    }

    fn validate(&self) -> Result<(), CustomError> {
        match self {
            Self::Int { name, min, max } if min > max => Err(CustomError::InvalidProperty(format!(
                "Int property {name}: min {min} is greater than max {max}"
            ))),
            Self::Enum { name, values } if values.is_empty() => {
                Err(CustomError::InvalidProperty(format!("Enum property {name} has no values")))
            }
            _ => Ok(()),
        }
    }
}

/// Builder for a custom block (AxiomCustomBlockBuilder). Blocks are global block state ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomBlockBuilder {
    id: String,
    translation_key: String,
    properties: Vec<AxiomProperty>,
    blocks: Vec<u32>,
    /// Pick-block item, as the body of `ItemStack.OPTIONAL_STREAM_CODEC` encoded by the host.
    /// Not sent to clients with a protocol mismatch.
    pick_item: Option<Vec<u8>>,
    placement_logics: BTreeSet<PlacementLogic>,
    send_server_pick_block_if_possible: bool,
    prevent_right_click_interaction: bool,
    prevent_shape_updates: bool,
    automatic_rotation_and_mirroring: bool,
    rotate_y: BTreeMap<u32, u32>,
    flip_x: BTreeMap<u32, u32>,
    flip_y: BTreeMap<u32, u32>,
    flip_z: BTreeMap<u32, u32>,
}

impl CustomBlockBuilder {
    /// `AxiomCustomBlocksAPI.create`. `id` is `namespace:path` (a bare path means `minecraft`).
    /// `blocks` holds one state per combination of property values.
    pub fn new(id: &str, translation_key: &str, properties: Vec<AxiomProperty>, blocks: Vec<u32>) -> Self {
        Self {
            id: id.to_string(),
            translation_key: translation_key.to_string(),
            properties,
            blocks,
            pick_item: None,
            placement_logics: BTreeSet::new(),
            send_server_pick_block_if_possible: true,
            prevent_right_click_interaction: false,
            prevent_shape_updates: false,
            automatic_rotation_and_mirroring: true,
            rotate_y: BTreeMap::new(),
            flip_x: BTreeMap::new(),
            flip_y: BTreeMap::new(),
            flip_z: BTreeMap::new(),
        }
    }

    /// `createSingle`: one block, no properties.
    pub fn single(id: &str, translation_key: &str, state: u32) -> Self {
        Self::new(id, translation_key, Vec::new(), vec![state])
    }

    /// `createAxis`: blocks are the x, y and z states.
    pub fn axis(id: &str, translation_key: &str, x: u32, y: u32, z: u32) -> Self {
        let mut b = Self::new(id, translation_key, vec![AxiomProperty::Known(KnownProperty::Axis)], vec![x, y, z]);
        b.add_placement_logic(PlacementLogic::Axis);
        b
    }

    /// `createFacing`: blocks are north, east, south, west, up and down.
    pub fn facing(id: &str, translation_key: &str, states: [u32; 6]) -> Self {
        let mut b = Self::new(id, translation_key, vec![AxiomProperty::Known(KnownProperty::Facing)], states.to_vec());
        b.add_placement_logic(PlacementLogic::FacingOpposite);
        b
    }

    /// `createHorizontalFacing`: blocks are north, east, south and west.
    pub fn horizontal_facing(id: &str, translation_key: &str, states: [u32; 4]) -> Self {
        let mut b = Self::new(
            id,
            translation_key,
            vec![AxiomProperty::Known(KnownProperty::HorizontalFacing)],
            states.to_vec(),
        );
        b.add_placement_logic(PlacementLogic::FacingOpposite);
        b
    }

    pub fn pick_block_item(&mut self, item: Option<Vec<u8>>) {
        self.pick_item = item;
    }

    pub fn send_server_pick_block_if_possible(&mut self, value: bool) {
        self.send_server_pick_block_if_possible = value;
    }

    pub fn prevent_right_click_interaction(&mut self, value: bool) {
        self.prevent_right_click_interaction = value;
    }

    pub fn prevent_shape_updates(&mut self, value: bool) {
        self.prevent_shape_updates = value;
    }

    pub fn automatic_rotation_and_mirroring(&mut self, value: bool) {
        self.automatic_rotation_and_mirroring = value;
    }

    pub fn add_placement_logic(&mut self, logic: PlacementLogic) {
        self.placement_logics.insert(logic);
    }

    pub fn override_rotate_y(&mut self, from: u32, to: u32) {
        self.rotate_y.insert(from, to);
    }

    pub fn override_flip_x(&mut self, from: u32, to: u32) {
        self.flip_x.insert(from, to);
    }

    pub fn override_flip_y(&mut self, from: u32, to: u32) {
        self.flip_y.insert(from, to);
    }

    pub fn override_flip_z(&mut self, from: u32, to: u32) {
        self.flip_z.insert(from, to);
    }

    /// Checks the same rules as ImplServerCustomBlocks and normalizes the id.
    fn validated(mut self) -> Result<Self, CustomError> {
        self.id = parse_id(&self.id)?;
        for property in &self.properties {
            property.validate()?;
        }
        let expected = self
            .properties
            .iter()
            .try_fold(1usize, |acc, p| acc.checked_mul(p.num_values()))
            .unwrap_or(usize::MAX);
        if expected != self.blocks.len() {
            return Err(CustomError::WrongStateCount(self.state_count_message(expected)));
        }
        check_mapping(&self.rotate_y, &self.blocks, "customRotateY")?;
        check_mapping(&self.flip_x, &self.blocks, "customFlipX")?;
        check_mapping(&self.flip_y, &self.blocks, "customFlipY")?;
        check_mapping(&self.flip_z, &self.blocks, "customFlipZ")?;
        Ok(self)
    }

    fn state_count_message(&self, expected: usize) -> String {
        let mut msg = format!(
            "Incorrect number of blocks provided, needed {expected}, got {}: ",
            self.blocks.len()
        );
        if self.properties.is_empty() {
            msg.push_str("no properties ");
        } else {
            for (i, property) in self.properties.iter().enumerate() {
                if i > 0 {
                    msg.push_str("* ");
                }
                msg.push_str(&format!("{} ({}) ", property.name(), property.num_values()));
            }
        }
        msg.push_str(&format!("= {expected} block(s)"));
        msg
    }

    /// Encodes the `axiom:register_custom_block_v2` body for one client.
    fn encode(&self, host: &dyn CustomHost, protocol_mismatch: bool) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_string(&self.id);
        w.write_string(&self.translation_key);
        put_list(&mut w, &self.properties, write_property);
        put_list(&mut w, &self.blocks, |w, state| put_state(w, host, protocol_mismatch, *state));
        match &self.pick_item {
            Some(item) if !protocol_mismatch => {
                w.write_bool(true);
                w.write_bytes(item);
            }
            _ => w.write_bool(false),
        }
        w.write_bool(self.send_server_pick_block_if_possible);
        w.write_bool(self.prevent_right_click_interaction);
        w.write_bool(self.prevent_shape_updates);
        let logics: Vec<PlacementLogic> = self.placement_logics.iter().copied().collect();
        put_list(&mut w, &logics, |w, logic| w.write_string(logic.wire_id()));
        w.write_bool(self.automatic_rotation_and_mirroring);
        for map in [&self.rotate_y, &self.flip_x, &self.flip_y, &self.flip_z] {
            put_map(&mut w, map, host, protocol_mismatch);
        }
        w.write_varint(0);
        w.into_bytes()
    }
}

/// Builder for a custom display item (AxiomCustomDisplayAPI).
#[derive(Debug, Clone, PartialEq)]
pub struct CustomDisplayBuilder {
    id: String,
    search_key: String,
    /// ItemStack body in `ItemStack.STREAM_CODEC`, encoded by the host.
    item: Vec<u8>,
    translation: Option<[f32; 3]>,
    left_rotation: Option<[f32; 4]>,
    scale: Option<[f32; 3]>,
    right_rotation: Option<[f32; 4]>,
    brightness: Option<(u8, u8)>,
}

impl CustomDisplayBuilder {
    pub fn new(id: &str, search_key: &str, item: Vec<u8>) -> Self {
        Self {
            id: id.to_string(),
            search_key: search_key.to_string(),
            item,
            translation: None,
            left_rotation: None,
            scale: None,
            right_rotation: None,
            brightness: None,
        }
    }

    pub fn set_default_translation(&mut self, x: f32, y: f32, z: f32) {
        self.translation = Some([x, y, z]);
    }

    pub fn set_default_left_rotation(&mut self, x: f32, y: f32, z: f32, w: f32) {
        self.left_rotation = Some([x, y, z, w]);
    }

    pub fn set_default_scale(&mut self, x: f32, y: f32, z: f32) {
        self.scale = Some([x, y, z]);
    }

    pub fn set_default_right_rotation(&mut self, x: f32, y: f32, z: f32, w: f32) {
        self.right_rotation = Some([x, y, z, w]);
    }

    /// Clamps both values to 0..=15 like the Java builder.
    pub fn set_default_brightness(&mut self, block: i32, sky: i32) {
        self.brightness = Some((block.clamp(0, 15) as u8, sky.clamp(0, 15) as u8));
    }

    fn validated(mut self) -> Result<Self, CustomError> {
        self.id = parse_id(&self.id)?;
        Ok(self)
    }

    fn encode_into(&self, w: &mut Writer) {
        w.write_bytes(&self.item);
        w.write_string(&self.id);
        w.write_string(&self.search_key);
        if self.translation.is_some() || self.left_rotation.is_some() || self.scale.is_some() || self.right_rotation.is_some() {
            w.write_bool(true);
            put_f32s(w, &self.translation.unwrap_or([0.0; 3]));
            put_f32s(w, &self.left_rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]));
            put_f32s(w, &self.scale.unwrap_or([1.0; 3]));
            put_f32s(w, &self.right_rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]));
        } else {
            w.write_bool(false);
        }
        match self.brightness {
            Some((block, sky)) => {
                w.write_bool(true);
                w.write_bytes(&[block, sky]);
            }
            None => w.write_bool(false),
        }
    }
}

/// Registry entry with the plugin that registered it.
struct Owned<T> {
    owner: String,
    value: T,
}

/// Custom block registry (ImplServerCustomBlocks).
#[derive(Default)]
pub struct CustomBlockRegistry {
    blocks: Vec<Owned<CustomBlockBuilder>>,
    pending_reregister: bool,
    has_registered_to_player: bool,
}

impl CustomBlockRegistry {
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().map(|b| b.value.id.as_str())
    }

    /// Validates and registers a block for `owner`, sending it at once when clients already have the list.
    pub fn register(&mut self, host: &mut dyn CustomHost, owner: &str, block: CustomBlockBuilder) -> Result<(), CustomError> {
        let block = block.validated()?;
        if self.blocks.iter().any(|b| b.value.id == block.id) {
            return Err(CustomError::AlreadyRegistered(format!(
                "Custom block is already registered with id {}",
                block.id
            )));
        }
        for state in &block.blocks {
            if let Some(existing) = self.blocks.iter().find(|b| b.value.blocks.contains(state)) {
                return Err(CustomError::AlreadyRegistered(format!(
                    "BlockState {state} is already used by {}",
                    existing.value.id
                )));
            }
        }
        self.blocks.push(Owned { owner: owner.to_string(), value: block });

        if !self.pending_reregister && self.has_registered_to_player {
            let block = &self.blocks.last().expect("block was just pushed").value;
            let mut cached: [Option<Vec<u8>>; 2] = [None, None];
            for player in host.axiom_players() {
                let mismatch = host.protocol_mismatch(player);
                let data = cached[usize::from(mismatch)].get_or_insert_with(|| block.encode(&*host, mismatch));
                host.send_payload(player, CHANNEL_REGISTER_CUSTOM_BLOCK, data.as_slice());
            }
        }
        Ok(())
    }

    /// Removes every block of `owner`; the full list is re-sent on the next `tick`.
    pub fn unregister_all(&mut self, owner: &str) {
        let before = self.blocks.len();
        self.blocks.retain(|b| b.owner != owner);
        if self.blocks.len() != before && self.has_registered_to_player {
            self.pending_reregister = true;
        }
    }

    /// Call once per server tick. Re-sends the full list if a re-register is pending.
    pub fn tick(&mut self, host: &mut dyn CustomHost) {
        if !self.pending_reregister {
            return;
        }
        self.pending_reregister = false;

        let mut correct = Vec::new();
        let mut mismatched = Vec::new();
        for player in host.axiom_players() {
            if host.protocol_mismatch(player) {
                mismatched.push(player);
            } else {
                correct.push(player);
            }
        }
        if correct.is_empty() && mismatched.is_empty() {
            self.has_registered_to_player = false;
        } else {
            self.send_all(host, &correct, false);
            self.send_all(host, &mismatched, true);
        }
    }

    /// Full list for a player who just joined (the caller checks that the player may use Axiom).
    pub fn on_player_join(&mut self, host: &mut dyn CustomHost, player: u128) {
        self.has_registered_to_player = true;
        let mismatch = host.protocol_mismatch(player);
        host.send_payload(player, CHANNEL_CUSTOM_BLOCKS, &clear_payload());
        for block in &self.blocks {
            let data = block.value.encode(&*host, mismatch);
            host.send_payload(player, CHANNEL_REGISTER_CUSTOM_BLOCK, &data);
        }
    }

    fn send_all(&mut self, host: &mut dyn CustomHost, players: &[u128], mismatch: bool) {
        if players.is_empty() {
            return;
        }
        self.has_registered_to_player = true;
        for player in players {
            host.send_payload(*player, CHANNEL_CUSTOM_BLOCKS, &clear_payload());
        }
        for block in &self.blocks {
            let data = block.value.encode(&*host, mismatch);
            for player in players {
                host.send_payload(*player, CHANNEL_REGISTER_CUSTOM_BLOCK, &data);
            }
        }
    }
}

/// Custom display registry (ImplServerCustomDisplays). Only clients without a protocol mismatch get the list.
#[derive(Default)]
pub struct CustomDisplayRegistry {
    displays: Vec<Owned<CustomDisplayBuilder>>,
    pending_reregister: bool,
    has_registered_to_player: bool,
}

impl CustomDisplayRegistry {
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.displays.iter().map(|d| d.value.id.as_str())
    }

    /// Validates and registers a display for `owner`. Clients get the new list on the next `tick`.
    pub fn register(&mut self, owner: &str, display: CustomDisplayBuilder) -> Result<(), CustomError> {
        let display = display.validated()?;
        if self.displays.iter().any(|d| d.value.id == display.id) {
            return Err(CustomError::AlreadyRegistered(format!(
                "Custom display is already registered with id {}",
                display.id
            )));
        }
        self.displays.push(Owned { owner: owner.to_string(), value: display });
        if self.has_registered_to_player {
            self.pending_reregister = true;
        }
        Ok(())
    }

    pub fn unregister_all(&mut self, owner: &str) {
        let before = self.displays.len();
        self.displays.retain(|d| d.owner != owner);
        if self.displays.len() != before && self.has_registered_to_player {
            self.pending_reregister = true;
        }
    }

    pub fn tick(&mut self, host: &mut dyn CustomHost) {
        if !self.pending_reregister {
            return;
        }
        self.pending_reregister = false;

        let players: Vec<u128> = host
            .axiom_players()
            .into_iter()
            .filter(|p| !host.protocol_mismatch(*p))
            .collect();
        if players.is_empty() {
            self.has_registered_to_player = false;
            return;
        }
        self.has_registered_to_player = true;
        let data = self.payload();
        for player in players {
            host.send_payload(player, CHANNEL_REGISTER_CUSTOM_ITEMS, &data);
        }
    }

    /// Full list for a player who just joined (the caller checks that the player may use Axiom).
    pub fn on_player_join(&mut self, host: &mut dyn CustomHost, player: u128) {
        self.has_registered_to_player = true;
        host.send_payload(player, CHANNEL_REGISTER_CUSTOM_ITEMS, &self.payload());
    }

    /// `axiom:register_custom_items` body: count, then every display.
    pub fn payload(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_varint(self.displays.len() as i32);
        for display in &self.displays {
            display.value.encode_into(&mut w);
        }
        w.into_bytes()
    }
}

/// `axiom:custom_blocks` body that clears the client's custom block list.
pub fn clear_payload() -> Vec<u8> {
    let mut w = Writer::new();
    w.write_varint(0);
    w.into_bytes()
}

/// `axiom:update_available_dispatch_sends` body: amount added and the maximum per second.
pub fn update_dispatch_sends_payload(add: i32, max: i32) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_varint(add);
    w.write_varint(max);
    w.into_bytes()
}

/// Sends the `axiom:update_available_dispatch_sends` payload to one player.
pub fn send_update_dispatch_sends(host: &mut dyn CustomHost, player: u128, add: i32, max: i32) {
    host.send_payload(player, CHANNEL_UPDATE_DISPATCH_SENDS, &update_dispatch_sends_payload(add, max));
}

/// Runs one `/axiompaperdebug` subcommand and returns the chat line.
/// `sender` is None for non-players. Returns None when nothing would be sent: no permission,
/// unknown subcommand, or a missing argument.
pub fn debug_command(host: &dyn DebugHost, sender: Option<u128>, input: &str) -> Option<String> {
    let player = sender?;
    let allowed = host.is_op(player)
        || host.has_permission(player, "axiom.all")
        || host.has_permission(player, "axiom.debug")
        || player == DEBUG_COMMAND_UUID;
    if !allowed {
        return None;
    }

    let input = input.trim();
    let (sub, arg) = match input.split_once(' ') {
        Some((sub, arg)) => (sub, arg.trim()),
        None => (input, ""),
    };
    Some(match sub {
        "canUseAxiom" => format!("canUseAxiom: {}", host.can_use_axiom(player)),
        "isMismatchedDataVersion" => format!("isMismatchedDataVersion: {}", host.is_mismatched_data_version(player)),
        "canModifyWorld" => format!("canModifyWorld: {}", host.can_modify_world(player)),
        "isClientListening" if !arg.is_empty() => {
            format!("listening to {arg}: {}", host.is_client_listening(player, arg))
        }
        "hasPermission" if !arg.is_empty() => format!("has permission {arg}: {}", host.has_permission(player, arg)),
        "getRestrictions" => match host.restrictions(player) {
            Some(text) => format!("restrictions: {text}"),
            None => "no restrictions".to_string(),
        },
        "canBreakBlockAtCurrentPosition" => format!("canBreakBlock: {}", host.can_break_block(player, provider_arg(arg)?)),
        "canPlaceBlockAtCurrentPosition" => format!("canPlaceBlock: {}", host.can_place_block(player, provider_arg(arg)?)),
        "isPlotWorld" => format!("isPlotWorld: {}", host.is_plot_world(player)),
        "getCurrentEditablePlot" => format!("plotBounds: {}", host.current_editable_plot(player)),
        _ => return None,
    })
}

/// Optional provider literal: empty means the generic check. Anything else is a syntax error.
fn provider_arg(arg: &str) -> Option<Option<Provider>> {
    match arg {
        "" => Some(None),
        "plotsquared" => Some(Some(Provider::PlotSquared)),
        "worldguard" => Some(Some(Provider::WorldGuard)),
        _ => None,
    }
}

/// Validates `namespace:path` (a bare path means `minecraft`) with Identifier's character rules.
pub fn parse_id(input: &str) -> Result<String, CustomError> {
    let (namespace, path) = input.split_once(':').unwrap_or(("minecraft", input));
    let namespace_ok = !namespace.is_empty()
        && namespace.bytes().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.'));
    let path_ok = !path.is_empty()
        && path.bytes().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/'));
    if namespace_ok && path_ok {
        Ok(format!("{namespace}:{path}"))
    } else {
        Err(CustomError::InvalidId(format!("Invalid identifier: {input}")))
    }
}

fn check_mapping(map: &BTreeMap<u32, u32>, states: &[u32], name: &'static str) -> Result<(), CustomError> {
    for (from, to) in map {
        if !states.contains(from) && !states.contains(to) {
            return Err(CustomError::MappingOutsideBlock(name));
        }
    }
    Ok(())
}

fn write_property(w: &mut Writer, property: &AxiomProperty) {
    match property {
        AxiomProperty::Known(known) => w.write_bytes(&[known.wire_code()]),
        AxiomProperty::Boolean { name } => {
            w.write_bytes(&[0]);
            w.write_string(name);
        }
        AxiomProperty::Int { name, min, max } => {
            w.write_bytes(&[1]);
            w.write_string(name);
            w.write_bytes(&min.to_be_bytes());
            w.write_bytes(&max.to_be_bytes());
        }
        AxiomProperty::Enum { name, values } => {
            w.write_bytes(&[2]);
            w.write_string(name);
            put_list(w, values, |w, value| w.write_string(value));
        }
    }
}

/// Block state as an id, or as `-1` plus its string form for a protocol mismatch.
fn put_state(w: &mut Writer, host: &dyn CustomHost, protocol_mismatch: bool, state: u32) {
    if protocol_mismatch {
        w.write_varint(-1);
        w.write_string(&host.block_state_string(state));
    } else {
        w.write_varint(state as i32);
    }
}

fn put_map(w: &mut Writer, map: &BTreeMap<u32, u32>, host: &dyn CustomHost, protocol_mismatch: bool) {
    w.write_varint(map.len() as i32);
    for (from, to) in map {
        put_state(w, host, protocol_mismatch, *from);
        put_state(w, host, protocol_mismatch, *to);
    }
}

fn put_list<T>(w: &mut Writer, items: &[T], mut each: impl FnMut(&mut Writer, &T)) {
    w.write_varint(items.len() as i32);
    for item in items {
        each(w, item);
    }
}

fn put_f32s(w: &mut Writer, values: &[f32]) {
    for value in values {
        w.write_bytes(&value.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Length-prefixed string as Writer::write_string produces it (lengths below 128 only).
    fn utf(s: &str) -> Vec<u8> {
        let mut v = vec![s.len() as u8];
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[derive(Default)]
    struct Mock {
        players: Vec<u128>,
        mismatched: Vec<u128>,
        sent: Vec<(u128, String, Vec<u8>)>,
    }

    impl CustomHost for Mock {
        fn axiom_players(&self) -> Vec<u128> {
            self.players.clone()
        }
        fn protocol_mismatch(&self, player: u128) -> bool {
            self.mismatched.contains(&player)
        }
        fn block_state_string(&self, state: u32) -> String {
            format!("minecraft:test[state={state}]")
        }
        fn send_payload(&mut self, player: u128, channel: &str, data: &[u8]) {
            self.sent.push((player, channel.to_string(), data.to_vec()));
        }
    }

    impl Mock {
        fn payloads(&self, player: u128, channel: &str) -> Vec<Vec<u8>> {
            self.sent
                .iter()
                .filter(|(p, c, _)| *p == player && c == channel)
                .map(|(_, _, d)| d.clone())
                .collect()
        }
    }

    #[test]
    fn single_block_layout() {
        let block = CustomBlockBuilder::single("a:b", "t", 5).validated().unwrap();
        let mut expected = utf("a:b");
        expected.extend(utf("t"));
        expected.extend([
            0x00, // no properties
            0x01, 0x05, // one block, state 5
            0x00, // no pick item
            0x01, 0x00, 0x00, // send pick, no right click, no shape updates
            0x00, // no placement logic
            0x01, // automatic rotation and mirroring
            0x00, 0x00, 0x00, 0x00, // four empty mappings
            0x00, // trailing zero
        ]);
        assert_eq!(block.encode(&Mock::default(), false), expected);
    }

    #[test]
    fn mismatch_uses_state_strings_and_skips_item() {
        let mut builder = CustomBlockBuilder::single("a:b", "t", 5);
        builder.pick_block_item(Some(vec![0xaa]));
        let block = builder.validated().unwrap();
        let mut expected = utf("a:b");
        expected.extend(utf("t"));
        expected.push(0x00);
        expected.extend([0x01, 0xff, 0xff, 0xff, 0xff, 0x0f]); // -1 marker
        expected.extend(utf("minecraft:test[state=5]"));
        expected.extend([0x00]); // item omitted for mismatched clients
        expected.extend([0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(block.encode(&Mock::default(), true), expected);
    }

    #[test]
    fn pick_item_is_sent_for_matching_clients() {
        let mut builder = CustomBlockBuilder::single("a:b", "t", 5);
        builder.pick_block_item(Some(vec![0xaa, 0xbb]));
        let data = builder.validated().unwrap().encode(&Mock::default(), false);
        let needle = [0x01, 0xaa, 0xbb, 0x01, 0x00, 0x00];
        assert!(data.windows(needle.len()).any(|w| w == needle));
    }

    #[test]
    fn int_and_known_property_codes() {
        let block = CustomBlockBuilder::new(
            "a:b",
            "t",
            vec![AxiomProperty::Int { name: "age".into(), min: 0, max: 2 }],
            vec![1, 2, 3],
        )
        .validated()
        .unwrap();
        let mut prefix = utf("a:b");
        prefix.extend(utf("t"));
        prefix.extend([0x01, 0x01]); // one property, int code
        prefix.extend(utf("age"));
        prefix.extend([0, 0, 0, 0, 0, 0, 0, 2]); // min and max, big endian
        prefix.extend([0x03, 0x01, 0x02, 0x03]);
        assert!(block.encode(&Mock::default(), false).starts_with(&prefix));

        assert_eq!(KnownProperty::Axis.wire_code(), 3);
        assert_eq!(KnownProperty::VerticalDirection.wire_code(), 15);
        assert_eq!(KnownProperty::Facing.num_values(), 6);
        assert_eq!(KnownProperty::HorizontalFacing.num_values(), 4);
    }

    #[test]
    fn validation_errors() {
        let wrong = CustomBlockBuilder::new("a:b", "t", vec![AxiomProperty::Known(KnownProperty::Axis)], vec![1, 2]);
        match wrong.validated() {
            Err(CustomError::WrongStateCount(msg)) => assert!(msg.contains("needed 3, got 2"), "{msg}"),
            other => panic!("unexpected {other:?}"),
        }

        let mut mapped = CustomBlockBuilder::single("a:b", "t", 5);
        mapped.override_rotate_y(7, 8);
        assert_eq!(mapped.validated(), Err(CustomError::MappingOutsideBlock("customRotateY")));

        assert!(matches!(CustomBlockBuilder::single("A:B", "t", 1).validated(), Err(CustomError::InvalidId(_))));
        assert_eq!(CustomBlockBuilder::single("stone", "t", 1).validated().unwrap().id, "minecraft:stone");

        let inverted = CustomBlockBuilder::new(
            "a:b",
            "t",
            vec![AxiomProperty::Int { name: "x".into(), min: 3, max: 1 }],
            vec![],
        );
        assert!(matches!(inverted.validated(), Err(CustomError::InvalidProperty(_))));
    }

    #[test]
    fn block_registration_rules_and_resend() {
        let mut host = Mock { players: vec![1, 2], mismatched: vec![2], ..Default::default() };
        let mut reg = CustomBlockRegistry::default();
        reg.register(&mut host, "keep", CustomBlockBuilder::single("a:b", "t", 5)).unwrap();
        reg.register(&mut host, "gone", CustomBlockBuilder::single("a:c", "t", 6)).unwrap();
        assert!(host.sent.is_empty(), "nothing is sent before a player has joined");

        assert_eq!(
            reg.register(&mut host, "x", CustomBlockBuilder::single("a:b", "t", 7)),
            Err(CustomError::AlreadyRegistered("Custom block is already registered with id a:b".into()))
        );
        assert_eq!(
            reg.register(&mut host, "x", CustomBlockBuilder::single("a:z", "t", 5)),
            Err(CustomError::AlreadyRegistered("BlockState 5 is already used by a:b".into()))
        );

        reg.on_player_join(&mut host, 1);
        assert_eq!(host.payloads(1, CHANNEL_CUSTOM_BLOCKS), vec![vec![0x00]]);
        assert_eq!(host.payloads(1, CHANNEL_REGISTER_CUSTOM_BLOCK).len(), 2);

        // A new block now goes to every Axiom player, mismatched clients get the string form.
        reg.register(&mut host, "keep", CustomBlockBuilder::single("a:d", "t", 8)).unwrap();
        let fresh = CustomBlockBuilder::single("a:d", "t", 8).validated().unwrap();
        assert_eq!(host.payloads(1, CHANNEL_REGISTER_CUSTOM_BLOCK).last().unwrap(), &fresh.encode(&host, false));
        assert_eq!(host.payloads(2, CHANNEL_REGISTER_CUSTOM_BLOCK).last().unwrap(), &fresh.encode(&host, true));

        // Unregistering "gone" re-sends the remaining list on the next tick.
        host.sent.clear();
        reg.unregister_all("gone");
        reg.tick(&mut host);
        assert_eq!(host.payloads(1, CHANNEL_CUSTOM_BLOCKS), vec![vec![0x00]]);
        assert_eq!(host.payloads(1, CHANNEL_REGISTER_CUSTOM_BLOCK).len(), 2);
        assert_eq!(host.payloads(2, CHANNEL_REGISTER_CUSTOM_BLOCK).len(), 2);

        // Nothing pending anymore.
        host.sent.clear();
        reg.tick(&mut host);
        assert!(host.sent.is_empty());
    }

    #[test]
    fn display_encoding() {
        let mut display = CustomDisplayBuilder::new("a:d", "k", vec![0x01]);
        display.set_default_translation(1.0, 0.0, 0.0);
        display.set_default_brightness(20, -3);
        let mut w = Writer::new();
        display.validated().unwrap().encode_into(&mut w);
        let mut expected = vec![0x01];
        expected.extend(utf("a:d"));
        expected.extend(utf("k"));
        expected.push(0x01); // transform present
        expected.extend(1.0f32.to_be_bytes());
        expected.extend(0.0f32.to_be_bytes());
        expected.extend(0.0f32.to_be_bytes());
        expected.extend(0.0f32.to_be_bytes()); // left rotation x
        expected.extend(0.0f32.to_be_bytes());
        expected.extend(0.0f32.to_be_bytes());
        expected.extend(1.0f32.to_be_bytes()); // left rotation w
        for v in [1.0f32; 3] {
            expected.extend(v.to_be_bytes()); // scale
        }
        expected.extend(0.0f32.to_be_bytes());
        expected.extend(0.0f32.to_be_bytes());
        expected.extend(0.0f32.to_be_bytes());
        expected.extend(1.0f32.to_be_bytes()); // right rotation w
        expected.extend([0x01, 15, 0]); // brightness clamped to 0..=15
        assert_eq!(w.into_bytes(), expected);
    }

    #[test]
    fn display_registry_sends_to_non_mismatched_players() {
        let mut host = Mock { players: vec![1, 2], mismatched: vec![2], ..Default::default() };
        let mut reg = CustomDisplayRegistry::default();
        reg.register("x", CustomDisplayBuilder::new("a:one", "k", vec![0x01])).unwrap();
        assert!(reg.register("x", CustomDisplayBuilder::new("a:one", "k", vec![])).is_err());
        reg.on_player_join(&mut host, 1);
        assert_eq!(host.payloads(1, CHANNEL_REGISTER_CUSTOM_ITEMS)[0][0], 0x01);

        reg.register("y", CustomDisplayBuilder::new("a:two", "k", vec![0x02])).unwrap();
        host.sent.clear();
        reg.tick(&mut host);
        assert!(host.payloads(2, CHANNEL_REGISTER_CUSTOM_ITEMS).is_empty());
        let data = &host.payloads(1, CHANNEL_REGISTER_CUSTOM_ITEMS)[0];
        assert_eq!(data[0], 0x02);
        assert_eq!(data, &reg.payload());
    }

    #[test]
    fn dispatch_sends_payload() {
        assert_eq!(update_dispatch_sends_payload(2, 10), vec![0x02, 0x0a]);
        assert_eq!(update_dispatch_sends_payload(-1, 10), vec![0xff, 0xff, 0xff, 0xff, 0x0f, 0x0a]);
    }

    #[derive(Default)]
    struct DebugMock {
        ops: Vec<u128>,
        perms: Vec<u128>,
    }

    impl DebugHost for DebugMock {
        fn is_op(&self, player: u128) -> bool {
            self.ops.contains(&player)
        }
        fn has_permission(&self, player: u128, node: &str) -> bool {
            self.perms.contains(&player) && node == "axiom.debug"
        }
        fn can_use_axiom(&self, _: u128) -> bool {
            true
        }
        fn is_mismatched_data_version(&self, _: u128) -> bool {
            false
        }
        fn can_modify_world(&self, _: u128) -> bool {
            true
        }
        fn is_client_listening(&self, _: u128, channel: &str) -> bool {
            channel == "foo bar"
        }
        fn restrictions(&self, _: u128) -> Option<String> {
            None
        }
        fn can_break_block(&self, _: u128, provider: Option<Provider>) -> bool {
            provider == Some(Provider::WorldGuard)
        }
        fn can_place_block(&self, _: u128, _: Option<Provider>) -> bool {
            false
        }
        fn is_plot_world(&self, _: u128) -> bool {
            false
        }
        fn current_editable_plot(&self, _: u128) -> String {
            "null".into()
        }
    }

    #[test]
    fn debug_command_text_and_permissions() {
        let host = DebugMock { ops: vec![1], perms: vec![2] };
        assert_eq!(debug_command(&host, None, "canUseAxiom"), None);
        assert_eq!(debug_command(&host, Some(3), "canUseAxiom"), None);
        assert_eq!(debug_command(&host, Some(1), "canUseAxiom").as_deref(), Some("canUseAxiom: true"));
        assert_eq!(debug_command(&host, Some(2), "canModifyWorld").as_deref(), Some("canModifyWorld: true"));
        assert_eq!(
            debug_command(&host, Some(1), "isClientListening foo bar").as_deref(),
            Some("listening to foo bar: true")
        );
        assert_eq!(debug_command(&host, Some(1), "isClientListening"), None);
        assert_eq!(debug_command(&host, Some(1), "getRestrictions").as_deref(), Some("no restrictions"));
        assert_eq!(
            debug_command(&host, Some(1), "canBreakBlockAtCurrentPosition worldguard").as_deref(),
            Some("canBreakBlock: true")
        );
        assert_eq!(debug_command(&host, Some(1), "canBreakBlockAtCurrentPosition bogus"), None);
        assert_eq!(debug_command(&host, Some(1), "nope"), None);
        assert_eq!(
            debug_command(&DebugMock::default(), Some(DEBUG_COMMAND_UUID), "isPlotWorld").as_deref(),
            Some("isPlotWorld: false")
        );
    }

    #[test]
    fn parse_id_rules() {
        assert_eq!(parse_id("a.b-c_d:f/g/h.i-j_k").unwrap(), "a.b-c_d:f/g/h.i-j_k");
        assert!(parse_id("a/b:c").is_err());
        assert!(parse_id("a:b:c").is_err());
        assert!(parse_id(":b").is_err());
        assert!(parse_id("").is_err());
    }
}
