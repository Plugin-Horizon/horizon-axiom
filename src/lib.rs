//! Axiom server side (AxiomPaperPlugin) as an arismoko native trait mod. The protocol logic is the unchanged
//! `horizon-axiom-core` crate (`core/`). This file is the glue: it implements the core's host traits on the
//! `ServerHost` handle API, and routes the hooks the old mixin injections used. Engine APIs arismoko does not have
//! yet are marked `TODO(arismoko-api: <name>)` with what is lost meanwhile; PORT_NOTES.md lists them all.
//!
//! Fail-closed rule: where a write or a reply has no API, the write is refused and the reply is not sent, so the
//! Axiom client never gets a handshake and nothing in the world changes.

mod block_ids;
mod config_migrate;
pub mod horizon_axiom_core;
use crate::horizon_axiom_core::annotation::{
    self, AnnotationHost, AnnotationState, MarkerData, MarkerEntity,
};
use crate::horizon_axiom_core::blueprint::{self, BlueprintHost, BlueprintStore};
use crate::horizon_axiom_core::buf::{BufError, Nbt, Reader, Writer};
use crate::horizon_axiom_core::buffer::{zstd_compress, zstd_decompress};
use crate::horizon_axiom_core::constants::CLIENTBOUND_CHANNELS;
use crate::horizon_axiom_core::constants::{
    CHANNEL_ADD_SERVER_HEIGHTMAP, CHANNEL_TUNNEL, SERVERBOUND_CHANNELS, TUNNEL_BUFFER_FLAG_FIRST,
    TUNNEL_BUFFER_FLAG_LAST, TUNNEL_PACKET_FLAG_ZSTD_COMPRESSED,
};
use crate::horizon_axiom_core::custom::{
    self, CustomBlockRegistry, CustomDisplayRegistry, CustomHost, DebugHost,
};
use crate::horizon_axiom_core::entity::{self, EntityHost, EntityTypeFilter, HiddenEntities, Pos, Turn};
use crate::horizon_axiom_core::regex::full_match;
use crate::horizon_axiom_core::request::{
    self, EntityInfo, RequestChunksOperation, RequestHost, SectionRead,
};
use crate::horizon_axiom_core::restrictions::{
    ALL_PERMISSIONS, AxiomHandshakeEvent, AxiomModifyWorldEvent, Config as AxiomConfig, MessageColor, PlotBounds,
    RestrictionsHost, RestrictionsState, original_path,
};
use crate::horizon_axiom_core::restrictions::disallowed::{is_disallowed, parse_all, Disallowed};
use crate::horizon_axiom_core::setblock::block_packet::{BlockHit, Hand, handle_set_block};
use crate::horizon_axiom_core::setblock::buffer_packet::handle_buffer_packet;
use crate::horizon_axiom_core::setblock::dispatch::DispatchBudget;
use crate::horizon_axiom_core::setblock::tick_blocks::handle_tick_blocks;
use crate::horizon_axiom_core::setblock::{
    BlockPos, ChunkLoadState, OperationQueue, SectionPermissionChecker, SetBlockConfig,
    SetBlockHost,
};
use crate::horizon_axiom_core::world::packets::{set_fly_speed, set_gamemode, set_world_time, teleport};
use crate::horizon_axiom_core::world::{
    GameMode as CoreGameMode, Location, MarkerEntry, WorldHost, WorldState,
};
use crate::block_ids::{BlockTable, RegistryBlock};
use mod_api::{
    BlockMetadata, BlockState as HostState, BlockUpdateFlags, HostBlockStates, StateConstraint,
    ChunkRef, Command, CommandArgumentValue, Compound, Config, ConfigFile, ConfigKind,
    ConfigLoadingEvent, ConfigReloadingEvent, EntityRef, Flow, GameMode, GameRuleValue, Hooks,
    HostSide, LevelRef, NodeDefault, PermissionGatherNodesEvent, PermissionLevel, PermissionNode,
    PermissionValueType, PlayerLoggedInEvent, PlayerLoggedOutEvent, PlayerRef,
    RegisterCommandsEvent, RegisterConfigsEvent, RunModCommandEvent, ServerAboutToStartEvent,
    ServerHost, ServerTickEvent, ServerTickPhase, Tag, Text, TicketController, TicketOwner,
};
use mod_api::{
    IPayloadContext, IPayloadHandler, PayloadBytes, PayloadHandleError, PayloadId, Registrar,
    RegistrationContext,
};
use mod_api::text::{TextClick, TextHover, TextStyle};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

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
const CHUNK_TICKET_OWNER: TicketOwner = TicketOwner::Block([0, 0, 0]);

/// The ticket controller `axiom:chunks`, made in `register`; unset when the game refused it.
static CHUNK_CONTROLLER: OnceLock<TicketController> = OnceLock::new();
/// Set once the tickets an earlier run left in the saves have been released.
static OLD_TICKETS_RELEASED: AtomicBool = AtomicBool::new(false);

/// The state id (air) a block the vanilla table does not know, such as a mod's block, reads as.
const UNMAPPED_STATE: u32 = 0;

/// The engine's process-local block states as the numeric ids the wire format uses, kept per thread because the
/// conversion goes through the game's property tables.
#[derive(Default)]
struct StateCache {
    to_host: HashMap<u32, Option<HostState>>,
    from_host: HashMap<HostState, u32>,
    has_block_entity: HashMap<u32, bool>,
}

thread_local! {
    static STATE_CACHE: RefCell<StateCache> = RefCell::new(StateCache::default());
}

/// The engine's update flags for the Java `Block.UPDATE_*` bits the core passes (`IMMEDIATE`, 8, has no counterpart).
fn update_flags(bits: u32) -> BlockUpdateFlags {
    const MAP: [(u32, BlockUpdateFlags); 9] = [
        (1, BlockUpdateFlags::NEIGHBORS),
        (2, BlockUpdateFlags::CLIENTS),
        (4, BlockUpdateFlags::INVISIBLE),
        (16, BlockUpdateFlags::KNOWN_SHAPE),
        (32, BlockUpdateFlags::SUPPRESS_DROPS),
        (64, BlockUpdateFlags::MOVE_BY_PISTON),
        (128, BlockUpdateFlags::SKIP_SHAPE_UPDATE_ON_WIRE),
        (256, BlockUpdateFlags::SKIP_BLOCK_ENTITY_SIDE_EFFECTS),
        (512, BlockUpdateFlags::SKIP_ON_PLACE),
    ];
    MAP.iter()
        .filter(|(bit, _)| bits & bit != 0)
        .fold(BlockUpdateFlags::NONE, |acc, (_, flag)| acc | *flag)
}

/// The flags of a raw section write: the clients hear of it, nothing else runs (Java `setBlockState` on the section,
/// then `blockChanged`).
const RAW_WRITE_BITS: u32 = 2 | 16 | 32 | 256 | 512;

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
/// Nodes that are asked besides the core's own `axiom.*` ones: `/axiompaperdebug` and the region bypass.
const EXTRA_NODES: [&str; 2] = ["axiom.debug", "axiomadmin.bypass_region_checks"];

/// How often (in ticks) the permission answers of the online players are asked again.
const PERMISSION_REFRESH_TICKS: u32 = 20;

thread_local! {
    /// The nodes each online player holds, as of the last `refresh_permissions`. The core's host traits only get
    /// `&self`, which cannot ask the permission service, so the answers are taken at the tick start.
    static PERMS: RefCell<HashMap<u128, BTreeSet<String>>> = RefCell::new(HashMap::new());
}
static PERMISSION_TICKS: AtomicU32 = AtomicU32::new(0);
/// `(added, removed)` top-level keys of the config file against the shipped default, as read at startup.
static CONFIG_OUTDATED: Mutex<(usize, usize)> = Mutex::new((0, 0));

/// `ServerHeightmaps`: the image files directly in `plugins/AxiomPaper/heightmaps` as `(file name, bytes)`. Java reads
/// them once in `onEnable`; this port reads them at the first join.
static SERVER_HEIGHTMAPS: OnceLock<Vec<(String, Vec<u8>)>> = OnceLock::new();

/// Every node name this mod asks about: the core's non-wildcard `axiom.*` nodes (a typed node cannot carry a `*`;
/// grants of a wildcard reach the leaf through the permission handler), `axiom.debug`, the region bypass and one
/// `axiomlimits.<key>` per limit key of the config. Names a node cannot carry are left out.
fn declared_nodes(limit_keys: &[String]) -> Vec<String> {
    let mut nodes: Vec<String> = ALL_PERMISSIONS
        .iter()
        .map(|p| p.node().to_owned())
        .chain(EXTRA_NODES.iter().map(|n| (*n).to_owned()))
        .chain(limit_keys.iter().map(|k| format!("axiomlimits.{k}")))
        .filter(|n| node_name_ok(n))
        .collect();
    nodes.sort();
    nodes.dedup();
    nodes
}

/// A typed node name: non-empty dot-separated segments of ASCII letters, digits, `_` and `-`.
fn node_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

fn typed_node(name: &str) -> PermissionNode {
    PermissionNode::new(name, PermissionValueType::Boolean)
}

/// The limit keys of the config in force (`limits.<key>` with a rate limit, as `dispatch_send_overrides`).
fn limit_keys() -> Vec<String> {
    with_config(|c| c.dispatch_send_overrides())
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

/// Asks the permission service for every node of every online player and keeps the answers for the host traits.
/// Any refusal is a denial. Returns the players whose answers differ from the last time (or are new).
fn refresh_permissions(host: &mut dyn ServerHost) -> Vec<u128> {
    let keys = limit_keys();
    let nodes = declared_nodes(&keys);
    let online = host.players().server_players();
    let mut fresh: HashMap<u128, BTreeSet<String>> = HashMap::new();
    for player in online {
        let own: BTreeSet<String> = nodes
            .iter()
            .filter(|n| player.permission_bool(host, &typed_node(n)).unwrap_or(false))
            .cloned()
            .collect();
        let held = with_parents(&own);
        fresh.insert(player.uuid(), held);
    }
    PERMS.with(|cell| {
        let mut old = cell.borrow_mut();
        let changed = fresh
            .iter()
            .filter(|(uuid, held)| old.get(uuid) != Some(held))
            .map(|(uuid, _)| *uuid)
            .collect();
        *old = fresh;
        changed
    })
}

/// Bukkit's plugin.yml `children`: a node also holds when one of its wildcard parents holds, so `axiom.default`
/// gives `axiom.build.place` through `axiom.build.*`. A wildcard cannot be asked (`*` is not a typed node name), so
/// only the parents the player holds as typed nodes count, and the chain is walked up to the root.
fn with_parents(own: &BTreeSet<String>) -> BTreeSet<String> {
    let mut held = own.clone();
    for permission in ALL_PERMISSIONS.iter().copied() {
        if held.contains(permission.node()) {
            continue;
        }
        let mut parent = permission.parent();
        while let Some(ancestor) = parent {
            if own.contains(ancestor.node()) {
                held.insert(permission.node().to_owned());
                break;
            }
            parent = ancestor.parent();
        }
    }
    held
}

/// Refreshes the answers every `PERMISSION_REFRESH_TICKS` ticks, and at once when a player has none yet.
fn refresh_permissions_if_due(host: &mut dyn ServerHost) -> Vec<u128> {
    let tick = PERMISSION_TICKS.fetch_add(1, Ordering::Relaxed);
    let unknown = PERMS.with(|cell| {
        let known = cell.borrow();
        host.players()
            .server_players()
            .iter()
            .any(|p| !known.contains_key(&p.uuid()))
    });
    if unknown || tick % PERMISSION_REFRESH_TICKS == 0 {
        refresh_permissions(host)
    } else {
        Vec::new()
    }
}

/// Bukkit `player.hasPermission(node)` as far as the typed nodes go: the answer taken at the last refresh.
/// Operators hold the nodes whose default level they reach; there is no blanket operator level and no free node.
fn permission_fallback(_host: &dyn ServerHost, uuid: u128, node: &str) -> bool {
    PERMS.with(|cell| {
        cell.borrow()
            .get(&uuid)
            .is_some_and(|held| held.contains(node))
    })
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
    release_old_chunk_tickets(host);
    let changed = refresh_permissions_if_due(host);
    with_state(|slot| {
        if slot.as_ref().is_none_or(|s| s.generation != generation) {
            *slot = Some(AxiomState::new(with_config(|c| c.clone()), generation));
        }
        let Some(state) = slot.as_mut() else {
            return;
        };
        // Java `LuckPermsListener`: a changed answer drops the cached set and resends the restrictions.
        for player in changed {
            let mut h = AxiomHost::new(&mut *host, state.active.clone(), state.config.clone());
            state.restrictions.on_permissions_changed(&mut h, player);
        }
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

/// Releases, once, the tickets of `axiom:chunks` that an earlier run left in the saves: the engine keeps them across
/// restarts, and nothing here would ever let go of them.
fn release_old_chunk_tickets(host: &mut dyn ServerHost) {
    if OLD_TICKETS_RELEASED.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(controller) = CHUNK_CONTROLLER.get().copied() else {
        return;
    };
    for dim in VANILLA_DIMENSIONS {
        let Some(level) = host.level(dim) else {
            continue;
        };
        let Some(tickets) = level.mod_forced_chunks(&*host, controller) else {
            continue;
        };
        for (owner, [x, z], natural) in tickets {
            let _ = ChunkRef::new(level, [x, z]).set_mod_forced(host, controller, owner, false, natural);
        }
    }
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
        // AxiomPaper's WrapperPacketListener: a failing handler kicks with the channel and the error.
        log(&format!("WARN {channel} from {player:032x}: {e}"));
        host.kick_player(player, &format!("Error while processing packet {channel}: {e}"));
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
                host.kick_player(player, &format!("Axiom: Zstd failed to decompress: {e}"));
                return;
            }
        }
    } else {
        raw
    };
    if let Err(e) = route(host, state, player, &id, &body) {
        log(&format!("WARN {id} from {player:032x}: {e}"));
        host.kick_player(player, &format!("Error while processing packet {id}: {e}"));
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

/// What Java's `AxiomPaper.sendInitialPackets` sends once a player is active.
fn send_initial(host: &mut AxiomHost<'_>, state: &mut AxiomState, player: u128) {
    let dim = host
        .dimension_of(player)
        .unwrap_or_else(|| VANILLA_DIMENSIONS[0].to_owned());
    state.world.send_world_properties(host, player, &dim);
    state.world.on_player_join(host, player, &dim);
    if let Err(e) = blueprint::send_manifest(host, &state.blueprints, &[player]) {
        log(&format!("WARN blueprint manifest for {player:032x}: {e:?}"));
    }
    send_heightmaps(host, player);
    if let Err(e) = annotation::send_all(host, &mut state.annotations, &dim, player) {
        log(&format!("WARN annotations for {player:032x}: {e:?}"));
    }
    state.custom_blocks.on_player_join(host, player);
    state.custom_displays.on_player_join(host, player);
    if is_op_level(&*host.host, player) {
        send_outdated_notice(host, player);
    }
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
    /// `disallowed-blocks` as parsed entries: a client's state matching one reads as void air (Java `DisallowedBlocks`).
    disallowed: Vec<Disallowed>,
    /// Entities of the dimensions indexed for this call: client-visible uuid to (dimension, engine handle).
    entities: HashMap<u128, (String, EntityRef)>,
    /// The restrictions text of the player who runs `/axiompaperdebug`.
    debug_restrictions: Option<(u128, String)>,
    /// Spawn trees the core built detached, keyed by the root's uuid, until they are added or dropped.
    pending: HashMap<u128, PendingTree>,
}

impl<'a> AxiomHost<'a> {
    fn new(host: &'a mut dyn ServerHost, active: BTreeSet<u128>, config: AxiomConfig) -> Self {
        // The vanilla state numbering comes from the engine's registry: block keys in registry order (vanilla's
        // first), each with its properties and legal values.
        BlockTable::ensure(|| {
            let meta = host.block_states();
            host.block_keys()
                .into_iter()
                .filter_map(|key| {
                    let id = meta.name(key).ok()?.to_owned();
                    let properties = meta
                        .definition(key)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| (p.name, p.values))
                        .collect();
                    Some(RegistryBlock { id, properties })
                })
                .collect()
        });
        let filter = EntityTypeFilter::new(
            config.whitelist_entities.clone(),
            config.blacklist_entities.clone(),
        );
        let disallowed = parse_all(&config.disallowed_blocks);
        Self {
            host,
            active,
            config,
            filter,
            disallowed,
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
        if let Err(e) = p.disconnect(&mut *self.host, Text::literal(reason)) {
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
        self.tell_text(player, Text::literal(text));
    }

    fn tell_text(&mut self, player: u128, text: Text) {
        if let Some(p) = self.player(player) {
            let _ = p.tell(&mut *self.host, text);
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

    /// Holds chunk `(cx, cz)` of `dim` for Axiom: the first hold forces it loaded through the `axiom:chunks` ticket
    /// controller, later holds only count.
    fn hold_chunk(&mut self, dim: &str, cx: i32, cz: i32) {
        let first = CHUNK_HOLDS.with(|holds| {
            let mut holds = holds.borrow_mut();
            let count = holds.entry((dim.to_owned(), cx, cz)).or_insert(0);
            *count += 1;
            *count == 1
        });
        if first {
            self.force_chunk(dim, cx, cz, true);
        }
    }

    /// Lets go of one hold on chunk `(cx, cz)` of `dim`; the last one releases the ticket.
    fn release_chunk_hold(&mut self, dim: &str, cx: i32, cz: i32) {
        let last = CHUNK_HOLDS.with(|holds| {
            let mut holds = holds.borrow_mut();
            let key = (dim.to_owned(), cx, cz);
            match holds.get_mut(&key) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => {
                    holds.remove(&key);
                    true
                }
                None => false,
            }
        });
        if last {
            self.force_chunk(dim, cx, cz, false);
        }
    }

    /// Takes or drops the ticket of one chunk. A game that refused the controller leaves the chunks unforced.
    fn force_chunk(&mut self, dim: &str, cx: i32, cz: i32, forced: bool) {
        let (Some(controller), Some(level)) = (CHUNK_CONTROLLER.get().copied(), self.level(dim)) else {
            return;
        };
        let _ = ChunkRef::new(level, [cx, cz]).set_mod_forced(
            &mut *self.host,
            controller,
            CHUNK_TICKET_OWNER,
            forced,
            false,
        );
    }

    /// The engine state the numeric state id `id` names, `None` for an id the game has no block for.
    fn state_to_host(&self, id: StateId) -> Option<HostState> {
        if let Some(hit) = STATE_CACHE.with(|c| c.borrow().to_host.get(&id).copied()) {
            return hit;
        }
        let resolved = BlockTable::get()
            .state_string(id)
            .and_then(|text| self.host.block_state_id(&text));
        STATE_CACHE.with(|c| c.borrow_mut().to_host.insert(id, resolved));
        resolved
    }

    /// The numeric state id of an engine state, `UNMAPPED_STATE` for a block outside the vanilla table.
    fn state_from_host(&self, state: HostState) -> StateId {
        if let Some(hit) = STATE_CACHE.with(|c| c.borrow().from_host.get(&state).copied()) {
            return hit;
        }
        let meta = self.host.block_states();
        let id = meta
            .name(state.block())
            .ok()
            .and_then(|name| {
                let values: Vec<(String, String)> = meta
                    .definition(state.block())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|p| {
                        let value = state.property(&meta, &p.name).ok().flatten()?.to_owned();
                        Some((p.name, value))
                    })
                    .collect();
                let refs: Vec<(&str, &str)> =
                    values.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                BlockTable::get().id_of(name, &refs)
            })
            .unwrap_or(UNMAPPED_STATE);
        STATE_CACHE.with(|c| c.borrow_mut().from_host.insert(state, id));
        id
    }

    /// The block state at `pos`, from a loaded chunk.
    fn state_at(&self, dim: &str, pos: [i32; 3]) -> Option<StateId> {
        if !self.chunk_loaded(dim, pos[0] >> 4, pos[2] >> 4) {
            return None;
        }
        let state = self.level(dim)?.block_state(&*self.host, pos)?;
        Some(self.state_from_host(state))
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

    /// The block entity at `pos` as `saveWithId` writes it: its data with `id` and the position.
    fn saved_block_entity(&mut self, dim: &str, pos: [i32; 3]) -> Option<Nbt> {
        let level = self.level(dim)?;
        let entity = self.host.block_entity(level, pos)?;
        let data = entity.data(&*self.host)?;
        let Nbt::Compound(mut map) = tag_to_core(&Tag::Compound(data)) else { return None };
        if let Some(kind) = entity.kind(&*self.host) {
            map.insert("id".to_owned(), Nbt::String(kind.id(&*self.host).to_string()));
        }
        map.insert("x".to_owned(), Nbt::Int(pos[0]));
        map.insert("y".to_owned(), Nbt::Int(pos[1]));
        map.insert("z".to_owned(), Nbt::Int(pos[2]));
        Some(Nbt::Compound(map))
    }

    /// Reads a section of the chunk cell by cell (no section read API; `read_columns` is column shaped).
    fn section_read(&self, dimension: &str, cx: i32, sy: i32, cz: i32) -> SectionRead {
        if !self.chunk_loaded(dimension, cx, cz) {
            return SectionRead::Missing;
        }
        let (min_section, max_section) = SetBlockHost::section_y_range(self, dimension);
        if sy < min_section || sy > max_section {
            return SectionRead::Missing;
        }
        let Some(level) = self.level(dimension) else {
            return SectionRead::Missing;
        };
        let table = BlockTable::get();
        let mut states = Box::new([UNMAPPED_STATE; 4096]);
        let mut only_air = true;
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let pos = [cx * 16 + x, sy * 16 + y, cz * 16 + z];
                    let id = level
                        .block_state(&*self.host, pos)
                        .map_or(UNMAPPED_STATE, |s| self.state_from_host(s));
                    only_air &= table.is_air(id);
                    states[((y << 8) | (z << 4) | x) as usize] = id;
                }
            }
        }
        if only_air {
            SectionRead::Empty
        } else {
            SectionRead::Blocks(states)
        }
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
        // HMOD-GAP(permission-explicit-deny): a typed Boolean query answers only yes or no, so an explicit deny
        // cannot be told from "not set". A held node is Some(true); every other node is undecided (None).
        permission_fallback(&*self.host, player, node).then_some(true)
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
    fn send_message(&mut self, player: u128, text: &str, color: MessageColor) {
        let colour = match color {
            MessageColor::Red => "red",
            MessageColor::Yellow => "yellow",
        };
        self.tell_text(player, coloured(text, colour));
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
    fn marker_snapshot(&mut self, dim: &str) -> Vec<MarkerEntry> {
        self.level_markers(dim)
            .into_iter()
            .map(|(uuid, position, data)| MarkerEntry {
                uuid,
                encoded: encode_marker(uuid, position, &data),
            })
            .collect()
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
    fn block_state_from_wire(&self, _player: u128, wire_id: i32) -> Option<u32> {
        // The client's ids are the vanilla 26.3 numbering, which `BlockTable` rebuilds. An id the game has no block
        // for (a mod's) is refused. A state under `disallowed-blocks` reads as void air, as Java's client registry does.
        let id = u32::try_from(wire_id).ok()?;
        self.state_to_host(id)?;
        if !self.disallowed.is_empty() {
            let text = BlockTable::get().state_string(id)?;
            if is_disallowed(&self.disallowed, &text) {
                return Some(self.empty_state());
            }
        }
        Some(id)
    }
    fn is_air(&self, state: u32) -> bool {
        BlockTable::get().is_air(state)
    }
    fn empty_state(&self) -> u32 {
        BlockTable::get().void_air()
    }
    fn global_bits(&self) -> u8 {
        BlockTable::get().global_bits()
    }
    fn same_block(&self, a: u32, b: u32) -> bool {
        let table = BlockTable::get();
        a == b || (table.block_index(a).is_some() && table.block_index(a) == table.block_index(b))
    }
    fn state_can_be_replaced(&self, state: u32) -> bool {
        match self.state_to_host(state) {
            Some(hs) => self.host.block_states().replaceable(hs).unwrap_or(false),
            None => self.is_air(state),
        }
    }
    fn has_block_entity(&self, state: u32) -> bool {
        if let Some(hit) = STATE_CACHE.with(|c| c.borrow().has_block_entity.get(&state).copied()) {
            return hit;
        }
        let has = self
            .state_to_host(state)
            .and_then(|hs| self.host.block_states().has_block_entity(hs))
            .unwrap_or(false);
        STATE_CACHE.with(|c| c.borrow_mut().has_block_entity.insert(state, has));
        has
    }
    fn fluid_is_empty(&self, state: u32) -> bool {
        BlockTable::get().fluid_is_empty(state)
    }
    fn is_liquid_block(&self, state: u32) -> bool {
        BlockTable::get().is_liquid_block(state)
    }
    fn is_bed_double_plant_or_door(&self, state: u32) -> bool {
        BlockTable::get().is_bed_double_plant_or_door(state)
    }
    fn light_properties_differ(&self, old: u32, new: u32) -> bool {
        old != new
    }
    fn block_entity_must_be_removed(&self, _old: u32, _new: u32) -> bool {
        // The engine's write drops the old block entity and makes the new block's own (no remove call exists).
        false
    }
    fn is_game_master_block(&self, dim: &str, pos: BlockPos) -> bool {
        self.state_at(dim, [pos.0, pos.1, pos.2])
            .is_some_and(|state| BlockTable::get().is_game_master_block(state))
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
    fn set_block_raw(&mut self, dim: &str, pos: BlockPos, state: u32) -> u32 {
        let old = self.block_state(dim, pos);
        if old == state {
            return old;
        }
        if let (Some(level), Some(hs)) = (self.level(dim), self.state_to_host(state)) {
            let _ = level.set_block(
                &mut *self.host,
                [pos.0, pos.1, pos.2],
                hs,
                update_flags(RAW_WRITE_BITS),
            );
        }
        old
    }
    // The engine's write keeps the heightmaps.
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
    fn load_block_entities(&mut self, dim: &str, entries: &[(BlockPos, Vec<u8>)]) {
        let Some(level) = self.level(dim) else { return };
        for (pos, bytes) in entries {
            let Ok(Nbt::Compound(new)) = Reader::new(bytes).read_nbt() else { continue };
            let at = [pos.0, pos.1, pos.2];
            let Some(entity) = self.host.block_entity(level, at) else { continue };
            // `loadWithComponents` keeps what the data does not name.
            let mut data: Compound = entity.data(&*self.host).unwrap_or_default();
            if let Tag::Compound(overlay) = core_to_tag(&Nbt::Compound(new)) {
                for (k, v) in overlay.iter() {
                    data.insert(k.clone(), v.clone());
                }
            }
            let _ = entity.replace_data(&mut *self.host, data);
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
    fn set_block(&mut self, dim: &str, pos: BlockPos, state: u32, flags: u32) {
        if let (Some(level), Some(hs)) = (self.level(dim), self.state_to_host(state)) {
            let _ = level.set_block(&mut *self.host, [pos.0, pos.1, pos.2], hs, update_flags(flags));
        }
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
        self.entity_of(uuid)?
            .kind(&*self.host)
            .map(|kind| kind.id(&*self.host))
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

/// The vanilla keys `Entity.saveWithoutId` writes beside a marker's custom data. Custom data is merged into the same
/// root, so `/data get entity` shows both; these are cut to leave `MarkerData.getData`'s compound.
const MARKER_VANILLA_KEYS: [&str; 12] = [
    "Pos", "Motion", "Rotation", "FallDistance", "Fire", "Air", "OnGround", "Invulnerable", "PortalCooldown", "UUID",
    "Tags", "Passengers",
];

impl AxiomHost<'_> {
    /// The markers of `dim` (`minecraft:marker`) as `(uuid, position, custom data)`, in the level's entity order.
    /// Markers hidden by Axiom's gizmo flag are listed too: that flag is in the core state, not in this host.
    fn level_markers(&mut self, dim: &str) -> Vec<(u128, [f64; 3], Nbt)> {
        let Some(level) = self.level(dim) else {
            return Vec::new();
        };
        let entities = level.all_entities(&*self.host).unwrap_or_default();
        let mut out = Vec::new();
        for entity in entities {
            let kind = entity.kind(&*self.host).map(|k| k.id(&*self.host));
            if kind.as_deref() != Some(entity::TYPE_MARKER) {
                continue;
            }
            let Some(uuid) = entity.uuid(&mut *self.host) else {
                continue;
            };
            let Some(pos) = entity.position(&*self.host) else {
                continue;
            };
            let Some(data) = entity.nbt(&mut *self.host) else {
                continue;
            };
            let Nbt::Compound(mut map) = tag_to_core(&Tag::Compound(data)) else {
                continue;
            };
            for key in MARKER_VANILLA_KEYS {
                map.remove(key);
            }
            out.push((uuid, pos, Nbt::Compound(map)));
        }
        out
    }

    /// The NBT of a loaded entity, with its `id`, as a save would give it.
    fn saved_entity(&mut self, dimension: &str, uuid: u128) -> Option<Nbt> {
        let (dim, entity) = self.entities.get(&uuid)?.clone();
        if dim.as_str() != dimension {
            return None;
        }
        let kind = entity.kind(&*self.host)?;
        let mut data = entity.nbt(&mut *self.host)?;
        data.insert("id".into(), Tag::String(kind.id(&*self.host).as_str().into()));
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
    fn block_entity_nbt(&mut self, dimension: &str, x: i32, y: i32, z: i32) -> Option<Nbt> {
        self.saved_block_entity(dimension, [x, y, z])
    }
    fn section_block_entities(
        &mut self,
        dimension: &str,
        chunk_x: i32,
        section_y: i32,
        chunk_z: i32,
    ) -> Vec<(i32, i32, i32, Nbt)> {
        let Some(level) = self.level(dimension) else { return Vec::new() };
        let mut found = Vec::new();
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let pos = [chunk_x * 16 + x, section_y * 16 + y, chunk_z * 16 + z];
                    let Some(state) = level.block_state(&*self.host, pos) else { continue };
                    if !SetBlockHost::has_block_entity(self, self.state_from_host(state)) {
                        continue;
                    }
                    if let Some(nbt) = self.saved_block_entity(dimension, pos) {
                        found.push((pos[0], pos[1], pos[2], nbt));
                    }
                }
            }
        }
        found
    }
    fn entity_info(&self, dimension: &str, uuid: u128) -> Option<EntityInfo> {
        let (dim, entity) = self.entities.get(&uuid)?;
        if dim.as_str() != dimension {
            return None;
        }
        let entity_type = entity.kind(&*self.host)?.id(&*self.host);
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
        self.host.server_data_version()
    }
    fn default_state(&self, block: &str) -> u32 {
        self.host
            .block_state_id(block)
            .map_or(UNMAPPED_STATE, |s| self.state_from_host(s))
    }
    fn global_state_bits(&self) -> u8 {
        self.global_bits()
    }
    fn state_to_nbt(&self, state: u32) -> Nbt {
        // `BlockState.CODEC`: `Name` and, when the block has properties, `Properties` (all values as strings).
        let Some((block, values)) = BlockTable::get().describe(state) else { return Nbt::End };
        let mut map = BTreeMap::new();
        map.insert("Name".to_owned(), Nbt::String(format!("minecraft:{}", block.name)));
        if !values.is_empty() {
            let props = values
                .iter()
                .map(|(k, v)| ((*k).to_owned(), Nbt::String((*v).to_owned())))
                .collect();
            map.insert("Properties".to_owned(), Nbt::Compound(props));
        }
        Nbt::Compound(map)
    }
    fn state_from_nbt(&self, nbt: &Nbt) -> Option<u32> {
        let Nbt::Compound(map) = nbt else { return None };
        let Some(Nbt::String(name)) = map.get("Name") else { return None };
        let mut text = name.clone();
        if let Some(Nbt::Compound(props)) = map.get("Properties") {
            let parts: Vec<String> = props
                .iter()
                .filter_map(|(k, v)| match v {
                    Nbt::String(v) => Some(format!("{k}={v}")),
                    _ => None,
                })
                .collect();
            if !parts.is_empty() {
                text = format!("{name}[{}]", parts.join(","));
            }
        }
        // The game fills the properties the data leaves out with the block's defaults and refuses unknown ones.
        let state = self.host.block_state_id(&text)?;
        Some(self.state_from_host(state))
    }
    fn block_entity_type_of(&self, _state: u32) -> Option<String> {
        // No API names the block entity type of a state (see GAPS_UPSTREAM.md).
        None
    }
    fn block_entity_accepts(&self, _type_id: &str, state: u32) -> bool {
        SetBlockHost::has_block_entity(self, state)
    }
    fn upgrade_block_state(&self, nbt: Nbt, from_version: i32) -> Result<Nbt, String> {
        if from_version == self.data_version() {
            Ok(nbt)
        } else {
            Err("no block state data fixer available for an older data version".to_owned())
        }
    }
    fn upgrade_block_entity(&self, nbt: Nbt, from_version: i32) -> Result<Nbt, String> {
        let Tag::Compound(data) = core_to_tag(&nbt) else { return Err("not a compound".to_owned()) };
        self.host
            .upgrade_block_entity_nbt(from_version, data)
            .map(|c| tag_to_core(&Tag::Compound(c)))
            .map_err(|e| format!("{e:?}"))
    }
    fn upgrade_entity(&self, nbt: Nbt, from_version: i32) -> Result<Nbt, String> {
        let Tag::Compound(data) = core_to_tag(&nbt) else { return Err("not a compound".to_owned()) };
        self.host
            .upgrade_entity_nbt(from_version, data)
            .map(|c| tag_to_core(&Tag::Compound(c)))
            .map_err(|e| format!("{e:?}"))
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
    fn markers(&mut self, dim: &str) -> Vec<MarkerEntity> {
        self.level_markers(dim)
            .into_iter()
            .map(|(uuid, position, data)| MarkerEntity {
                uuid,
                position,
                data,
            })
            .collect()
    }
    fn marker_custom_data(&mut self, dim: &str, uuid: u128) -> Option<Nbt> {
        self.level_markers(dim)
            .into_iter()
            .find(|(found, _, _)| *found == uuid)
            .map(|(_, _, data)| data)
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
    fn block_state_string(&self, state: u32) -> String {
        BlockTable::get()
            .state_string(state)
            .unwrap_or_else(|| "minecraft:air".to_owned())
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
        _provider: Option<crate::horizon_axiom_core::custom::Provider>,
    ) -> bool {
        self.may_build(player)
    }
    fn can_place_block(
        &self,
        player: u128,
        _provider: Option<crate::horizon_axiom_core::custom::Provider>,
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

/// `AxiomPaper.migrateConfig`: backs `config.yml` up to `config.yml.bak`, then rewrites it as the shipped default
/// with the values it sets. The answer lines, in order.
fn migrate_config() -> Vec<Text> {
    let (added, removed) = *CONFIG_OUTDATED.lock().unwrap_or_else(|e| e.into_inner());
    if added == 0 && removed == 0 {
        return vec![coloured("No migration necessary", "yellow")];
    }
    let path = original_path(Path::new("."));
    let Ok(current) = std::fs::read_to_string(&path) else {
        return vec![coloured("Internal error: config.yml doesn't exist", "red")];
    };
    let backup = path.with_file_name("config.yml.bak");
    if std::fs::copy(&path, &backup).is_err() {
        return vec![coloured("Internal error: couldn't backup existing config", "red")];
    }
    *CONFIG_OUTDATED.lock().unwrap_or_else(|e| e.into_inner()) = (0, 0);
    let mut lines = vec![coloured(
        "Successfully backed up current config to config.yml.bak",
        "green",
    )];
    let migrated = config_migrate::migrate(config_migrate::DEFAULT_CONFIG, &current);
    if std::fs::write(&path, migrated).is_err() {
        lines.push(coloured("Internal error: failed to save migrated config.yml", "red"));
        return lines;
    }
    lines.push(coloured("Successfully migrated config.yml", "green"));
    lines
}

/// The two lines an operator is sent once Axiom is active when the config file lags the shipped default: the summary
/// in yellow, then a green bold "CLICK HERE" that runs `/axiompapermigrateconfig` (hover shows the command), as in
/// `AxiomPaper`'s join handler.
fn send_outdated_notice(host: &mut AxiomHost<'_>, player: u128) {
    let (added, removed) = *CONFIG_OUTDATED.lock().unwrap_or_else(|e| e.into_inner());
    if added == 0 && removed == 0 {
        return;
    }
    let mut parts = Vec::new();
    if added != 0 {
        parts.push(format!("{added} new entries"));
    }
    if removed != 0 {
        parts.push(format!("{removed} removed entries"));
    }
    let summary = format!("Axiom: Plugin config is outdated ({})", parts.join(", "));
    host.tell_text(player, coloured(&summary, "yellow"));

    let mut click = TextStyle::default();
    click.color = Some("green".to_owned());
    click.bold = Some(true);
    click.click = Some(TextClick::RunCommand("/axiompapermigrateconfig".to_owned()));
    click.hover = Some(TextHover::Text(Box::new(Text::literal(
        "/axiompapermigrateconfig",
    ))));
    let hint = Text::literal("CLICK HERE")
        .with_style(click)
        .append(coloured(" to migrate the config", "yellow"));
    host.tell_text(player, hint);
}

/// A chat line in a named colour (`red`, `yellow`, `green`), as `Component.text(..).color(..)`.
fn coloured(text: &str, colour: &str) -> Text {
    let mut style = TextStyle::default();
    style.color = Some(colour.to_owned());
    Text::literal(text).with_style(style)
}

/// The image files of `dir` (top level only) as `(name, bytes)`, in name order. A file that is not an image, or
/// is over 2,000,000 bytes, is left out, as `ServerHeightmaps.load` does.
fn load_server_heightmaps(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();
    let mut maps = Vec::new();
    for path in paths {
        // A directory does not read as a file, so it is skipped as `ImageIO.read` skips it.
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if !looks_like_image(&bytes) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        if bytes.len() > 2_000_000 {
            log(&format!("Heightmap {name} skipped because it's over 2mb"));
            continue;
        }
        maps.push((name, bytes));
    }
    maps
}

/// The formats `ImageIO.read` decodes by default (PNG, JPEG, GIF, BMP), by their leading bytes. The file is not
/// decoded, so a damaged image with a valid header is sent where Java would skip it.
fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || bytes.starts_with(b"BM")
}

/// `ServerHeightmaps.sendTo`: one `axiom:add_server_heightmap` payload per file, the name then the bytes.
fn send_heightmaps(host: &mut AxiomHost<'_>, player: u128) {
    let maps = SERVER_HEIGHTMAPS.get_or_init(|| {
        load_server_heightmaps(&Path::new(".").join("plugins").join("AxiomPaper").join("heightmaps"))
    });
    for (name, bytes) in maps {
        let mut out = Writer::new();
        out.write_string(name);
        out.write_byte_array(bytes);
        host.send_raw(player, CHANNEL_ADD_SERVER_HEIGHTMAP, &out.into_bytes());
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

/// The subcommand line of `/axiompaperdebug`: the literals after the command name, then the text argument if the usage
/// has one (`isClientListening <channel>`, `hasPermission <permission>`). Brigadier's `greedyString` takes the rest.
fn debug_input(event: &RunModCommandEvent<'_>) -> String {
    let mut parts: Vec<String> = event.command.literals.iter().skip(1).cloned().collect();
    for argument in &event.command.arguments {
        if let CommandArgumentValue::String(text) = &argument.value {
            parts.push(text.clone());
        }
    }
    parts.join(" ")
}

/// One usage per `AxiomDebugCommand` literal, so the tab list shows the subcommands. The handler checks the sender.
const DEBUG_USAGES: &[&str] = &[
    "axiompaperdebug canUseAxiom",
    "axiompaperdebug isMismatchedDataVersion",
    "axiompaperdebug canModifyWorld",
    "axiompaperdebug isClientListening <channel:text>",
    "axiompaperdebug hasPermission <permission:text>",
    "axiompaperdebug getRestrictions",
    "axiompaperdebug canBreakBlockAtCurrentPosition",
    "axiompaperdebug canBreakBlockAtCurrentPosition plotsquared",
    "axiompaperdebug canBreakBlockAtCurrentPosition worldguard",
    "axiompaperdebug canPlaceBlockAtCurrentPosition",
    "axiompaperdebug canPlaceBlockAtCurrentPosition plotsquared",
    "axiompaperdebug canPlaceBlockAtCurrentPosition worldguard",
    "axiompaperdebug isPlotWorld",
    "axiompaperdebug getCurrentEditablePlot",
];

/// Config text from the game (the registered TOML fallback, or nothing for the Paper file).
fn config_file(file: &ConfigFile) -> bool {
    file.mod_id == ID && matches!(file.kind, ConfigKind::Server)
}

/// The limit keys the config will carry once the server has started: the Paper file when there is one, else the
/// settings read so far. The nodes are declared before `server_about_to_start` has read the file.
fn original_limit_keys() -> Vec<String> {
    let original = original_path(Path::new("."));
    if original.is_file() {
        if let Ok((config, _)) = AxiomConfig::read_original(&original) {
            return config
                .dispatch_send_overrides()
                .into_iter()
                .map(|(key, _)| key)
                .collect();
        }
    }
    limit_keys()
}

#[derive(Default)]
struct Mod;

#[mod_api::horizon_mod]
impl Hooks for Mod {
    /// Declares the Axiom channels as raw play channels (arismoko #3209): serverbound ones with the inbox,
    /// clientbound ones send-only.
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        let _ = CHUNK_CONTROLLER.set(r.register_ticket_controller("chunks", false));
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
            if let Ok(text) = std::fs::read_to_string(&original) {
                *CONFIG_OUTDATED.lock().unwrap_or_else(|e| e.into_inner()) =
                    config_migrate::outdated(config_migrate::DEFAULT_CONFIG, &text);
            }
        }
    }

    fn permission_gather_nodes(&self, event: &mut PermissionGatherNodesEvent<'_>) {
        // Every node is op-default, as in AxiomPaper's plugin.yml (an undeclared default is op in Bukkit). Two
        // differences: Bukkit's `axiomadmin.bypass_region_checks` is for nobody and `axiomlimits.*` is unset for
        // everyone, which a level cannot say, so both take the highest level.
        let op = PermissionLevel::from_level(1);
        let keys = original_limit_keys();
        for name in declared_nodes(&keys) {
            let level = if name == "axiomadmin.bypass_region_checks" || name.starts_with("axiomlimits.") {
                PermissionLevel::from_level(4)
            } else {
                op
            };
            let _ = event.nodes.add(typed_node(&name), NodeDefault::Level(level));
        }
    }

    fn register_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        // Registered for every player: the handler checks op / axiom.debug / the debug UUID itself (Java: the
        // axiom.debug permission, not op only, and Brigadier hides the usages from everyone else).
        for usage in DEBUG_USAGES {
            event.commands.push(Command::new(usage));
        }
        event
            .commands
            .push(Command::op("axiompapermigrateconfig", 1));
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
                for line in migrate_config() {
                    let _ = host.commands().feedback(line);
                }
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
                let input = debug_input(event);
                let text = debug(&mut *host, uuid, input.trim());
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
    ) -> Result<(), PayloadHandleError> {
        let player = context.player().map_err(|e| PayloadHandleError::Failed(format!("{e:?}")))?;
        on_custom_payload(player.uuid(), &self.0, &payload.0);
        Ok(())
    }
}

/// The encoded `MarkerData` of one marker, as `marker_snapshot` and the marker packets send it.
fn encode_marker(uuid: u128, position: [f64; 3], data: &Nbt) -> Vec<u8> {
    let mut w = Writer::new();
    MarkerData::create_from(uuid, position, data).write(&mut w);
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heightmap_files_are_recognised_by_their_leading_bytes() {
        assert!(looks_like_image(b"\x89PNG\r\n\x1a\nrest"));
        assert!(looks_like_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(looks_like_image(b"GIF89a...."));
        assert!(looks_like_image(b"BM...."));
        assert!(!looks_like_image(b"\x89PNG"));
        assert!(!looks_like_image(b"name: not an image"));
        assert!(!looks_like_image(b""));
    }

    #[test]
    fn a_missing_heightmap_folder_gives_no_files() {
        assert!(load_server_heightmaps(Path::new("/nonexistent/axiom/heightmaps")).is_empty());
    }

    #[test]
    fn declared_nodes_have_no_wildcards_and_no_free_nodes() {
        let nodes = declared_nodes(&["vip".to_owned(), "bad key".to_owned()]);
        assert!(nodes.contains(&"axiom.use".to_owned()));
        assert!(nodes.contains(&"axiom.default".to_owned()));
        assert!(nodes.contains(&"axiom.all".to_owned()));
        assert!(nodes.contains(&"axiom.debug".to_owned()));
        assert!(nodes.contains(&"axiomadmin.bypass_region_checks".to_owned()));
        assert!(nodes.contains(&"axiomlimits.vip".to_owned()));
        assert!(!nodes.contains(&"axiom.entity.*".to_owned()));
        assert!(!nodes.contains(&"axiomlimits.bad key".to_owned()));
        assert!(nodes.iter().all(|n| node_name_ok(n)));
    }

    #[test]
    fn a_player_without_an_answer_holds_no_node() {
        PERMS.with(|cell| cell.borrow_mut().clear());
        PERMS.with(|cell| {
            cell.borrow_mut()
                .insert(7, BTreeSet::from(["axiom.use".to_owned()]))
        });
        let held = |node: &str| {
            PERMS.with(|cell| {
                cell.borrow()
                    .get(&7)
                    .is_some_and(|set| set.contains(node))
            })
        };
        assert!(held("axiom.use"));
        assert!(!held("axiom.default"));
        assert!(!held("axiom.entity.spawn"));
    }

    #[test]
    fn a_default_grant_reaches_its_plugin_yml_children() {
        let own = BTreeSet::from(["axiom.default".to_owned()]);
        let held = with_parents(&own);
        assert!(held.contains("axiom.use"));
        assert!(held.contains("axiom.build.place"));
        assert!(held.contains("axiom.tool.stamp"));
        assert!(held.contains("axiom.capability.no_clip"));
        assert!(!held.contains("axiom.entity.spawn"));
        assert!(!held.contains("axiom.debug"));
    }

    #[test]
    fn a_child_grant_alone_does_not_grant_its_parent() {
        let own = BTreeSet::from(["axiom.build.place".to_owned()]);
        let held = with_parents(&own);
        assert!(held.contains("axiom.build.place"));
        assert!(!held.contains("axiom.default"));
        assert!(!held.contains("axiom.use"));
        assert!(!held.contains("axiom.build.section"));
    }

    #[test]
    fn disallowed_wire_state_reads_as_void_air_in_the_core_list() {
        let list = parse_all(&["minecraft:oak_stairs[waterlogged=true]".to_owned()]);
        assert!(is_disallowed(&list, "minecraft:oak_stairs[facing=east,half=top,shape=straight,waterlogged=true]"));
        assert!(!is_disallowed(&list, "minecraft:stone"));
    }
}

#[cfg(test)]
mod block_state_tests {
    use super::*;

    #[test]
    fn update_flags_map_the_java_bits() {
        assert_eq!(update_flags(0).bits(), 0);
        assert_eq!(update_flags(3).bits(), 3);
        assert_eq!(update_flags(816).bits(), 816);
        // IMMEDIATE (8) has no counterpart.
        assert_eq!(update_flags(8).bits(), 0);
        assert_eq!(update_flags(RAW_WRITE_BITS).bits(), 2 | 16 | 32 | 256 | 512);
    }
}
