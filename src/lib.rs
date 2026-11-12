//! Axiom server side (AxiomPaperPlugin) as an arismoko native trait mod. The protocol logic is the unchanged
//! `horizon-axiom-core` crate (`core/`). This file is the glue: it implements the core's host traits on the
//! `ServerHost` handle API, and routes the hooks the old mixin injections used. Engine APIs arismoko does not have
//! yet are marked `TODO(arismoko-api: <name>)` with what is lost meanwhile; PORT_NOTES.md lists them all.
//!
//! Fail-closed rule: where a write or a reply has no API, the write is refused and the reply is not sent, so the
//! Axiom client never gets a handshake and nothing in the world changes.
use horizon_axiom_core::annotation::{
    self, AnnotationHost, AnnotationState, MarkerData, MarkerEntity,
};
use horizon_axiom_core::blueprint::{self, BlueprintHost, BlueprintStore};
use horizon_axiom_core::buf::{BufError, Nbt, Reader, Writer};
use horizon_axiom_core::buffer::{zstd_compress, zstd_decompress};
use horizon_axiom_core::constants::CLIENTBOUND_CHANNELS;
use horizon_axiom_core::constants::{
    CHANNEL_TUNNEL, SERVERBOUND_CHANNELS, TUNNEL_BUFFER_FLAG_FIRST, TUNNEL_BUFFER_FLAG_LAST,
    TUNNEL_PACKET_FLAG_ZSTD_COMPRESSED,
};
use horizon_axiom_core::custom::{
    self, CustomBlockRegistry, CustomDisplayRegistry, CustomHost, DebugHost,
};
use horizon_axiom_core::entity::{self, EntityHost, EntityTypeFilter, HiddenEntities, Pos, Turn};
use horizon_axiom_core::regex::full_match;
use horizon_axiom_core::request::{
    self, EntityInfo, RequestChunksOperation, RequestHost, SectionRead,
};
use horizon_axiom_core::restrictions::{
    AxiomHandshakeEvent, AxiomModifyWorldEvent, Config as AxiomConfig, MessageColor, PlotBounds,
    RestrictionsHost, RestrictionsState, original_path,
};
use horizon_axiom_core::setblock::block_packet::{BlockHit, Hand, handle_set_block};
use horizon_axiom_core::setblock::buffer_packet::handle_buffer_packet;
use horizon_axiom_core::setblock::dispatch::DispatchBudget;
use horizon_axiom_core::setblock::tick_blocks::handle_tick_blocks;
use horizon_axiom_core::setblock::{
    BlockPos, ChunkLoadState, OperationQueue, SectionPermissionChecker, SetBlockConfig,
    SetBlockHost,
};
use horizon_axiom_core::world::packets::{set_fly_speed, set_gamemode, set_world_time, teleport};
use horizon_axiom_core::world::{
    GameMode as CoreGameMode, Location, MarkerEntry, WorldHost, WorldState,
};
use mod_api::{
    Command, CommandArgumentValue, Compound, Config, ConfigFile, ConfigKind, ConfigLoadingEvent,
    ConfigReloadingEvent, EntityRef, Flow, GameMode, GameRuleValue, Hooks, HostSide, LevelRef,
    PlayerLoggedInEvent, PlayerLoggedOutEvent, PlayerRef, RegisterCommandsEvent,
    RegisterConfigsEvent, RunModCommandEvent, ServerAboutToStartEvent, ServerHost, ServerTickEvent,
    ServerTickPhase, Tag, Text,
};
use mod_api::{
    IPayloadContext, IPayloadHandler, PayloadBytes, PayloadId, Registrar, RegistrationContext,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

const ID: &str = "axiom";

/// What a fresh server writes when there is no `plugins/AxiomPaper/config.yml`.
const DEFAULT_CONFIG: &str = "# Axiom server settings for Horizon. Keys as in AxiomPaperPlugin's config.yml; unset keys keep their defaults.\n";

/// The one UUID AxiomDebugCommand also admits to `/axiompaperdebug`, besides operators and the axiom.* nodes.
const DEBUG_EXTRA_UUID: u128 = 0xd0e0_5de7_6067_454d_beae_c6d1_9d88_6191;

/// The vanilla dimension keys.
const VANILLA_DIMENSIONS: [&str; 3] = [
    "minecraft:overworld",
    "minecraft:the_nether",
    "minecraft:the_end",
];

/// The owner the chunk tickets of Axiom's requests and buffers are held under.
const CHUNK_TICKET_OWNER: &str = "axiom:chunks";

/// Stands in for every block state id until arismoko exposes the game's numeric state ids (see `block_state_from_wire`).
/// Writes never reach it, because no wire id maps to a state yet.
const UNMAPPED_STATE: u32 = 0;

/// A random 64-bit value from the std hasher, which is seeded per process.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// What the hooks heard, in order, until the next server tick.
enum Event {
    Joined {
        player: u128,
    },
    Left {
        player: u128,
    },
    /// A client's `axiom:*` payload, queued by the raw channel inbox (`on_custom_payload`).
    Payload {
        player: u128,
        channel: String,
        data: Vec<u8>,
    },
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static CONFIG: RwLock<Option<AxiomConfig>> = RwLock::new(None);
/// Bumped whenever the config is read. The tick rebuilds the state when it changes.
static CONFIG_GEN: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The one Axiom state of this server, held by the server thread.
    static STATE: RefCell<Option<AxiomState>> = const { RefCell::new(None) };
    /// Chunks held for Axiom's requests and buffers, with the holds on each (the ticket itself is TODO).
    static CHUNK_HOLDS: RefCell<BTreeMap<(String, i32, i32), u32>> = const { RefCell::new(BTreeMap::new()) };
}

/// Per-player reassembly of split tunnel packets (`AxiomPaper.tunnelPendingBuffers` / `skipCurrentTunnelPacket`).
#[derive(Default)]
struct TunnelState {
    buffers: Vec<Vec<u8>>,
    skip: bool,
}

/// Everything the Axiom core keeps for this server, in one place.
struct AxiomState {
    generation: u64,
    config: AxiomConfig,
    restrictions: RestrictionsState,
    /// Players who completed the handshake.
    active: BTreeSet<u128>,
    tunnels: HashMap<u128, TunnelState>,
    operations: OperationQueue,
    budget: DispatchBudget,
    requests: Vec<RequestChunksOperation>,
    annotations: AnnotationState,
    blueprints: BlueprintStore,
    blueprints_loaded: bool,
    hidden: HiddenEntities,
    world: WorldState,
    custom_blocks: CustomBlockRegistry,
    custom_displays: CustomDisplayRegistry,
}

impl AxiomState {
    fn new(config: AxiomConfig, generation: u64) -> Self {
        let supported: Vec<String> = SERVERBOUND_CHANNELS
            .iter()
            .filter(|channel| served(&config, channel))
            .map(|channel| (*channel).to_owned())
            .collect();
        Self {
            generation,
            restrictions: RestrictionsState::new(config.clone(), supported, false),
            config,
            active: BTreeSet::new(),
            tunnels: HashMap::new(),
            operations: OperationQueue::new(),
            budget: DispatchBudget::new(),
            requests: Vec::new(),
            annotations: AnnotationState::default(),
            blueprints: BlueprintStore::default(),
            blueprints_loaded: false,
            hidden: HiddenEntities::new(),
            world: WorldState::new(),
            custom_blocks: CustomBlockRegistry::default(),
            custom_displays: CustomDisplayRegistry::default(),
        }
    }
}

/// Whether a serverbound channel is registered under this config (Java registers `upload_blueprint` and
/// `request_blueprint` only with `blueprint-sharing`, and `annotation_update` only with `allow-annotations`).
fn served(config: &AxiomConfig, channel: &str) -> bool {
    match channel {
        "axiom:upload_blueprint" | "axiom:request_blueprint" => config.blueprint_sharing,
        "axiom:annotation_update" => config.allow_annotations,
        _ => true,
    }
}

fn queue(event: Event) {
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

fn log(line: &str) {
    eprintln!("[axiom] {line}");
}

/// The settings in force become `config`. A new config rebuilds the core state, so every client handshakes again.
fn store(config: AxiomConfig) {
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Some(config);
    CONFIG_GEN.fetch_add(1, Ordering::Relaxed);
}

/// The settings in force, or the core's defaults before a config was read.
fn with_config<R>(read: impl FnOnce(&AxiomConfig) -> R) -> R {
    let guard = CONFIG.read().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(config) => read(config),
        None => read(&AxiomConfig::default()),
    }
}

/// `config/axiom-server.toml` (or the fallback registered when no original file exists) was read.
fn toml_changed(text: &str, file: &str) {
    match AxiomConfig::from_toml(text) {
        Ok(config) => store(config),
        Err(e) => log(&format!("{file}: {e}; keeping the settings in force")),
    }
}

/// The original `plugins/AxiomPaper/config.yml` was found: read it with the core's loader. A file that does not
/// parse keeps the settings in force.
fn original_config_read(path: &Path) {
    match AxiomConfig::read_original(path) {
        Ok((config, _read)) => store(config),
        Err(e) => log(&format!("{e}; keeping the settings in force")),
    }
}

/// Java `canModifyWorld`'s two checks for world `dim`: the whitelist, when set, must match, and the blacklist, when
/// set, must not. A pattern that does not compile fails both checks.
fn world_filters_pass(config: &AxiomConfig, dim: &str) -> bool {
    let name = paper_world_name(dim);
    let blank = |pattern: &str| pattern.trim().is_empty();
    (blank(&config.whitelist_world_regex)
        || full_match(&config.whitelist_world_regex, name) == Ok(true))
        && (blank(&config.blacklist_world_regex)
            || full_match(&config.blacklist_world_regex, name) == Ok(false))
}

/// The name the world filters match against: the vanilla world folder names Paper uses.
fn paper_world_name(dim: &str) -> &str {
    match dim {
        "minecraft:overworld" => "world",
        "minecraft:the_nether" => "world_nether",
        "minecraft:the_end" => "world_the_end",
        other => other,
    }
}

/// The play protocol of the game this mod is built for (26.3); Axiom's handshake compares the client's.
const GAME_PROTOCOL: i32 = 777;
/// Permission nodes a player gets without an operator level. Only these two are guessed from the core's default
/// chain (`axiom.default`, `axiom.use`); check AxiomPaper's plugin.yml.
const DEFAULT_TRUE_NODES: [&str; 2] = ["axiom.default", "axiom.use"];

/// The permission check: a permission provider's answer for the node (LuckPerms, through `player_permission_check`),
/// else operator level 2, the level the Paper ops of this network have. Default nodes pass for everyone.
fn permission_fallback(host: &dyn ServerHost, uuid: u128, node: &str) -> bool {
    if DEFAULT_TRUE_NODES.contains(&node) {
        return true;
    }
    host.player(uuid)
        .is_some_and(|p| p.has_permission(host, node))
}

/// Java `Player.isOp` as far as arismoko says it: a permission level of 1 and up.
fn is_op_level(host: &dyn ServerHost, uuid: u128) -> bool {
    host.player(uuid)
        .and_then(|p| p.permission_level(host))
        .is_some_and(|level| level >= 1)
}

/// Java `AxiomPaper.onAxiomActive` and the handshake, as one state for the whole server.
fn with_state<R>(f: impl FnOnce(&mut Option<AxiomState>) -> R) -> R {
    STATE.with(|cell| f(&mut cell.borrow_mut()))
}

/// The active players, the settings and the restrictions text of `player`, for a command that runs outside the tick.
fn snapshot(player: u128) -> (BTreeSet<u128>, AxiomConfig, Option<String>) {
    with_state(|slot| match slot.as_ref() {
        Some(state) => (
            state.active.clone(),
            state.config.clone(),
            state.restrictions.restrictions_text(player),
        ),
        None => (BTreeSet::new(), with_config(|c| c.clone()), None),
    })
}

/// Server tick start: answers what the hooks queued, in order, then runs the core's per-tick work.
fn tick(host: &mut dyn ServerHost) {
    let events = std::mem::take(&mut *EVENTS.lock().unwrap_or_else(|e| e.into_inner()));
    let generation = CONFIG_GEN.load(Ordering::Relaxed);
    with_state(|slot| {
        if slot.as_ref().is_none_or(|s| s.generation != generation) {
            *slot = Some(AxiomState::new(with_config(|c| c.clone()), generation));
        }
        let Some(state) = slot.as_mut() else {
            return;
        };
        for event in events {
            match event {
                // Java sends its initial packets once the client has handshaked (`axiom:hello`), not on join.
                Event::Joined { .. } => {}
                Event::Left { player } => left_player(state, player),
                Event::Payload {
                    player,
                    channel,
                    data,
                } => {
                    let mut h =
                        AxiomHost::new(&mut *host, state.active.clone(), state.config.clone());
                    payload_in(&mut h, state, player, &channel, &data);
                }
            }
        }
        run_ticks(&mut *host, state);
    });
}

/// A player left: drop the parts of the state that only they own.
fn left_player(state: &mut AxiomState, player: u128) {
    state.tunnels.remove(&player);
    state.requests.retain(|op| op.executor() != player);
    state.world.set_no_physical_trigger(player, false);
    state.active.remove(&player);
}

/// The core's per-tick work, in Java's order.
fn run_ticks(host: &mut dyn ServerHost, state: &mut AxiomState) {
    let mut h = AxiomHost::new(host, state.active.clone(), state.config.clone());
    if !state.blueprints_loaded {
        // TODO(arismoko-api: mod_data_dir): the blueprint folder lives in the mod's own data dir, which arismoko does
        // not hand out yet. The store stays empty: uploads and requests find nothing.
        state.blueprints_loaded = true;
    }
    state.restrictions.tick(&mut h);
    sync_active(&mut h, state);
    state.operations.tick(&mut h);
    for op in state.requests.iter_mut() {
        op.tick(&mut h);
    }
    state.requests.retain(|op| !op.is_finished());
    let (markers, relights, sends) = (
        h.config.send_markers,
        h.config.max_chunk_relights_per_tick,
        h.config.max_chunk_sends_per_tick,
    );
    state.world.tick(&mut h, markers, relights, sends);
    for dim in VANILLA_DIMENSIONS {
        annotation::tick_markers(&mut h, &mut state.annotations, dim);
    }
    state.custom_blocks.tick(&mut h);
    state.custom_displays.tick(&mut h);
    state.hidden.tick(&mut h);
    sync_active(&mut h, state);
}

/// Copies the restrictions' active set into the host and the state, after anything that may have changed it.
fn sync_active(host: &mut AxiomHost<'_>, state: &mut AxiomState) {
    host.active = host
        .online_uuids()
        .into_iter()
        .filter(|p| state.restrictions.is_active(*p))
        .collect();
    state.active = host.active.clone();
}

/// Routes one payload the client sent on an `axiom:*` channel: the tunnel, or a packet on its own channel.
fn payload_in(
    host: &mut AxiomHost<'_>,
    state: &mut AxiomState,
    player: u128,
    channel: &str,
    data: &[u8],
) {
    if channel == CHANNEL_TUNNEL {
        tunnel_buffer(host, state, player, data);
        return;
    }
    if !SERVERBOUND_CHANNELS.contains(&channel) || !served(&host.config, channel) {
        return;
    }
    if let Err(e) = route(host, state, player, channel, data) {
        log(&format!("WARN {channel} from {player:032x}: {e}"));
        host.kick_player(player, &format!("Axiom: Malformed {channel} packet"));
    }
}

/// `axiom:tunnel`: one buffer of a packet that may be split (`AxiomPaper.handleTunnelBuffer`).
fn tunnel_buffer(host: &mut AxiomHost<'_>, state: &mut AxiomState, player: u128, data: &[u8]) {
    if !host.active.contains(&player) {
        return;
    }
    if data.len() > host.config.tunnel_split_size.max(0) as usize {
        host.kick_player(player, "Axiom: Sent split buffer that was too large");
        return;
    }
    let Some(&flags) = data.first() else {
        host.kick_player(player, "Axiom: Malformed tunnel buffer");
        return;
    };
    let first = flags & TUNNEL_BUFFER_FLAG_FIRST != 0;
    let last = flags & TUNNEL_BUFFER_FLAG_LAST != 0;
    if first && last {
        tunnel_packet(host, state, player, &data[1..]);
        return;
    }
    let combined = {
        let tunnel = state.tunnels.entry(player).or_default();
        if first {
            tunnel.skip = false;
            tunnel.buffers.clear();
        } else if tunnel.skip {
            return;
        } else if tunnel.buffers.is_empty() {
            tunnel.skip = true;
            return;
        }
        tunnel.buffers.push(data.to_vec());
        if !last {
            return;
        }
        let combined: Vec<u8> = tunnel
            .buffers
            .iter()
            .flat_map(|b| b[1..].iter().copied())
            .collect();
        tunnel.buffers.clear();
        combined
    };
    tunnel_packet(host, state, player, &combined);
}

/// One whole tunnel packet: identifier, uncompressed size, flags, body (`AxiomPaper.handleTunnelPacket`).
fn tunnel_packet(host: &mut AxiomHost<'_>, state: &mut AxiomState, player: u128, data: &[u8]) {
    let mut r = Reader::new(data);
    let Ok(id) = r.read_identifier() else {
        host.kick_player(player, "Axiom: Malformed tunnel packet");
        return;
    };
    let (Ok(uncompressed), Ok(flags)) = (r.read_i32(), r.read_u8()) else {
        host.kick_player(player, "Axiom: Malformed tunnel packet");
        return;
    };
    if !SERVERBOUND_CHANNELS.contains(&id.as_str())
        || id == CHANNEL_TUNNEL
        || !served(&host.config, &id)
    {
        host.kick_player(player, "Axiom: Sent packet that was unregistered");
        return;
    }
    if uncompressed > host.config.maximum_tunnel_packet_size {
        host.kick_player(player, "Axiom: Sent packet was too large");
        return;
    }
    let raw = match r.read_bytes(r.remaining()) {
        Ok(raw) => raw,
        Err(_) => {
            host.kick_player(player, "Axiom: Malformed tunnel packet");
            return;
        }
    };
    let body = if flags & TUNNEL_PACKET_FLAG_ZSTD_COMPRESSED != 0 {
        let decoded = usize::try_from(uncompressed)
            .map_err(err)
            .and_then(|size| zstd_decompress(&raw, size).map_err(err));
        match decoded {
            Ok(body) => body,
            Err(e) => {
                log(&format!(
                    "WARN {id} from {player:032x}: zstd tunnel packet: {e}"
                ));
                host.kick_player(player, "Axiom: Malformed tunnel packet");
                return;
            }
        }
    } else {
        raw
    };
    if let Err(e) = route(host, state, player, &id, &body) {
        log(&format!("WARN {id} from {player:032x}: {e}"));
        host.kick_player(player, &format!("Axiom: Failed to read {id} packet"));
    }
}

/// Stringifies a core error (all of them derive Debug).
fn err<E: std::fmt::Debug>(e: E) -> String {
    format!("{e:?}")
}

/// Routes one packet body to its core handler. `channel` is the full name, e.g. `axiom:set_block`.
fn route(
    host: &mut AxiomHost<'_>,
    state: &mut AxiomState,
    player: u128,
    channel: &str,
    body: &[u8],
) -> Result<(), String> {
    match channel {
        "axiom:hello" => {
            let before = state.restrictions.is_active(player);
            state
                .restrictions
                .on_hello(host, player, body)
                .map_err(err)?;
            sync_active(host, state);
            if !before && state.restrictions.is_active(player) {
                send_initial(host, state, player);
            }
        }
        "axiom:set_gamemode" => set_gamemode(host, player, body).map_err(err)?,
        "axiom:set_fly_speed" => set_fly_speed(host, player, body).map_err(err)?,
        "axiom:teleport" => teleport(host, player, body).map_err(err)?,
        "axiom:set_world_time" => set_world_time(host, player, body).map_err(err)?,
        "axiom:set_no_physical_trigger" => state
            .world
            .on_set_no_physical_trigger(host, player, body)
            .map_err(err)?,
        "axiom:set_world_property" => state
            .world
            .on_set_world_property(host, player, body)
            .map_err(err)?,
        "axiom:request_chunk_data" => {
            if let Some(op) = request::handle_chunk_request(host, player, body).map_err(err)? {
                state.requests.push(op);
            }
        }
        "axiom:request_entity_data" => {
            host.index_dimension_of(player);
            request::handle_entity_request(host, player, body).map_err(err)?;
        }
        "axiom:spawn_entity" | "axiom:manipulate_entity" | "axiom:delete_entity" => {
            let dim = host
                .dimension_of(player)
                .unwrap_or_else(|| VANILLA_DIMENSIONS[0].to_owned());
            host.index_dimension(&dim);
            let outcome = match channel {
                "axiom:spawn_entity" => entity::handle_spawn(host, player, &dim, body),
                "axiom:manipulate_entity" => entity::handle_manipulate(host, player, &dim, body),
                _ => entity::handle_delete(host, player, &dim, body),
            };
            outcome.map_err(err)?;
        }
        "axiom:marker_nbt_request" => {
            annotation::handle_marker_nbt_request(host, player, body).map_err(err)?
        }
        "axiom:set_block" => {
            let cfg = host.set_block_config();
            let mut r = Reader::new(body);
            handle_set_block(host, &cfg, player, &mut r).map_err(err)?;
        }
        "axiom:set_buffer" => {
            let cfg = host.set_block_config();
            let mut r = Reader::new(body);
            handle_buffer_packet(
                host,
                &cfg,
                &mut state.budget,
                &mut state.operations,
                player,
                &mut r,
            )
            .map_err(err)?;
        }
        "axiom:tick_blocks" => {
            let mut r = Reader::new(body);
            handle_tick_blocks(host, player, &mut r).map_err(err)?;
        }
        "axiom:upload_blueprint" => {
            if let Err(e) = blueprint::handle_upload(host, &mut state.blueprints, player, body) {
                log(&format!("WARN upload_blueprint from {player:032x}: {e:?}"));
            }
        }
        "axiom:request_blueprint" => {
            if let Err(e) = blueprint::handle_request(host, &state.blueprints, player, body) {
                log(&format!("WARN request_blueprint from {player:032x}: {e:?}"));
            }
        }
        "axiom:annotation_update" => {
            annotation::handle_update_packet(host, &mut state.annotations, player, body)
                .map_err(err)?
        }
        other => return Err(format!("no handler for {other}")),
    }
    Ok(())
}

/// What Java's `AxiomPaper.sendInitialPackets` sends once a player is active. Heightmaps are not sent (GAP).
fn send_initial(host: &mut AxiomHost<'_>, state: &mut AxiomState, player: u128) {
    let dim = host
        .dimension_of(player)
        .unwrap_or_else(|| VANILLA_DIMENSIONS[0].to_owned());
    state.world.send_world_properties(host, player, &dim);
    state.world.on_player_join(host, player, &dim);
    if let Err(e) = blueprint::send_manifest(host, &state.blueprints, &[player]) {
        log(&format!("WARN blueprint manifest for {player:032x}: {e:?}"));
    }
    if let Err(e) = annotation::send_all(host, &mut state.annotations, &dim, player) {
        log(&format!("WARN annotations for {player:032x}: {e:?}"));
    }
    state.custom_blocks.on_player_join(host, player);
    state.custom_displays.on_player_join(host, player);
}

/// A fresh random UUID for a passenger of a spawn tree.
fn fresh_uuid() -> u128 {
    (u128::from(random_u64()) << 64) | u128::from(random_u64())
}

/// Where a detached spawn tree is posed.
#[derive(Clone, Copy, Debug, Default)]
struct Pose {
    pos: (f64, f64, f64),
    yaw: f32,
    pitch: f32,
    head_yaw: f32,
}

/// A spawn tree the core built but has not added yet.
struct PendingTree {
    dim: String,
    nodes: Vec<entity::DetachedEntity>,
    pose: Pose,
}

/// A block state as the core numbers it (the game's numeric state id). See `block_state_from_wire`.
type StateId = u32;

/// The game as the core sees it, for one call. Players are UUIDs, dimensions are ids like `minecraft:overworld`.
struct AxiomHost<'a> {
    host: &'a mut dyn ServerHost,
    active: BTreeSet<u128>,
    config: AxiomConfig,
    filter: EntityTypeFilter,
    /// Entities of the dimensions indexed for this call: client-visible uuid to (dimension, engine handle).
    entities: HashMap<u128, (String, EntityRef)>,
    /// The restrictions text of the player who runs `/axiompaperdebug`.
    debug_restrictions: Option<(u128, String)>,
    /// Spawn trees the core built detached, keyed by the root's uuid, until they are added or dropped.
    pending: HashMap<u128, PendingTree>,
}

impl<'a> AxiomHost<'a> {
    fn new(host: &'a mut dyn ServerHost, active: BTreeSet<u128>, config: AxiomConfig) -> Self {
        let filter = EntityTypeFilter::new(
            config.whitelist_entities.clone(),
            config.blacklist_entities.clone(),
        );
        Self {
            host,
            active,
            config,
            filter,
            entities: HashMap::new(),
            debug_restrictions: None,
            pending: HashMap::new(),
        }
    }

    fn player(&self, uuid: u128) -> Option<PlayerRef> {
        self.host.player(uuid)
    }

    fn online_uuids(&self) -> Vec<u128> {
        self.host
            .players()
            .server_players()
            .into_iter()
            .map(|p| p.uuid())
            .collect()
    }

    fn dimension_of(&self, player: u128) -> Option<String> {
        self.player(player)?
            .level(&*self.host)
            .map(|l| l.dimension_id().to_string())
    }

    fn position_of(&self, player: u128) -> Option<[f64; 3]> {
        self.player(player)?.entity().position(&*self.host)
    }

    fn player_name(&self, player: u128) -> String {
        self.player(player)
            .and_then(|p| p.name(&*self.host))
            .unwrap_or_default()
    }

    fn level(&self, dim: &str) -> Option<LevelRef> {
        self.host.level(dim)
    }

    fn kick_player(&mut self, player: u128, reason: &str) {
        let Some(p) = self.player(player) else {
            return;
        };
        if let Err(e) = p.kick(&mut *self.host, Text::literal(reason)) {
            log(&format!("kick {player:032x} refused: {e:?}"));
        }
    }

    fn send_raw(&mut self, player: u128, channel: &str, data: &[u8]) {
        let Some(p) = self.player(player) else {
            return;
        };
        let id = PayloadId(channel.to_owned());
        if let Err(e) = p.send_payload(&mut *self.host, &id, PayloadBytes(data.to_vec())) {
            log(&format!("cannot send {channel} to {player:032x}: {e:?}"));
        }
    }

    fn tell(&mut self, player: u128, text: &str) {
        // TODO(arismoko-api: text_colour): colours are dropped; every line is plain text.
        if let Some(p) = self.player(player) {
            let _ = p.tell(&mut *self.host, Text::literal(text));
        }
    }

    /// Whether `dimension` is a level the server has loaded.
    fn here(&self, dimension: &str) -> bool {
        self.level(dimension).is_some()
    }

    /// The height range of `dim` as `(min_y, height)`, from the level handle.
    fn height_of(&self, dim: &str) -> (i32, i32) {
        self.level(dim)
            .and_then(|l| l.height(&*self.host))
            .unwrap_or((-64, 384))
    }

    /// Whether the chunk is loaded in the level `dim`.
    fn chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool {
        self.level(dim)
            .is_some_and(|l| l.is_loaded(&*self.host, [cx * 16, 0, cz * 16]))
    }

    /// Settings of the block packets, as `SetBlockPacketListener` reads them.
    fn set_block_config(&self) -> SetBlockConfig {
        SetBlockConfig {
            packet_collection_read_limit: self.config.packet_collection_read_limit() as usize,
            log_large_block_buffer_changes: self.config.log_large_block_buffer_changes,
        }
    }

    /// Holds chunk `(cx, cz)` of `dim` for Axiom. TODO(arismoko-api: level_request_chunk_ticket): the engine ticket is
    /// not requested, so far chunks may unload during a request.
    fn hold_chunk(&mut self, dim: &str, cx: i32, cz: i32) {
        CHUNK_HOLDS.with(|holds| {
            *holds
                .borrow_mut()
                .entry((dim.to_owned(), cx, cz))
                .or_insert(0) += 1
        });
    }

    /// Lets go of one hold on chunk `(cx, cz)` of `dim`.
    fn release_chunk_hold(&mut self, dim: &str, cx: i32, cz: i32) {
        CHUNK_HOLDS.with(|holds| {
            let mut holds = holds.borrow_mut();
            let key = (dim.to_owned(), cx, cz);
            match holds.get_mut(&key) {
                Some(count) if *count > 1 => *count -= 1,
                Some(_) => {
                    holds.remove(&key);
                }
                None => {}
            }
        });
    }

    /// The block state at `pos`, from a loaded chunk. Every state reads as `UNMAPPED_STATE` (see `block_state_from_wire`).
    fn state_at(&self, dim: &str, pos: [i32; 3]) -> Option<StateId> {
        if !self.chunk_loaded(dim, pos[0] >> 4, pos[2] >> 4) {
            return None;
        }
        Some(UNMAPPED_STATE)
    }

    /// Indexes the entities of `dim` by uuid, for the calls of this host. Every entity of the level is visited.
    fn index_dimension(&mut self, dim: &str) {
        let Some(level) = self.level(dim) else {
            return;
        };
        for entity in level.all_entities(&*self.host).unwrap_or_default() {
            if let Some(uuid) = entity.uuid(&mut *self.host) {
                self.entities.insert(uuid, (dim.to_owned(), entity));
            }
        }
    }

    fn index_dimension_of(&mut self, player: u128) {
        if let Some(dim) = self.dimension_of(player) {
            self.index_dimension(&dim);
        }
    }

    fn entity_of(&self, uuid: u128) -> Option<EntityRef> {
        self.entities.get(&uuid).map(|(_, e)| e.clone())
    }

    /// The detached spawn tree that holds `uuid` (its root or a passenger), if the core is building one.
    fn pending_tree(&self, uuid: u128) -> Option<&PendingTree> {
        self.pending
            .values()
            .find(|tree| tree.nodes.iter().any(|node| node.uuid == uuid))
    }

    fn pending_tree_mut(&mut self, uuid: u128) -> Option<&mut PendingTree> {
        self.pending
            .values_mut()
            .find(|tree| tree.nodes.iter().any(|node| node.uuid == uuid))
    }

    /// Reads a section of the chunk. TODO(arismoko-api: chunk_section_read): arismoko has `read_columns` with a
    /// different shape, so sections read as missing and the request gets no blocks.
    fn section_read(&self, _dimension: &str, _cx: i32, _sy: i32, _cz: i32) -> SectionRead {
        SectionRead::Missing
    }
}

/// `Tag` (the engine's NBT) as the core reads it.
fn tag_to_core(tag: &Tag) -> Nbt {
    match tag {
        Tag::End => Nbt::End,
        Tag::Byte(v) => Nbt::Byte(*v),
        Tag::Short(v) => Nbt::Short(*v),
        Tag::Int(v) => Nbt::Int(*v),
        Tag::Long(v) => Nbt::Long(*v),
        Tag::Float(bits) => Nbt::Float(f32::from_bits(*bits)),
        Tag::Double(bits) => Nbt::Double(f64::from_bits(*bits)),
        Tag::ByteArray(v) => Nbt::ByteArray(v.iter().map(|b| *b as i8).collect()),
        Tag::String(s) => Nbt::String(String::from_utf16_lossy(&s.0)),
        Tag::List(v) => Nbt::List(v.iter().map(tag_to_core).collect()),
        Tag::Compound(c) => Nbt::Compound(
            c.iter()
                .map(|(k, v)| (String::from_utf16_lossy(&k.0), tag_to_core(v)))
                .collect(),
        ),
        Tag::IntArray(v) => Nbt::IntArray(v.clone()),
        Tag::LongArray(v) => Nbt::LongArray(v.clone()),
    }
}

/// The core's NBT as the engine reads it.
fn core_to_tag(nbt: &Nbt) -> Tag {
    match nbt {
        Nbt::End => Tag::End,
        Nbt::Byte(v) => Tag::Byte(*v),
        Nbt::Short(v) => Tag::Short(*v),
        Nbt::Int(v) => Tag::Int(*v),
        Nbt::Long(v) => Tag::Long(*v),
        Nbt::Float(v) => Tag::Float(v.to_bits()),
        Nbt::Double(v) => Tag::Double(v.to_bits()),
        Nbt::ByteArray(v) => Tag::ByteArray(v.iter().map(|b| *b as u8).collect()),
        Nbt::String(s) => Tag::String(s.as_str().into()),
        Nbt::List(v) => Tag::List(v.iter().map(core_to_tag).collect()),
        Nbt::Compound(c) => Tag::Compound(
            c.iter()
                .map(|(k, v)| (k.as_str().into(), core_to_tag(v)))
                .collect::<Compound>(),
        ),
        Nbt::IntArray(v) => Tag::IntArray(v.clone()),
        Nbt::LongArray(v) => Tag::LongArray(v.clone()),
    }
}

/// The direction offsets of `BlockHit::direction`, in Java's `Direction` order (down, up, north, south, west, east).
fn direction_offset(direction: u8) -> (i32, i32, i32) {
    match direction {
        0 => (0, -1, 0),
        1 => (0, 1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        _ => (1, 0, 0),
    }
}

impl RestrictionsHost for AxiomHost<'_> {
    fn is_op(&self, player: u128) -> bool {
        is_op_level(&*self.host, player)
    }
    fn has_permission(&self, player: u128, node: &str) -> bool {
        permission_fallback(&*self.host, player, node)
    }
    fn permission_value(&self, player: u128, node: &str) -> Option<bool> {
        // None when no permission provider decided the node.
        self.player(player)
            .and_then(|p| p.permission_value(&*self.host, node))
    }
    fn is_online(&self, player: u128) -> bool {
        self.player(player).is_some()
    }
    fn online_players(&self) -> Vec<u128> {
        self.online_uuids()
    }
    fn world_name(&self, player: u128) -> String {
        self.dimension_of(player).unwrap_or_default()
    }
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.send_raw(player, channel, &data);
    }
    fn send_message(&mut self, player: u128, text: &str, _color: MessageColor) {
        self.tell(player, text);
    }
    fn kick(&mut self, player: u128, reason: &str) {
        self.kick_player(player, reason);
    }
    fn random_i64(&mut self) -> i64 {
        random_u64() as i64
    }
    fn server_protocol_version(&self) -> i32 {
        // TODO(arismoko-api: game_protocol_version): no host method yet; the mod is built for game 26.3
        // (`[native] game` in horizon-mod.toml), whose protocol is 777.
        GAME_PROTOCOL
    }
    fn server_data_version(&self) -> i32 {
        self.host.server_data_version()
    }
    fn plugin_enabled(&self, _name: &str) -> bool {
        // Horizon runs no Bukkit plugins, and ViaVersion is not part of arismoko (no client-version adapter here).
        false
    }
    fn protocol_name(&self, protocol: i32) -> String {
        // TODO(arismoko-api: game_version_names): numbers only.
        protocol.to_string()
    }
    fn map_client_block_registry(&mut self, _player: u128, _protocol: i32) -> Result<(), String> {
        // TODO(arismoko-api: client_block_registry): no per-release block mapping, so older clients are refused.
        Err("arismoko has no block mapping for older clients yet".to_owned())
    }
    fn is_plot_world(&self, _player: u128) -> bool {
        false
    }
    fn editable_plot(&self, _player: u128) -> Option<PlotBounds> {
        None
    }
    fn matches_regex(&self, pattern: &str, text: &str) -> bool {
        full_match(pattern, paper_world_name(text)).unwrap_or(false)
    }
    fn on_axiom_handshake(&mut self, _event: &mut AxiomHandshakeEvent) {}
    fn on_modify_world(&mut self, _event: &mut AxiomModifyWorldEvent) {}
}

impl WorldHost for AxiomHost<'_> {
    fn world_names(&self) -> Vec<String> {
        VANILLA_DIMENSIONS.iter().map(|d| (*d).to_owned()).collect()
    }
    fn world_exists(&self, dim: &str) -> bool {
        VANILLA_DIMENSIONS.contains(&dim)
    }
    fn player_dimension(&self, player: u128) -> Option<String> {
        self.dimension_of(player)
    }
    fn players_in_dimension(&self, dim: &str) -> Vec<u128> {
        self.level(dim)
            .and_then(|l| l.online_players(&*self.host))
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.uuid())
            .collect()
    }
    fn is_axiom_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }
    fn has_permission(&self, player: u128, node: &str) -> bool {
        permission_fallback(&*self.host, player, node)
    }
    fn can_modify_world(&self, player: u128, dim: &str) -> bool {
        self.modify_world_allowed(player, dim)
    }
    fn allow_teleport_between_worlds(&self) -> bool {
        self.config.allow_teleport_between_worlds
    }
    fn is_plot_world(&self, _dim: &str) -> bool {
        false
    }
    fn set_day_time(&mut self, dim: &str, time: i32) {
        if let Some(level) = self.level(dim) {
            level.set_clock_time(&mut *self.host, i64::from(time));
        }
    }
    fn set_time_frozen(&mut self, dim: &str, frozen: bool) {
        if let Some(level) = self.level(dim) {
            level.set_game_rule(
                &mut *self.host,
                "advance_time",
                GameRuleValue::Bool(!frozen),
            );
        }
    }
    fn weather_paused(&self, dim: &str) -> bool {
        match self
            .level(dim)
            .and_then(|l| l.game_rule(&*self.host, "advance_weather").ok().flatten())
        {
            Some(GameRuleValue::Bool(advancing)) => !advancing,
            _ => false,
        }
    }
    fn set_weather_paused(&mut self, dim: &str, paused: bool) {
        if let Some(level) = self.level(dim) {
            level.set_game_rule(
                &mut *self.host,
                "advance_weather",
                GameRuleValue::Bool(!paused),
            );
        }
    }
    fn set_weather(
        &mut self,
        dim: &str,
        clear_time: i32,
        rain_time: i32,
        raining: bool,
        thundering: bool,
    ) {
        if let Some(level) = self.level(dim) {
            level.set_weather(&mut *self.host, clear_time, rain_time, raining, thundering);
        }
    }
    fn random_int(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        let span = (max as i64 - min as i64 + 1) as u64;
        (min as i64 + (random_u64() % span) as i64) as i32
    }
    fn set_game_mode(&mut self, player: u128, mode: CoreGameMode) {
        let mode = match mode {
            CoreGameMode::Survival => GameMode::Survival,
            CoreGameMode::Creative => GameMode::Creative,
            CoreGameMode::Adventure => GameMode::Adventure,
            CoreGameMode::Spectator => GameMode::Spectator,
        };
        if let Some(p) = self.player(player) {
            p.set_game_mode(&mut *self.host, mode);
        }
    }
    fn set_fly_speed(&mut self, player: u128, speed: f32) {
        // The fly speed lives in the player's abilities (`player_set_abilities`).
        if let Some(p) = self.player(player) {
            if let Some(mut abilities) = p.abilities(&*self.host) {
                abilities.fly_speed = speed;
                p.set_abilities(&mut *self.host, abilities);
            }
        }
    }
    fn teleport_player(&mut self, player: u128, dim: &str, at: Location) {
        let Some(p) = self.player(player) else {
            return;
        };
        let entity = p.entity();
        let pos = [at.x, at.y, at.z];
        let rotation = Some([at.yaw, at.pitch]);
        if self.dimension_of(player).as_deref() == Some(dim) {
            entity.teleport_to(&mut *self.host, pos, rotation);
        } else {
            entity.teleport_to_level(&mut *self.host, dim, pos, rotation);
        }
    }
    fn is_chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool {
        self.chunk_loaded(dim, cx, cz)
    }
    fn players_tracking_chunk(&self, dim: &str, cx: i32, cz: i32) -> Vec<u128> {
        let Some(level) = self.level(dim) else {
            return Vec::new();
        };
        self.host
            .chunk(level, [cx, cz])
            .and_then(|c| c.watchers(&*self.host))
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.uuid())
            .collect()
    }
    fn send_chunk_with_light(&mut self, players: &[u128], dim: &str, cx: i32, cz: i32) {
        // `chunk.resend` sends the whole chunk to every watcher at tick end, not only to `players`; a watcher who
        // holds the chunk just gets it again.
        if players.is_empty() {
            return;
        }
        if let Some(level) = self.level(dim) {
            let chunk = self.host.chunk(level, [cx, cz]);
            if let Some(chunk) = chunk {
                chunk.resend(&mut *self.host);
            }
        }
    }
    fn relight_chunks(&mut self, dim: &str, chunks: &BTreeSet<(i32, i32)>) {
        let list: Vec<[i32; 2]> = chunks.iter().map(|&(x, z)| [x, z]).collect();
        if list.is_empty() {
            return;
        }
        if let Some(level) = self.level(dim) {
            level.relight_chunks(&mut *self.host, &list);
        }
    }
    fn marker_snapshot(&mut self, _dim: &str) -> Vec<MarkerEntry> {
        // TODO(arismoko-api: level_markers): markers are entities the port cannot list yet.
        Vec::new()
    }
    fn anti_xray_enabled(&self, _player: u128) -> bool {
        false
    }
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.send_raw(player, channel, &data);
    }
    fn send_warning(&mut self, player: u128, text: &str) {
        self.tell(player, text);
    }
    fn send_chat_to_dimension(&mut self, dim: &str, text: &str) {
        for player in WorldHost::players_in_dimension(self, dim) {
            self.tell(player, text);
        }
    }
}

impl AxiomHost<'_> {
    /// Java `canModifyWorld` as `setblock::can_modify_world` gives it: the config's world filters, then the player's own
    /// world (the cancelled default of the event).
    fn modify_world_allowed(&self, player: u128, dim: &str) -> bool {
        world_filters_pass(&self.config, dim) && self.dimension_of(player).as_deref() == Some(dim)
    }

    /// The vanilla "may build" flag, which is all the region checks have on arismoko.
    /// TODO(arismoko-api: region_may_build): WorldGuard-style regions and plot checks are not seen; a player who may
    /// build anywhere is allowed here.
    fn may_build(&self, player: u128) -> bool {
        self.player(player)
            .and_then(|p| p.may_build(&*self.host))
            .unwrap_or(false)
    }
}

impl SetBlockHost for AxiomHost<'_> {
    fn player_dimension(&self, player: u128) -> String {
        self.dimension_of(player)
            .unwrap_or_else(|| VANILLA_DIMENSIONS[0].to_owned())
    }
    fn player_block_pos(&self, player: u128) -> BlockPos {
        let [x, y, z] = self.position_of(player).unwrap_or([0.0; 3]);
        (x.floor() as i32, y.floor() as i32, z.floor() as i32)
    }
    fn scoreboard_name(&self, player: u128) -> String {
        self.player_name(player)
    }
    fn is_axiom_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }
    fn has_axiom_permission(&self, player: u128, node: &str) -> bool {
        permission_fallback(&*self.host, player, node)
    }
    fn has_bukkit_permission(&self, player: u128, node: &str) -> bool {
        permission_fallback(&*self.host, player, node)
    }
    fn can_use_game_master_blocks(&self, player: u128) -> bool {
        self.player(player)
            .and_then(|p| p.permission_level(&*self.host))
            .is_some_and(|level| level >= 2)
    }
    fn allowed_dispatch_sends_per_second(&self, player: u128) -> i64 {
        for (key, sends) in self.config.dispatch_send_overrides() {
            if permission_fallback(&*self.host, player, &format!("axiomlimits.{key}")) {
                return sends as i64;
            }
        }
        self.config.default_dispatch_sends_per_second() as i64
    }
    fn kick(&mut self, player: u128, reason: &str) {
        self.kick_player(player, reason);
    }
    fn send_system_message(&mut self, player: u128, text: &str, _red: bool) {
        self.tell(player, text);
    }
    fn broadcast_system_message(&mut self, text: &str) {
        let _ = self.host.messages().broadcast(Text::literal(text));
    }
    fn log_info(&mut self, text: &str) {
        log(text);
    }
    fn world_allowed_by_config(&self, dim: &str) -> bool {
        world_filters_pass(&self.config, dim)
    }
    fn modify_world_event(&mut self, _player: u128, _dim: &str, cancelled: bool) -> bool {
        !cancelled
    }
    fn max_chunk_load_distance(&self, _dim: &str) -> i32 {
        self.config.max_chunk_load_distance_for(false)
    }
    fn block_state_from_wire(&self, _player: u128, _wire_id: i32) -> Option<u32> {
        // TODO(arismoko-api: block_state_registry_id): arismoko's BlockState is process-local, with no numeric id that the
        // client's wire ids can map to. Every edit that names a state is refused here, so nothing is written.
        None
    }
    fn is_air(&self, _state: u32) -> bool {
        true
    }
    fn empty_state(&self) -> u32 {
        UNMAPPED_STATE
    }
    fn global_bits(&self) -> u8 {
        // TODO(arismoko-api: block_state_registry_size): vanilla 26.3 width, assumed.
        15
    }
    fn same_block(&self, a: u32, b: u32) -> bool {
        a == b
    }
    fn state_can_be_replaced(&self, state: u32) -> bool {
        self.is_air(state)
    }
    fn has_block_entity(&self, _state: u32) -> bool {
        false
    }
    fn fluid_is_empty(&self, _state: u32) -> bool {
        true
    }
    fn is_liquid_block(&self, _state: u32) -> bool {
        false
    }
    fn is_bed_double_plant_or_door(&self, _state: u32) -> bool {
        false
    }
    fn light_properties_differ(&self, old: u32, new: u32) -> bool {
        old != new
    }
    fn block_entity_must_be_removed(&self, _old: u32, _new: u32) -> bool {
        false
    }
    fn is_game_master_block(&self, _dim: &str, _pos: BlockPos) -> bool {
        // TODO(arismoko-api: block_state_names): the name-based check needs the state's block name, which is not mapped.
        false
    }
    fn update_from_neighbour_shapes(&self, _dim: &str, _pos: BlockPos, state: u32) -> u32 {
        state
    }
    fn is_block_loaded(&self, dim: &str, pos: BlockPos) -> bool {
        self.state_at(dim, [pos.0, pos.1, pos.2]).is_some()
    }
    fn is_chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool {
        self.chunk_loaded(dim, cx, cz)
    }
    fn request_chunk_load(&mut self, dim: &str, cx: i32, cz: i32) {
        self.hold_chunk(dim, cx, cz);
    }
    fn release_chunk_load(&mut self, dim: &str, cx: i32, cz: i32) {
        self.release_chunk_hold(dim, cx, cz);
    }
    fn chunk_load_state(&self, dim: &str, cx: i32, cz: i32) -> ChunkLoadState {
        if self.chunk_loaded(dim, cx, cz) {
            ChunkLoadState::Loaded
        } else {
            ChunkLoadState::Pending
        }
    }
    fn section_y_range(&self, dim: &str) -> (i32, i32) {
        let (min_y, height) = self.height_of(dim);
        (min_y.div_euclid(16), (min_y + height).div_euclid(16) - 1)
    }
    fn section_has_only_air(&self, _dim: &str, _cx: i32, _cy: i32, _cz: i32) -> bool {
        false
    }
    fn block_state(&self, dim: &str, pos: BlockPos) -> u32 {
        self.state_at(dim, [pos.0, pos.1, pos.2])
            .unwrap_or(UNMAPPED_STATE)
    }
    fn set_block_raw(&mut self, _dim: &str, _pos: BlockPos, _state: u32) -> u32 {
        // Unreachable while `block_state_from_wire` refuses every state (TODO(arismoko-api: block_state_registry_id)).
        UNMAPPED_STATE
    }
    fn update_heightmaps(&mut self, _dim: &str, _pos: BlockPos, _new_state: u32) {}
    fn mark_chunk_unsaved(&mut self, _dim: &str, _cx: i32, _cz: i32) {}
    fn mark_block_changed(&mut self, dim: &str, pos: BlockPos) {
        if let Some(level) = self.level(dim) {
            level.send_block_updated(&mut *self.host, [pos.0, pos.1, pos.2]);
        }
    }
    fn check_block_light(&mut self, _dim: &str, _pos: BlockPos) {}
    fn update_section_emptiness(
        &mut self,
        _dim: &str,
        _cx: i32,
        _cy: i32,
        _cz: i32,
        _has_only_air: bool,
    ) {
    }
    fn update_poi(&mut self, _dim: &str, _pos: BlockPos, _old: u32, _new: u32) {}
    fn remove_block_entity(&mut self, _dim: &str, _pos: BlockPos) {}
    fn sync_block_entity(&mut self, dim: &str, pos: BlockPos, _state: u32) -> bool {
        match self.level(dim) {
            Some(level) => self
                .host
                .block_entity(level, [pos.0, pos.1, pos.2])
                .is_some(),
            None => false,
        }
    }
    fn load_block_entities(&mut self, _dim: &str, entries: &[(BlockPos, Vec<u8>)]) {
        // TODO(arismoko-api: block_entity_nbt_write): edit_blocks takes no NBT on arismoko, so block entities are dropped.
        if !entries.is_empty() {
            log(&format!(
                "{} block entit(ies) not written: TODO(arismoko-api: block_entity_nbt_write)",
                entries.len()
            ));
        }
    }
    fn send_chunk(&mut self, _dim: &str, _cx: i32, _cz: i32) {}
    fn light_chunk(&mut self, dim: &str, cx: i32, cz: i32) {
        self.relight_chunks(dim, &BTreeSet::from([(cx, cz)]));
    }
    fn biome_exists(&self, key: &str) -> bool {
        key.contains(':')
    }
    fn set_biome_quart(&mut self, dim: &str, qx: i32, qy: i32, qz: i32, key: &str) {
        // A biome quart is four blocks; the engine refuses an unknown biome id, and the core asks one cell at a time.
        if let Some(level) = self.level(dim) {
            let _ = level.set_biome(&mut *self.host, [qx * 4, qy * 4, qz * 4], key);
        }
    }
    fn players_tracking_chunk(&self, dim: &str, cx: i32, cz: i32) -> Vec<u128> {
        WorldHost::players_tracking_chunk(self, dim, cx, cz)
    }
    fn send_chunk_biomes(&mut self, _player: u128, _dim: &str, _chunks: &[(i32, i32)]) {}
    fn region_can_break(&self, player: u128, _dim: &str, _pos: BlockPos) -> bool {
        self.may_build(player)
    }
    fn region_can_place(&self, player: u128, _dim: &str, _pos: BlockPos) -> bool {
        self.may_build(player)
    }
    fn region_section(
        &self,
        player: u128,
        _dim: &str,
        _cx: i32,
        _cy: i32,
        _cz: i32,
    ) -> SectionPermissionChecker {
        if self.may_build(player) {
            SectionPermissionChecker::All
        } else {
            SectionPermissionChecker::from_allowed_boxes(Vec::new())
        }
    }
    fn ack_block_changes(&mut self, player: u128, sequence: i32) {
        if let Some(p) = self.player(player) {
            p.ack_block_changes(&mut *self.host, sequence);
        }
    }
    fn fire_player_interact(
        &mut self,
        _player: u128,
        _breaking: bool,
        _hit: &BlockHit,
        _hand: Hand,
        _dim: &str,
    ) -> bool {
        true
    }
    fn fire_block_place(
        &mut self,
        _player: u128,
        _hand: Hand,
        _dim: &str,
        _against: BlockPos,
        _placed: &[(BlockPos, u32)],
    ) -> bool {
        true
    }
    fn clicked_pos(&self, _player: u128, _hand: Hand, dim: &str, hit: &BlockHit) -> BlockPos {
        let (x, y, z) = hit.pos;
        let replaceable = self
            .state_at(dim, [x, y, z])
            .is_none_or(|state| self.state_can_be_replaced(state));
        if replaceable {
            hit.pos
        } else {
            let (dx, dy, dz) = direction_offset(hit.direction);
            (x + dx, y + dy, z + dz)
        }
    }
    fn apply_item_to_block_entity(
        &mut self,
        _player: u128,
        _hand: Hand,
        _dim: &str,
        _pos: BlockPos,
    ) {
    }
    fn set_placed_by(
        &mut self,
        _player: u128,
        _hand: Hand,
        _dim: &str,
        _pos: BlockPos,
        _state: u32,
    ) {
    }
    fn set_block(&mut self, _dim: &str, _pos: BlockPos, _state: u32, _flags: u32) {
        // Unreachable while `block_state_from_wire` refuses every state (TODO(arismoko-api: block_state_registry_id)).
    }
    fn tick_fluid(&mut self, _dim: &str, _pos: BlockPos, _state: u32) {}
    fn tick_block(&mut self, _dim: &str, _pos: BlockPos, _state: u32) {}
}

impl EntityHost for AxiomHost<'_> {
    fn is_axiom_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }
    fn has_axiom_permission(&self, player: u128, node: &str) -> bool {
        permission_fallback(&*self.host, player, node)
    }
    fn can_modify_world(&self, player: u128, dim: &str) -> bool {
        self.modify_world_allowed(player, dim)
    }
    fn packet_collection_read_limit(&self) -> usize {
        self.config.packet_collection_read_limit() as usize
    }
    fn disable_entity_sanitization(&self) -> bool {
        self.config.disable_entity_sanitization
    }
    fn entity_type_filter(&self) -> &EntityTypeFilter {
        &self.filter
    }
    fn read_tag_unknown(
        &self,
        _player: u128,
        reader: &mut Reader,
    ) -> Result<Option<Nbt>, BufError> {
        let nbt = reader.read_nbt()?;
        Ok(if matches!(nbt, Nbt::End) {
            None
        } else {
            Some(nbt)
        })
    }
    fn active_axiom_players(&self) -> Vec<u128> {
        self.active.iter().copied().collect()
    }
    fn send_payload(&mut self, player: u128, channel: &str, data: &[u8]) {
        self.send_raw(player, channel, data);
    }
    fn can_place_block(&self, player: u128, _dim: &str, _x: i32, _y: i32, _z: i32) -> bool {
        self.may_build(player)
    }
    fn can_break_block(&self, player: u128, _dim: &str, _x: i32, _y: i32, _z: i32) -> bool {
        self.may_build(player)
    }
    fn is_in_spawnable_bounds(&self, x: i32, y: i32, z: i32) -> bool {
        (-33_554_432..33_554_432).contains(&x)
            && (-2048..2048).contains(&y)
            && (-33_554_432..33_554_432).contains(&z)
    }
    fn fire_spawn(&mut self, _player: u128, _uuid: u128) -> bool {
        // TODO(arismoko-api: entity_spawn_event): no cancel hook; nothing vetoes an Axiom spawn.
        false
    }
    fn fire_manipulate(&mut self, _player: u128, _uuid: u128) -> bool {
        false
    }
    fn fire_after_manipulate(&mut self, _player: u128, _uuid: u128) {}
    fn fire_remove(&mut self, _player: u128, _uuid: u128) -> bool {
        false
    }
    fn has_entity(&self, dim: &str, uuid: u128) -> bool {
        self.entities.get(&uuid).is_some_and(|(d, _)| d == dim)
    }
    fn entity_type_id(&self, uuid: u128) -> Option<String> {
        self.entity_of(uuid)?.kind(&*self.host)
    }
    fn type_can_serialize(&self, type_id: &str) -> bool {
        type_id != entity::TYPE_PLAYER
    }
    fn position(&self, uuid: u128) -> Pos {
        if let Some(tree) = self.pending_tree(uuid) {
            return tree.pose.pos;
        }
        let [x, y, z] = self
            .entity_of(uuid)
            .and_then(|e| e.position(&*self.host))
            .unwrap_or([0.0; 3]);
        (x, y, z)
    }
    fn yaw(&self, uuid: u128) -> f32 {
        match self.pending_tree(uuid) {
            Some(tree) => tree.pose.yaw,
            None => self.rotation_of(uuid)[0],
        }
    }
    fn pitch(&self, uuid: u128) -> f32 {
        match self.pending_tree(uuid) {
            Some(tree) => tree.pose.pitch,
            None => self.rotation_of(uuid)[1],
        }
    }
    fn is_passenger(&self, uuid: u128) -> bool {
        self.entity_of(uuid)
            .and_then(|e| e.is_passenger(&*self.host))
            .unwrap_or(false)
    }
    fn vehicle(&self, _uuid: u128) -> Option<u128> {
        // TODO(arismoko-api: entity_uuid_read): the vehicle's uuid needs `entity_uuid`, a mutable host call, and this
        // read is `&self`; reads say no vehicle.
        None
    }
    fn passengers(&self, _uuid: u128) -> Vec<u128> {
        // TODO(arismoko-api: entity_uuid_read): as `vehicle` (`entity_uuid` needs `&mut` host).
        Vec::new()
    }
    fn is_removed(&self, uuid: u128) -> bool {
        !self.entities.contains_key(&uuid)
    }
    fn item_frame_on_y_axis(&self, _uuid: u128) -> bool {
        // TODO(arismoko-api: entity_set_hanging_direction): hanging facing has no read yet.
        false
    }
    fn entity_as_passenger_nbt(&mut self, dim: &str, uuid: u128) -> Option<Nbt> {
        self.saved_entity(dim, uuid)
    }
    fn load_entity_tree(
        &mut self,
        dim: &str,
        tag: &Nbt,
        root_uuid: u128,
    ) -> Option<entity::EntityNode> {
        let (node, nodes) = entity::build_detached_tree(tag, root_uuid, &mut fresh_uuid)?;
        self.pending.insert(
            root_uuid,
            PendingTree {
                dim: dim.to_owned(),
                nodes,
                pose: Pose::default(),
            },
        );
        Some(node)
    }
    fn set_hanging_turn(&mut self, _uuid: u128, _turn: Turn) {
        // TODO(arismoko-api: entity_set_hanging_direction): hanging facing is not set.
    }
    fn adjust_item_frame_rotation(&mut self, _uuid: u128, _delta: i32) {
        // TODO(arismoko-api: item_frame_set_rotation): as `set_hanging_turn`.
    }
    fn snap_to(&mut self, uuid: u128, pos: Pos, yaw: f32, pitch: f32) {
        if let Some(tree) = self.pending_tree_mut(uuid) {
            tree.pose.pos = pos;
            tree.pose.yaw = yaw;
            tree.pose.pitch = pitch;
            return;
        }
        if let Some(entity) = self.entity_of(uuid) {
            entity.teleport_to(&mut *self.host, [pos.0, pos.1, pos.2], Some([yaw, pitch]));
        }
    }
    fn set_head_yaw(&mut self, uuid: u128, yaw: f32) {
        if let Some(tree) = self.pending_tree_mut(uuid) {
            tree.pose.head_yaw = yaw;
            return;
        }
        if let Some(entity) = self.entity_of(uuid) {
            entity.set_head_yaw(&mut *self.host, yaw);
        }
    }
    fn set_yaw(&mut self, uuid: u128, yaw: f32) {
        if let Some(tree) = self.pending_tree_mut(uuid) {
            tree.pose.yaw = yaw;
            return;
        }
        let pitch = self.pitch(uuid);
        if let Some(entity) = self.entity_of(uuid) {
            entity.set_rotation(&mut *self.host, yaw, pitch);
        }
    }
    fn set_pitch(&mut self, uuid: u128, pitch: f32) {
        if let Some(tree) = self.pending_tree_mut(uuid) {
            tree.pose.pitch = pitch;
            return;
        }
        let yaw = self.yaw(uuid);
        if let Some(entity) = self.entity_of(uuid) {
            entity.set_rotation(&mut *self.host, yaw, pitch);
        }
    }
    fn set_pos_raw(&mut self, uuid: u128, pos: Pos) {
        if let Some(tree) = self.pending_tree_mut(uuid) {
            tree.pose.pos = pos;
            return;
        }
        // TODO(arismoko-api: entity_move_raw): moved with a teleport, which runs the entity's own move rules.
        let (yaw, pitch) = (self.yaw(uuid), self.pitch(uuid));
        if let Some(entity) = self.entity_of(uuid) {
            entity.teleport_to(&mut *self.host, [pos.0, pos.1, pos.2], Some([yaw, pitch]));
        }
    }
    fn save_without_id(&mut self, uuid: u128) -> Nbt {
        match self.entity_data(uuid) {
            Some(tag) => tag_to_core(&tag),
            None => Nbt::Compound(BTreeMap::new()),
        }
    }
    fn load_nbt(&mut self, uuid: u128, tag: &Nbt) {
        let Tag::Compound(compound) = core_to_tag(tag) else {
            return;
        };
        if let Some(entity) = self.entity_of(uuid) {
            entity.merge_nbt(&mut *self.host, compound);
        }
    }
    fn eject_passengers(&mut self, uuid: u128) {
        if let Some(entity) = self.entity_of(uuid) {
            entity.eject_passengers(&mut *self.host);
        }
    }
    fn start_riding(&mut self, passenger: u128, vehicle: u128) {
        if let (Some(rider), Some(vehicle)) = (self.entity_of(passenger), self.entity_of(vehicle)) {
            rider.start_riding(&mut *self.host, vehicle, false);
        }
    }
    fn stop_riding(&mut self, passenger: u128) {
        if let Some(rider) = self.entity_of(passenger) {
            rider.stop_riding(&mut *self.host);
        }
    }
    fn add_fresh_entity_with_passengers(&mut self, _dim: &str, uuid: u128) -> bool {
        // TODO(port): `host.summon` takes NBT with `UUID:[I;..]` (the API exists); the spawn tree and its
        // passenger wiring are not written yet, so the tree is refused. It is dropped and nothing is summoned.
        self.pending.remove(&uuid);
        false
    }
    fn remove(&mut self, uuid: u128) {
        if self.pending.remove(&uuid).is_none() {
            for tree in self.pending.values_mut() {
                tree.nodes.retain(|node| node.uuid != uuid);
            }
        }
        if let Some((_, entity)) = self.entities.remove(&uuid) {
            entity.discard(&mut *self.host);
        }
    }
}

impl AxiomHost<'_> {
    /// The NBT of a loaded entity, with its `id`, as a save would give it.
    fn saved_entity(&mut self, dimension: &str, uuid: u128) -> Option<Nbt> {
        let (dim, entity) = self.entities.get(&uuid)?.clone();
        if dim.as_str() != dimension {
            return None;
        }
        let kind = entity.kind(&*self.host)?;
        let mut data = entity.nbt(&mut *self.host)?;
        data.insert("id".into(), Tag::String(kind.as_str().into()));
        Some(tag_to_core(&Tag::Compound(data)))
    }

    /// The entity's NBT as a tag, or None.
    fn entity_data(&mut self, uuid: u128) -> Option<Tag> {
        let entity = self.entity_of(uuid)?;
        entity.nbt(&mut *self.host).map(Tag::Compound)
    }

    /// The entity's `[yaw, pitch]`, or zeros.
    fn rotation_of(&self, uuid: u128) -> [f32; 2] {
        self.entity_of(uuid)
            .and_then(|e| e.rotation(&*self.host))
            .unwrap_or([0.0; 2])
    }
}

impl RequestHost for AxiomHost<'_> {
    fn is_axiom_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }
    fn has_permission(&self, player: u128, node: &str) -> bool {
        permission_fallback(&*self.host, player, node)
    }
    fn is_mismatched_data_version(&self, _player: u128) -> bool {
        false
    }
    fn is_player_online(&self, player: u128) -> bool {
        self.player(player).is_some()
    }
    fn player_dimension(&self, player: u128) -> Option<String> {
        self.dimension_of(player)
    }
    fn player_block_pos(&self, player: u128) -> (i32, i32, i32) {
        let [x, y, z] = self.position_of(player).unwrap_or([0.0; 3]);
        (x.floor() as i32, y.floor() as i32, z.floor() as i32)
    }
    fn can_modify_world(&self, player: u128, dimension: &str) -> bool {
        self.modify_world_allowed(player, dimension)
    }
    fn can_place_block(&self, player: u128, _dimension: &str, _x: i32, _y: i32, _z: i32) -> bool {
        self.may_build(player)
    }
    fn max_chunk_load_distance(&self, _dimension: &str) -> i32 {
        self.config.max_chunk_load_distance_for(false)
    }
    fn packet_collection_read_limit(&self) -> usize {
        self.config.packet_collection_read_limit() as usize
    }
    fn can_entity_be_manipulated(&self, entity_type: &str) -> bool {
        entity_type != request::PLAYER_ENTITY_TYPE
    }
    fn build_height(&self, dimension: &str) -> (i32, i32) {
        self.height_of(dimension)
    }
    fn block_state_global_bits(&self) -> u8 {
        self.global_bits()
    }
    fn request_chunk(&mut self, dimension: &str, chunk_x: i32, chunk_z: i32) {
        self.hold_chunk(dimension, chunk_x, chunk_z);
    }
    fn release_chunk(&mut self, dimension: &str, chunk_x: i32, chunk_z: i32) {
        self.release_chunk_hold(dimension, chunk_x, chunk_z);
    }
    fn is_chunk_ready(&self, dimension: &str, chunk_x: i32, chunk_z: i32) -> bool {
        self.chunk_loaded(dimension, chunk_x, chunk_z)
    }
    fn section_states(
        &self,
        dimension: &str,
        chunk_x: i32,
        section_y: i32,
        chunk_z: i32,
    ) -> SectionRead {
        self.section_read(dimension, chunk_x, section_y, chunk_z)
    }
    fn block_entity_nbt(&mut self, _dimension: &str, _x: i32, _y: i32, _z: i32) -> Option<Nbt> {
        // TODO(arismoko-api: block_entity_nbt_read): block entity NBT is not readable here yet.
        None
    }
    fn section_block_entities(
        &mut self,
        _dimension: &str,
        _chunk_x: i32,
        _section_y: i32,
        _chunk_z: i32,
    ) -> Vec<(i32, i32, i32, Nbt)> {
        // TODO(arismoko-api: block_entity_nbt_read): as `block_entity_nbt`.
        Vec::new()
    }
    fn entity_info(&self, dimension: &str, uuid: u128) -> Option<EntityInfo> {
        let (dim, entity) = self.entities.get(&uuid)?;
        if dim.as_str() != dimension {
            return None;
        }
        let entity_type = entity.kind(&*self.host)?;
        let [x, y, z] = entity.position(&*self.host)?.map(|v| v.floor() as i32);
        Some(EntityInfo {
            entity_type,
            block_pos: (x, y, z),
        })
    }
    fn entity_save_nbt(&mut self, dimension: &str, uuid: u128) -> Option<Nbt> {
        self.saved_entity(dimension, uuid)
    }
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.send_raw(player, channel, &data);
    }
}

impl BlueprintHost for AxiomHost<'_> {
    fn data_version(&self) -> i32 {
        0
    }
    fn default_state(&self, _block: &str) -> u32 {
        UNMAPPED_STATE
    }
    fn global_state_bits(&self) -> u8 {
        self.global_bits()
    }
    fn state_to_nbt(&self, _state: u32) -> Nbt {
        // TODO(arismoko-api: block_state_numeric_id): a state id has no name here, so blueprints cannot save states.
        Nbt::End
    }
    fn state_from_nbt(&self, _nbt: &Nbt) -> Option<u32> {
        // TODO(arismoko-api: block_state_registry_id): blueprint states cannot be resolved, so they are refused.
        None
    }
    fn block_entity_type_of(&self, _state: u32) -> Option<String> {
        None
    }
    fn block_entity_accepts(&self, _type_id: &str, _state: u32) -> bool {
        false
    }
    fn upgrade_block_state(&self, _nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        Err("no block state data fixer on arismoko yet".to_owned())
    }
    fn upgrade_block_entity(&self, _nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        Err("no block entity data fixer on arismoko yet (TODO(arismoko-api: upgrade_block_entity_nbt))".to_owned())
    }
    fn upgrade_entity(&self, _nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        Err(
            "no entity data fixer on arismoko yet (TODO(arismoko-api: upgrade_entity_nbt))"
                .to_owned(),
        )
    }
    fn compress_block_entity(&self, raw: &[u8]) -> Vec<u8> {
        zstd_compress(raw)
    }
    fn decompress_block_entity(
        &self,
        compressed: &[u8],
        original_size: usize,
    ) -> Result<Vec<u8>, String> {
        zstd_decompress(compressed, original_size).map_err(err)
    }
    fn can_use_axiom(&self, player: u128, permission: &str) -> bool {
        self.active.contains(&player) && permission_fallback(&*self.host, player, permission)
    }
    fn is_mismatched_data_version(&self, _player: u128) -> bool {
        false
    }
    fn game_version_name(&self) -> String {
        // TODO(arismoko-api: game_version_names): the server runs Minecraft 26.3.
        "26.3".to_owned()
    }
    fn send_message(&mut self, player: u128, text: &str) {
        self.tell(player, text);
    }
    fn kick(&mut self, player: u128, reason: &str) {
        self.kick_player(player, reason);
    }
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.send_raw(player, channel, &data);
    }
    fn online_players(&self) -> Vec<u128> {
        self.online_uuids()
    }
}

impl AnnotationHost for AxiomHost<'_> {
    fn allow_annotations(&self) -> bool {
        self.config.allow_annotations
    }
    fn send_markers(&self) -> bool {
        self.config.send_markers
    }
    fn world_folder(&self, _dim: &str) -> Option<PathBuf> {
        // TODO(arismoko-api: world_dir): `host.world_dir()` gives the level root; the per-dimension folder mapping for
        // annotations (Paper 26 `dimensions/...` layout) is not verified yet.
        None
    }
    fn player_dimension(&self, player: u128) -> Option<String> {
        self.dimension_of(player)
    }
    fn world_players(&self, dim: &str) -> Vec<u128> {
        WorldHost::players_in_dimension(self, dim)
    }
    fn can_use_axiom(&self, player: u128) -> bool {
        self.active.contains(&player)
    }
    fn has_permission(&self, player: u128, node: &str) -> bool {
        permission_fallback(&*self.host, player, node)
    }
    fn can_modify_world(&self, player: u128, dim: &str) -> bool {
        self.modify_world_allowed(player, dim)
    }
    fn markers(&mut self, _dim: &str) -> Vec<MarkerEntity> {
        // TODO(arismoko-api: level_markers): as `marker_snapshot`.
        Vec::new()
    }
    fn marker_custom_data(&mut self, _dim: &str, _uuid: u128) -> Option<Nbt> {
        None
    }
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.send_raw(player, channel, &data);
    }
}

impl CustomHost for AxiomHost<'_> {
    fn axiom_players(&self) -> Vec<u128> {
        self.active.iter().copied().collect()
    }
    fn protocol_mismatch(&self, _player: u128) -> bool {
        false
    }
    fn block_state_string(&self, _state: u32) -> String {
        // TODO(arismoko-api: block_state_numeric_id): the name of a numeric state is not known yet.
        "minecraft:air".to_owned()
    }
    fn send_payload(&mut self, player: u128, channel: &str, data: &[u8]) {
        self.send_raw(player, channel, data);
    }
}

impl DebugHost for AxiomHost<'_> {
    fn is_op(&self, player: u128) -> bool {
        is_op_level(&*self.host, player)
    }
    fn has_permission(&self, player: u128, node: &str) -> bool {
        permission_fallback(&*self.host, player, node)
    }
    fn can_use_axiom(&self, player: u128) -> bool {
        self.active.contains(&player)
    }
    fn is_mismatched_data_version(&self, _player: u128) -> bool {
        false
    }
    fn can_modify_world(&self, player: u128) -> bool {
        self.dimension_of(player)
            .is_some_and(|dim| self.modify_world_allowed(player, &dim))
    }
    fn is_client_listening(&self, player: u128, channel: &str) -> bool {
        self.player(player)
            .and_then(|p| p.announced_channels(&*self.host))
            .is_some_and(|list| list.iter().any(|c| c == channel))
    }
    fn restrictions(&self, player: u128) -> Option<String> {
        self.debug_restrictions
            .as_ref()
            .filter(|(sender, _)| *sender == player)
            .map(|(_, text)| text.clone())
    }
    fn can_break_block(
        &self,
        player: u128,
        _provider: Option<horizon_axiom_core::custom::Provider>,
    ) -> bool {
        self.may_build(player)
    }
    fn can_place_block(
        &self,
        player: u128,
        _provider: Option<horizon_axiom_core::custom::Provider>,
    ) -> bool {
        self.may_build(player)
    }
    fn is_plot_world(&self, _player: u128) -> bool {
        false
    }
    fn current_editable_plot(&self, _player: u128) -> String {
        String::new()
    }
}

/// `/axiompaperdebug`: the core's `debug_command` answers the subcommands, for an operator or a player with axiom.*.
fn debug(host: &mut dyn ServerHost, uuid: u128, args: &str) -> String {
    let (active, config, restrictions) = snapshot(uuid);
    let mut h = AxiomHost::new(host, active, config);
    h.debug_restrictions = restrictions.map(|text| (uuid, text));
    custom::debug_command(&h, Some(uuid), args)
        .unwrap_or_else(|| "This /axiompaperdebug subcommand is not known.".to_owned())
}

/// The text of a command's `args:text` argument, or empty.
fn text_argument(event: &RunModCommandEvent<'_>, name: &str) -> String {
    match event.command.argument(name) {
        Some(CommandArgumentValue::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Config text from the game (the registered TOML fallback, or nothing for the Paper file).
fn config_file(file: &ConfigFile) -> bool {
    file.mod_id == ID && matches!(file.kind, ConfigKind::Server)
}

#[derive(Default)]
struct Mod;

#[mod_api::horizon_mod]
impl Hooks for Mod {
    /// Declares the Axiom channels as raw play channels (arismoko #3209): serverbound ones with the inbox,
    /// clientbound ones send-only.
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        let mut handlers = r.payload_handlers();
        let mut registrar = handlers.registrar("1");
        for channel in SERVERBOUND_CHANNELS {
            let inbox: Arc<dyn IPayloadHandler> = Arc::new(Inbox(channel.to_string()));
            registrar.play_raw(PayloadId(channel.to_string()), Some(inbox));
        }
        // A channel used both ways (`axiom:hello`, ...) is declared once, with the inbox; a second declaration
        // refuses load.
        for channel in CLIENTBOUND_CHANNELS.iter().filter(|c| !SERVERBOUND_CHANNELS.contains(c)) {
            registrar.play_raw(PayloadId(channel.to_string()), None);
        }
    }

    fn register_configs(&self, _: HostSide, event: &mut RegisterConfigsEvent<'_>) {
        // The original plugins/AxiomPaper/config.yml is the settings when it exists (read in server_about_to_start);
        // the TOML fallback is registered only without it, so the game does not write a file the Paper one replaces.
        if original_path(Path::new(".")).is_file() {
            return;
        }
        event
            .configs
            .push(Config::new(ID, ConfigKind::Server, DEFAULT_CONFIG));
    }

    fn config_loading(&self, _: HostSide, event: &mut ConfigLoadingEvent<'_>) {
        if config_file(&event.config) {
            toml_changed(&event.config.text, &event.config.file);
        }
    }

    fn config_reloading(&self, _: HostSide, event: &mut ConfigReloadingEvent<'_>) {
        if config_file(&event.config) {
            toml_changed(&event.config.text, &event.config.file);
        }
    }

    fn server_about_to_start(&self, _: &mut dyn ServerHost, _: &mut ServerAboutToStartEvent) {
        let original = original_path(Path::new("."));
        if original.is_file() {
            original_config_read(&original);
        }
    }

    fn register_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        // TODO(arismoko-api: command_zero_args): the debug command is registered with a text argument; check that
        // `/axiompaperdebug` with no argument reaches the handler.
        event
            .commands
            .push(Command::op("axiompaperdebug <args:text>", 1));
        event
            .commands
            .push(Command::op("axiompapermigrateconfig", 4));
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        let reply = |host: &mut dyn ServerHost, line: String| {
            let _ = host.commands().feedback(Text::literal(line));
        };
        match event.command.literals.first().map(String::as_str) {
            Some("axiompapermigrateconfig") => {
                // The Paper config is read as it is, so there is nothing to migrate.
                reply(host, "Axiom on Horizon reads plugins/AxiomPaper/config.yml as it is; there is nothing to migrate.".into());
                Flow::Cancel
            }
            Some("axiompaperdebug") => {
                let Some(uuid) = host.players().acting_player().map(|p| p.uuid()) else {
                    reply(host, "Only a player can run /axiompaperdebug.".into());
                    return Flow::Cancel;
                };
                let allowed = is_op_level(&*host, uuid)
                    || permission_fallback(&*host, uuid, "axiom.all")
                    || permission_fallback(&*host, uuid, "axiom.debug")
                    || uuid == DEBUG_EXTRA_UUID;
                if !allowed {
                    reply(
                        host,
                        "You do not have permission to use this command.".into(),
                    );
                    return Flow::Cancel;
                }
                let args = text_argument(event, "args");
                let text = debug(&mut *host, uuid, args.trim());
                reply(host, text);
                Flow::Cancel
            }
            _ => Flow::Continue,
        }
    }

    fn player_logged_in(&self, _host: &mut dyn ServerHost, event: &mut PlayerLoggedInEvent<'_>) {
        queue(Event::Joined {
            player: event.player,
        });
    }

    fn player_logged_out(&self, _host: &mut dyn ServerHost, event: &mut PlayerLoggedOutEvent<'_>) {
        queue(Event::Left {
            player: event.player,
        });
    }

    fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>) {
        if !matches!(event.phase, ServerTickPhase::ServerTickStart { .. }) {
            return;
        }
        tick(host);
    }
}

/// A client's `axiom:*` payload, as the raw channel inbox hears it: queued for the tick, which routes it through
/// `payload_in`. The physical-trigger cancel for no-physical-trigger players is still TODO(arismoko-api:
/// player_physical_trigger).
fn on_custom_payload(player: u128, channel: &str, data: &[u8]) {
    queue(Event::Payload {
        player,
        channel: channel.to_owned(),
        data: data.to_vec(),
    });
}

/// Receives one serverbound Axiom channel (`play_raw` with this handler, on the main thread).
struct Inbox(String);

impl IPayloadHandler for Inbox {
    fn handle(
        &self,
        payload: PayloadBytes,
        context: &mut dyn IPayloadContext,
    ) -> Result<(), String> {
        let player = context.player().map_err(|e| format!("{e:?}"))?;
        on_custom_payload(player.uuid(), &self.0, &payload.0);
        Ok(())
    }
}

/// The module file path for the glue of `MarkerData` (kept for the marker encoding path, which is unused until
/// `level_markers` exists).
#[allow(dead_code)]
fn encode_marker(uuid: u128, position: [f64; 3], data: &Nbt) -> Vec<u8> {
    let mut w = Writer::new();
    MarkerData::create_from(uuid, position, data).write(&mut w);
    w.into_bytes()
}
