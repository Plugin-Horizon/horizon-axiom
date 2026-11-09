// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd):
// WorldExtension.java, WorldPropertiesExample.java, world_properties/** (registry, holder, data and widget types,
// category, update result, handler), packet/impl/{SetWorldPropertyListener, SetTimePacketListener,
// SetGamemodePacketListener, SetFlySpeedPacketListener, TeleportPacketListener,
// SetNoPhysicalTriggerPacketListener}.java, event/{AxiomTimeChangeEvent, AxiomGameModeChangeEvent,
// AxiomFlySpeedChangeEvent, AxiomTeleportEvent, AxiomUnknownTeleportEvent, AxiomCreateWorldPropertiesEvent}.java.
//
// This file holds the host trait the glue implements, shared constants and codec helpers, and `WorldState`
// (the per-server state the glue owns and feeds with packets and ticks). Submodules: `properties` (registry,
// holders, data and widget types), `extension` (chunk queues and marker sync), `packets` (inbound handlers),
// `examples` (sample properties, not registered by default, as in Java).

pub mod examples;
pub mod extension;
pub mod packets;
pub mod properties;

#[cfg(test)]
mod testing;

use std::collections::{BTreeMap, BTreeSet};

use crate::buf::{BufError, Reader, Writer};

pub use extension::WorldExtension;
pub use properties::{
    Category, PropValue, PropertyUpdateResult, Widget, WorldPropertiesRegistry, WorldProperty,
};

/// Leaf permission nodes checked here (AxiomPermission.getPermissionNode()).
pub const PERM_WORLD_TIME: &str = "axiom.world.time";
pub const PERM_WORLD_PROPERTY: &str = "axiom.world.property";
pub const PERM_PLAYER_SPEED: &str = "axiom.player.speed";
pub const PERM_PLAYER_TELEPORT: &str = "axiom.player.teleport";
pub const PERM_PLAYER_GAMEMODE_SURVIVAL: &str = "axiom.player.gamemode.survival";
pub const PERM_PLAYER_GAMEMODE_CREATIVE: &str = "axiom.player.gamemode.creative";
pub const PERM_PLAYER_GAMEMODE_ADVENTURE: &str = "axiom.player.gamemode.adventure";
pub const PERM_PLAYER_GAMEMODE_SPECTATOR: &str = "axiom.player.gamemode.spectator";
pub const PERM_PLAYER_SET_NO_PHYSICAL_TRIGGER: &str = "axiom.player.set_no_physical_trigger";

/// Plugin channels owned by this module (outbound).
pub const CH_REGISTER_WORLD_PROPERTIES: &str = "axiom:register_world_properties";
pub const CH_SET_WORLD_PROPERTY: &str = "axiom:set_world_property";
pub const CH_ACK_WORLD_PROPERTIES: &str = "axiom:ack_world_properties";
pub const CH_MARKER_DATA: &str = "axiom:marker_data";

/// Chat line sent to a player whose anti-xray is on when they join (same text as Java).
pub const ANTI_XRAY_WARNING: &str =
    "Axiom: Warning, anti-xray is enabled. This will cause issues when copying blocks. Please turn anti-xray off";

/// Errors from decoding a packet or a property value. Any error means "malformed": the glue kicks the
/// player, as the Java `WrapperPacketListener` does for an exception.
#[derive(Debug)]
pub enum WorldError {
    Buf(BufError),
    /// A fixed-size field was cut short.
    Truncated,
    /// Not a valid `namespace:path` (Java `Identifier.parse` throws).
    InvalidIdentifier,
    /// A property value is shorter than its data type needs.
    InvalidValue,
    /// Two properties with the same id in one registry.
    DuplicateProperty,
}

impl From<BufError> for WorldError {
    fn from(e: BufError) -> Self {
        WorldError::Buf(e)
    }
}

/// Position and rotation of a teleport (Bukkit `Location` without the world).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Location {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

/// Game mode as sent by the client (`GameType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    /// Maps the wire byte. Out-of-range ids fall back to `Survival`, which is what vanilla `GameType.byId`
    /// does (ByIdMap with the ZERO out-of-bounds strategy). Assumed for 26.3, not checked against the jar.
    pub fn from_id(id: i8) -> Self {
        match id {
            0 => GameMode::Survival,
            1 => GameMode::Creative,
            2 => GameMode::Adventure,
            3 => GameMode::Spectator,
            _ => GameMode::Survival,
        }
    }

    pub fn id(self) -> i8 {
        match self {
            GameMode::Survival => 0,
            GameMode::Creative => 1,
            GameMode::Adventure => 2,
            GameMode::Spectator => 3,
        }
    }

    pub fn permission_node(self) -> &'static str {
        match self {
            GameMode::Survival => PERM_PLAYER_GAMEMODE_SURVIVAL,
            GameMode::Creative => PERM_PLAYER_GAMEMODE_CREATIVE,
            GameMode::Adventure => PERM_PLAYER_GAMEMODE_ADVENTURE,
            GameMode::Spectator => PERM_PLAYER_GAMEMODE_SPECTATOR,
        }
    }
}

/// One visible marker entity: its UUID and its encoded `MarkerData` (the annotation module's wire form,
/// exactly the bytes `MarkerData.write` produces). Hidden markers are not included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkerEntry {
    pub uuid: u128,
    pub encoded: Vec<u8>,
}

/// Everything this module needs from the game. Players are UUIDs, dimensions are keys such as
/// `"minecraft:overworld"`. Methods named `on_*` are cancellable events: return `false` to cancel
/// (default: not cancelled, so the glue only overrides what it needs).
pub trait WorldHost {
    // --- worlds and players ---

    /// Dimension keys of all loaded worlds.
    fn world_names(&self) -> Vec<String>;
    fn world_exists(&self, dim: &str) -> bool;
    /// Dimension the player is in, `None` if the player is not online.
    fn player_dimension(&self, player: u128) -> Option<String>;
    /// Online players currently in the dimension.
    fn players_in_dimension(&self, dim: &str) -> Vec<u128>;

    // --- permissions and restrictions ---

    /// The player completed the Axiom handshake (`AxiomPaper.canUseAxiom(player)`).
    fn is_axiom_active(&self, player: u128) -> bool;
    /// `AxiomPaper.hasPermission`: operators pass, otherwise the permission set decides (explicit allow and
    /// deny, parent inheritance). `node` is a leaf node such as `PERM_WORLD_TIME`.
    fn has_permission(&self, player: u128, node: &str) -> bool;
    /// `AxiomPaper.canModifyWorld`: whitelist and blacklist world regexes from config, plus
    /// `AxiomModifyWorldEvent`. The regex lives in the glue (no regex crate here).
    fn can_modify_world(&self, player: u128, dim: &str) -> bool;
    /// Config `allow-teleport-between-worlds`.
    fn allow_teleport_between_worlds(&self) -> bool;
    /// PlotSquared plot world. Return `false` when there is no such integration.
    fn is_plot_world(&self, dim: &str) -> bool;

    // --- time and weather ---

    fn set_day_time(&mut self, dim: &str, time: i32);
    /// `frozen == true` means gamerule `advance_time` is `false`.
    fn set_time_frozen(&mut self, dim: &str, frozen: bool);
    /// `true` when gamerule `advance_weather` is `false`.
    fn weather_paused(&self, dim: &str) -> bool;
    fn set_weather_paused(&mut self, dim: &str, paused: bool);
    /// `Server.setWeatherParameters` for this dimension.
    fn set_weather(
        &mut self,
        dim: &str,
        clear_time: i32,
        rain_time: i32,
        raining: bool,
        thundering: bool,
    );
    /// Uniform random integer in `min..=max` (vanilla `UniformInt.sample`).
    fn random_int(&mut self, min: i32, max: i32) -> i32;

    // --- player actions ---

    fn set_game_mode(&mut self, player: u128, mode: GameMode);
    fn set_fly_speed(&mut self, player: u128, speed: f32);
    fn teleport_player(&mut self, player: u128, dim: &str, at: Location);

    // --- chunks, markers, anti-xray ---

    fn is_chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool;
    /// Players that currently track the chunk (`ChunkMap.getPlayers`).
    fn players_tracking_chunk(&self, dim: &str, cx: i32, cz: i32) -> Vec<u128>;
    /// Sends the full chunk packet with light data to each of the players.
    fn send_chunk_with_light(&mut self, players: &[u128], dim: &str, cx: i32, cz: i32);
    /// Runs the light engine over these chunks (`starlight$serverRelightChunks`).
    fn relight_chunks(&mut self, dim: &str, chunks: &BTreeSet<(i32, i32)>);
    /// Visible markers of the dimension (see `MarkerEntry`).
    fn marker_snapshot(&self, dim: &str) -> Vec<MarkerEntry>;
    /// Anti-xray would modify chunks around the player (`ChunkPacketBlockController.shouldModify`).
    fn anti_xray_enabled(&self, player: u128) -> bool;

    // --- networking and chat ---

    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>);
    /// Red chat line to one player.
    fn send_warning(&mut self, player: u128, text: &str);
    /// Chat line to every player in the dimension.
    fn send_chat_to_dimension(&mut self, dim: &str, text: &str);

    // --- events (Java AxiomXxxEvent, cancellable) ---

    /// `AxiomTimeChangeEvent`. `false` cancels.
    fn on_time_change(&mut self, _player: u128, _time: Option<i32>, _freeze: Option<bool>) -> bool {
        true
    }
    /// `AxiomGameModeChangeEvent`.
    fn on_game_mode_change(&mut self, _player: u128, _mode: GameMode) -> bool {
        true
    }
    /// `AxiomFlySpeedChangeEvent`, speed already clamped.
    fn on_fly_speed_change(&mut self, _player: u128, _speed: f32) -> bool {
        true
    }
    /// `AxiomUnknownTeleportEvent`, fired before the world is looked up.
    fn on_unknown_teleport(&mut self, _player: u128, _dim: &str, _at: Location) -> bool {
        true
    }
    /// `AxiomTeleportEvent`, fired for a known world.
    fn on_teleport(&mut self, _player: u128, _dim: &str, _at: Location) -> bool {
        true
    }
    /// `AxiomCreateWorldPropertiesEvent`: called once per world with a fresh registry (built-in properties
    /// already in). Add categories with `registry.add_category(&*self, dim, ..)`. `false` means the world
    /// gets no world properties at all (cached, like Java).
    fn on_create_world_properties(
        &mut self,
        _dim: &str,
        _registry: &mut WorldPropertiesRegistry,
    ) -> bool {
        true
    }
}

/// Server-side state of this module. The glue owns one per server and calls into it.
#[derive(Default)]
pub struct WorldState {
    /// `None` means the create event was cancelled for that world.
    registries: BTreeMap<String, Option<WorldPropertiesRegistry>>,
    extensions: BTreeMap<String, WorldExtension>,
    no_physical_trigger: BTreeSet<u128>,
}

impl WorldState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `AxiomPaper.getOrCreateWorldProperties`: creates the registry (and fires the create callback) once.
    pub fn registry_or_create(
        &mut self,
        host: &mut dyn WorldHost,
        dim: &str,
    ) -> Option<&mut WorldPropertiesRegistry> {
        if !self.registries.contains_key(dim) {
            let mut registry = WorldPropertiesRegistry::new(&*host, dim);
            let allowed = host.on_create_world_properties(dim, &mut registry);
            self.registries
                .insert(dim.to_string(), allowed.then_some(registry));
        }
        self.registries.get_mut(dim).and_then(|slot| slot.as_mut())
    }

    /// Sends `axiom:register_world_properties` to one player: the registry, or the single `0` byte when the
    /// world has none (AxiomPaper sends that on join).
    pub fn send_world_properties(&mut self, host: &mut dyn WorldHost, player: u128, dim: &str) {
        let data = match self.registry_or_create(host, dim) {
            Some(registry) => registry.encode_registration(),
            None => vec![0],
        };
        host.send_payload(player, CH_REGISTER_WORLD_PROPERTIES, data);
    }

    /// `WorldExtension.onPlayerJoin`: marker snapshot for the new player and the anti-xray warning.
    /// The Java static also sends annotations (`ServerAnnotations.sendAll`) for Axiom players; that is the
    /// annotation module's job and the glue calls it.
    pub fn on_player_join(&mut self, host: &mut dyn WorldHost, player: u128, dim: &str) {
        self.extensions
            .entry(dim.to_string())
            .or_default()
            .on_player_join(host, player);
    }

    /// `WorldExtension.tick` for every loaded world (call once per server tick). Extensions of unloaded
    /// worlds are dropped.
    pub fn tick(
        &mut self,
        host: &mut dyn WorldHost,
        send_markers: bool,
        max_chunk_relights: i32,
        max_chunk_sends: i32,
    ) {
        let worlds = host.world_names();
        let live: BTreeSet<String> = worlds.iter().cloned().collect();
        self.extensions.retain(|dim, _| live.contains(dim));
        for dim in &worlds {
            self.extensions.entry(dim.clone()).or_default().tick(
                host,
                dim,
                send_markers,
                max_chunk_relights,
                max_chunk_sends,
            );
        }
    }

    /// `WorldExtension.sendChunk`: queue a chunk for a full resend.
    pub fn send_chunk(&mut self, dim: &str, cx: i32, cz: i32) {
        self.extensions
            .entry(dim.to_string())
            .or_default()
            .queue_send(cx, cz);
    }

    /// `WorldExtension.lightChunk`: queue a chunk for relighting.
    pub fn light_chunk(&mut self, dim: &str, cx: i32, cz: i32) {
        self.extensions
            .entry(dim.to_string())
            .or_default()
            .queue_light(cx, cz);
    }

    /// Drops the registry and extension of a world. Java keys the registry by the `World` object, so an
    /// unloaded and reloaded world starts fresh. The glue calls this on unload.
    pub fn forget_world(&mut self, dim: &str) {
        self.registries.remove(dim);
        self.extensions.remove(dim);
    }

    /// `ServerWorldProperty.setValue`: server-side change of a property value, synced to Axiom players in the
    /// dimension when the value changed. Returns `false` when there is no registry or no such id.
    pub fn set_property_value(
        &mut self,
        host: &mut dyn WorldHost,
        dim: &str,
        id: &str,
        value: PropValue,
    ) -> bool {
        let Some(Some(registry)) = self.registries.get_mut(dim) else {
            return false;
        };
        let Some(holder) = registry.get_mut(id) else {
            return false;
        };
        holder.set_value(host, dim, value);
        true
    }

    /// Whether the player's physical triggers (farmland, pressure plates, ...) are disabled. The glue's
    /// physical-trigger listener checks this.
    pub fn is_no_physical_trigger(&self, player: u128) -> bool {
        self.no_physical_trigger.contains(&player)
    }

    pub fn set_no_physical_trigger(&mut self, player: u128, on: bool) {
        if on {
            self.no_physical_trigger.insert(player);
        } else {
            self.no_physical_trigger.remove(&player);
        }
    }

    /// `AxiomPaper.tick` keeps only the still-active players in this set.
    pub fn retain_no_physical_trigger(&mut self, keep: impl Fn(u128) -> bool) {
        self.no_physical_trigger.retain(|p| keep(*p));
    }
}

/// `Axiom` capability check for one leaf node (`AxiomPaper.canUseAxiom(player, permission)`).
pub(crate) fn can_use(host: &dyn WorldHost, player: u128, node: &str) -> bool {
    host.is_axiom_active(player) && host.has_permission(player, node)
}

/// Validates a wire identifier like `Identifier.parse`: no colon means `minecraft`. Returns `ns:path`.
pub fn parse_identifier(s: &str) -> Result<String, WorldError> {
    let (ns, path) = match s.split_once(':') {
        Some((ns, path)) => (ns, path),
        None => ("minecraft", s),
    };
    let ns_ok = !ns.is_empty()
        && ns
            .bytes()
            .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.'));
    let path_ok = !path.is_empty()
        && path
            .bytes()
            .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/'));
    if ns_ok && path_ok {
        Ok(format!("{ns}:{path}"))
    } else {
        Err(WorldError::InvalidIdentifier)
    }
}

// --- codec helpers (fixed-width fields and validated identifiers) ---

pub(crate) fn read_identifier(r: &mut Reader<'_>) -> Result<String, WorldError> {
    let raw = r.read_identifier()?;
    parse_identifier(&raw)
}

pub(crate) fn read_byte_array(r: &mut Reader<'_>) -> Result<Vec<u8>, WorldError> {
    let max = r.remaining();
    Ok(r.read_byte_array(max)?)
}

pub(crate) fn write_byte_array(w: &mut Writer, data: &[u8]) {
    w.write_byte_array(data);
}
