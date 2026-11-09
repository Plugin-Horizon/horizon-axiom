// Axiom (AxiomPaperPlugin, MIT, Copyright (c) 2023 Moulberry Pty Ltd) for a Horizon dedicated server: the game side.
// Staged into `crates/horizon-client/src/game.rs` as `mod axiom`, so it reaches the level in place (`Game::world`),
// the registry and the native helpers directly. The hook helpers in helpers/ only call in here.
//
// Blind port (written 2026-11-09 against Horizon 92fd45976, not built). Each engine function used is named in the
// native_* file it lives in. What has no engine function is a stub marked `// GAP:`, listed in GAPS.md.
// Hooks have no game in hand, so they queue events; `tick` (server tick start) answers them in order.
use super::native_configs::{NativeConfig, NativeConfigFile, NativeConfigKind};
use super::native_server::{NativeCommand, NativeServerPlayer};
use super::native_world_edit::{NativeBlockEdit, NativeBlockRun};
use super::Game;
use horizon_client::axiom::core::annotation::{
    self, AnnotationHost, AnnotationState, MarkerEntity,
};
use horizon_client::axiom::core::blueprint::{self, BlueprintHost, BlueprintStore};
use horizon_client::axiom::core::buf::{Nbt, Reader};
use horizon_client::axiom::core::buffer::{zstd_compress, zstd_decompress};
use horizon_client::axiom::core::constants::{
    CHANNEL_TUNNEL, SERVERBOUND_CHANNELS, TUNNEL_BUFFER_FLAG_FIRST, TUNNEL_BUFFER_FLAG_LAST,
    TUNNEL_PACKET_FLAG_ZSTD_COMPRESSED,
};
use horizon_client::axiom::core::custom::{
    self, CustomBlockRegistry, CustomDisplayRegistry, CustomHost, DebugHost, Provider,
};
use horizon_client::axiom::core::entity::{
    self, EntityHost, EntityTypeFilter, HiddenEntities, Pos,
};
use horizon_client::axiom::core::request::{
    self, BlockStates, EntityInfo, RequestChunksOperation, RequestHost, SectionRead,
};
use horizon_client::axiom::core::restrictions::{
    AxiomHandshakeEvent, AxiomModifyWorldEvent, Config, MessageColor, PlotBounds, RestrictionsHost,
    RestrictionsState,
};
use horizon_client::axiom::core::setblock::block_packet::{handle_set_block, BlockHit, Hand};
use horizon_client::axiom::core::setblock::buffer_packet::handle_buffer_packet;
use horizon_client::axiom::core::setblock::dispatch::DispatchBudget;
use horizon_client::axiom::core::setblock::tick_blocks::handle_tick_blocks;
use horizon_client::axiom::core::setblock::{
    BlockPos, ChunkLoadState, OperationQueue, SectionPermissionChecker, SetBlockConfig,
    SetBlockHost,
};
use horizon_client::axiom::core::world::packets::{
    set_fly_speed, set_gamemode, set_world_time, teleport,
};
use horizon_client::axiom::core::world::{GameMode, Location, MarkerEntry, WorldHost, WorldState};
use horizon_client::mod_api::ServerTickPhase;
use horizon_client::text::Text;
use horizon_world::nbt::{JavaString, Tag};
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

/// The mod id, the config file (`config/axiom-server.toml`) and the command names.
pub(super) const MOD_ID: &str = "axiom";

/// What a fresh server writes: the keys of Axiom's config.yml live in `restrictions::Config`, whose defaults apply
/// to every key not written here.
const DEFAULT_CONFIG: &str =
    "# Axiom server settings for Horizon. Keys as in AxiomPaperPlugin's config.yml; unset keys keep their defaults.\n";

/// The one UUID AxiomDebugCommand also admits to `/axiompaperdebug`, besides operators and the axiom.* nodes.
const DEBUG_EXTRA_UUID: u128 = 0xd0e0_5de7_6067_454d_beae_c6d1_9d88_6191;

/// The vanilla dimension keys.
const VANILLA_DIMENSIONS: [&str; 3] = [
    "minecraft:overworld",
    "minecraft:the_nether",
    "minecraft:the_end",
];

/// The owner name the chunk tickets of Axiom's requests are held under.
const CHUNK_TICKET_OWNER: &str = "axiom";

/// A random 64-bit value from the std hasher, which is seeded per process. Enough for handshake ids and
/// `UniformInt`-style draws; nothing here needs more.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// Logs a STUB call once per process, so a missing engine function shows up in the log without flooding it.
fn stub_once(name: &'static str) {
    static SEEN: AtomicU64 = AtomicU64::new(0);
    let bit = match name {
        "set_day_time" => 1,
        "set_time_frozen" => 2,
        "set_weather_paused" => 4,
        "set_weather" => 8,
        _ => 0,
    };
    if bit != 0 && SEEN.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!(
            "[WARN] [axiom] {name} is not supported on Horizon yet (see GAPS.md); nothing changed"
        );
    }
}

/// What the hooks heard, in order, until the next server tick.
enum Event {
    Joined {
        player: u128,
    },
    Left {
        player: u128,
    },
    Payload {
        player: u128,
        channel: String,
        data: Vec<u8>,
    },
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static CONFIG: RwLock<Option<Config>> = RwLock::new(None);
/// Bumped whenever `config/axiom-server.toml` is read or saved. The tick rebuilds the state when it changes.
static CONFIG_GEN: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The one Axiom state of this server, held by the server thread.
    static STATE: RefCell<Option<AxiomState>> = const { RefCell::new(None) };
}

/// Per-player reassembly of split tunnel packets (`AxiomPaper.tunnelPendingBuffers` / `skipCurrentTunnelPacket`).
#[derive(Default)]
struct TunnelState {
    buffers: Vec<Vec<u8>>,
    skip: bool,
}

/// Everything the Axiom core keeps for this server, in one place.
struct AxiomState {
    /// The `CONFIG_GEN` this state was built from.
    generation: u64,
    config: Config,
    restrictions: RestrictionsState,
    /// Players who completed the handshake (mirror of `RestrictionsState::is_active`, refreshed after each call).
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
    fn new(config: Config, generation: u64) -> Self {
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

/// Whether a serverbound channel is registered under this config. Java registers `upload_blueprint` and
/// `request_blueprint` only with `blueprint-sharing`, and `annotation_update` only with `allow-annotations`.
fn served(config: &Config, channel: &str) -> bool {
    match channel {
        "axiom:upload_blueprint" | "axiom:request_blueprint" => config.blueprint_sharing,
        "axiom:annotation_update" => config.allow_annotations,
        _ => true,
    }
}

fn queue(event: Event) {
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

pub(super) fn joined(player: u128) {
    queue(Event::Joined { player });
}

pub(super) fn left(player: u128) {
    queue(Event::Left { player });
}

pub(super) fn payload(player: u128, channel: &str, data: &[u8]) {
    queue(Event::Payload {
        player,
        channel: channel.to_owned(),
        data: data.to_vec(),
    });
}

/// `config/axiom-server.toml` was read (or saved): parse it with the core's `Config`. A file that does not parse
/// keeps the settings in force. A new config rebuilds the core state, so every client handshakes again.
pub(super) fn config_changed(file: &NativeConfigFile) {
    if file.mod_id != MOD_ID || !matches!(file.kind, NativeConfigKind::Server) {
        return;
    }
    match Config::from_toml(&file.text) {
        Ok(config) => {
            *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Some(config);
            CONFIG_GEN.fetch_add(1, Ordering::Relaxed);
        }
        Err(e) => eprintln!(
            "[WARN] [axiom] {}: {e}; keeping the settings in force",
            file.file
        ),
    }
}

/// The settings in force, or the defaults before a config was read.
pub(super) fn with_config<R>(read: impl FnOnce(&Config) -> R) -> R {
    let guard = CONFIG.read().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(config) => read(config),
        None => read(&Config::default()),
    }
}

pub(super) fn register_config(configs: &mut Vec<NativeConfig>) {
    configs.push(NativeConfig::new(
        MOD_ID,
        NativeConfigKind::Server,
        DEFAULT_CONFIG,
    ));
}

pub(super) fn register(commands: &mut Vec<NativeCommand>) {
    commands.push(NativeCommand::op("axiompaperdebug", 1));
    commands.push(NativeCommand::op("axiompapermigrateconfig", 4));
}

/// A command the mod registered, typed by a player whose level allows it.
pub(super) fn perform(
    game: &mut Game<'_>,
    player: &NativeServerPlayer,
    command: &str,
    ran: &mut bool,
) {
    let (word, args) = command.split_once(' ').unwrap_or((command, ""));
    match word {
        "axiompaperdebug" => {
            *ran = true;
            debug(game, player, args.trim());
        }
        "axiompapermigrateconfig" => {
            *ran = true;
            // Paper's config.yml migration has no meaning here: the settings are TOML and read as they are.
            game.native_feedback(Text::literal(
                "Axiom on Horizon reads config/axiom-server.toml; there is no Paper config to migrate.",
            ));
        }
        _ => {}
    }
}

/// `/axiompaperdebug`: the core's `debug_command` answers the subcommands, for an operator or a player with axiom.*.
fn debug(game: &mut Game<'_>, player: &NativeServerPlayer, args: &str) {
    let uuid = player.uuid;
    let allowed = player.permission > 0
        || game.native_has_permission(uuid, "axiom.all", 1)
        || game.native_has_permission(uuid, "axiom.debug", 1)
        || uuid == DEBUG_EXTRA_UUID;
    if !allowed {
        game.native_feedback(Text::literal(
            "You do not have permission to use this command.",
        ));
        return;
    }
    let (active, config) = snapshot();
    let text = {
        let host = AxiomHost::new(game, active, config);
        custom::debug_command(&host, Some(uuid), args)
            .unwrap_or_else(|| "This /axiompaperdebug subcommand is not known.".to_owned())
    };
    game.native_feedback(Text::literal(text));
}

/// The active players and the settings, for a command that runs outside the tick. Empty before the first tick.
fn snapshot() -> (BTreeSet<u128>, Config) {
    STATE.with(|cell| match cell.try_borrow() {
        Ok(slot) => match slot.as_ref() {
            Some(state) => (state.active.clone(), state.config.clone()),
            None => (BTreeSet::new(), with_config(|c| c.clone())),
        },
        Err(_) => (BTreeSet::new(), with_config(|c| c.clone())),
    })
}

/// Server tick start: answers what the hooks queued, in order, then runs the core's per-tick work.
/// Other phases are left alone.
pub(super) fn tick(game: &mut Game<'_>, phase: ServerTickPhase) {
    if !matches!(phase, ServerTickPhase::ServerTickStart { .. }) {
        return;
    }
    let events = std::mem::take(&mut *EVENTS.lock().unwrap_or_else(|e| e.into_inner()));
    let generation = CONFIG_GEN.load(Ordering::Relaxed);
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().is_none_or(|s| s.generation != generation) {
            *slot = Some(AxiomState::new(with_config(|c| c.clone()), generation));
        }
        let Some(state) = slot.as_mut() else {
            return;
        };
        for event in events {
            match event {
                // Joins do nothing yet: Java sends its initial packets once the client has handshaked
                // (`onAxiomActive`), and that happens in `route` for `axiom:hello`.
                Event::Joined { .. } => {}
                Event::Left { player } => left_player(state, player),
                Event::Payload {
                    player,
                    channel,
                    data,
                } => {
                    let mut host = AxiomHost::new(game, state.active.clone(), state.config.clone());
                    payload_in(&mut host, state, player, &channel, &data);
                }
            }
        }
        run_ticks(game, state);
    });
}

/// A player left: drop the parts of the state that only they own. The restrictions state drops them on its own slow tick.
fn left_player(state: &mut AxiomState, player: u128) {
    state.tunnels.remove(&player);
    state.requests.retain(|op| op.executor() != player);
    state.world.set_no_physical_trigger(player, false);
    state.active.remove(&player);
}

/// The core's per-tick work, in Java's order.
fn run_ticks(game: &mut Game<'_>, state: &mut AxiomState) {
    let mut host = AxiomHost::new(game, state.active.clone(), state.config.clone());
    if !state.blueprints_loaded {
        state.blueprints_loaded = true;
        if host.config.blueprint_sharing {
            if let Some(root) = host.world_dir().map(|d| d.join("axiom").join("blueprints")) {
                let (store, errors) = BlueprintStore::load(&host, root);
                for (path, e) in errors {
                    eprintln!("[WARN] [axiom] blueprint {}: {e:?}", path.display());
                }
                state.blueprints = store;
            }
        }
    }
    state.restrictions.tick(&mut host);
    sync_active(&mut host, state);
    state.operations.tick(&mut host);
    for op in state.requests.iter_mut() {
        op.tick(&mut host);
    }
    state.requests.retain(|op| !op.is_finished());
    let (markers, relights, sends) = (
        host.config.send_markers,
        host.config.max_chunk_relights_per_tick,
        host.config.max_chunk_sends_per_tick,
    );
    state.world.tick(&mut host, markers, relights, sends);
    for dim in VANILLA_DIMENSIONS {
        annotation::tick_markers(&mut host, &mut state.annotations, dim);
    }
    state.custom_blocks.tick(&mut host);
    state.custom_displays.tick(&mut host);
    state.hidden.tick(&mut host);
    sync_active(&mut host, state);
}

/// Copies the restrictions' active set into the host and the state, after anything that may have changed it.
fn sync_active(host: &mut AxiomHost<'_, '_>, state: &mut AxiomState) {
    host.active = host
        .online_uuids()
        .into_iter()
        .filter(|p| state.restrictions.is_active(*p))
        .collect();
    state.active = host.active.clone();
}

/// Routes one payload the client sent on an `axiom:*` channel: the tunnel, or a packet on its own channel.
fn payload_in(
    host: &mut AxiomHost<'_, '_>,
    state: &mut AxiomState,
    player: u128,
    channel: &str,
    data: &[u8],
) {
    if channel == CHANNEL_TUNNEL {
        tunnel_buffer(host, state, player, data);
        return;
    }
    // A packet on its own channel is only registered when Java registers it (see `served`); anything else is ignored.
    if !SERVERBOUND_CHANNELS.contains(&channel) || !served(&host.config, channel) {
        return;
    }
    if let Err(e) = route(host, state, player, channel, data) {
        eprintln!("[WARN] [axiom] {channel} from {player:032x}: {e}");
        host.kick_player(player, &format!("Axiom: Malformed {channel} packet"));
    }
}

/// `axiom:tunnel`: one buffer of a packet that may be split (`AxiomPaper.handleTunnelBuffer`).
fn tunnel_buffer(host: &mut AxiomHost<'_, '_>, state: &mut AxiomState, player: u128, data: &[u8]) {
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
        } else if tunnel.skip {
            return;
        }
        if first {
            tunnel.buffers.clear();
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
fn tunnel_packet(host: &mut AxiomHost<'_, '_>, state: &mut AxiomState, player: u128, data: &[u8]) {
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
                eprintln!("[WARN] [axiom] {id} from {player:032x}: zstd tunnel packet: {e}");
                host.kick_player(player, "Axiom: Malformed tunnel packet");
                return;
            }
        }
    } else {
        raw
    };
    if let Err(e) = route(host, state, player, &id, &body) {
        eprintln!("[WARN] [axiom] {id} from {player:032x}: {e}");
        host.kick_player(player, &format!("Axiom: Failed to read {id} packet"));
    }
}

/// Stringifies a core error (all of them derive Debug or Display).
fn err<E: std::fmt::Debug>(e: E) -> String {
    format!("{e:?}")
}

/// Routes one packet body to its core handler. `channel` is the full name, e.g. `axiom:set_block`.
/// Errors are decode errors (or unknown channels); the caller kicks the player.
fn route(
    host: &mut AxiomHost<'_, '_>,
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
            // Blueprint errors are logged inside the core's file IO and never a decode error of the packet.
            if let Err(e) = blueprint::handle_upload(host, &mut state.blueprints, player, body) {
                eprintln!("[WARN] [axiom] upload_blueprint from {player:032x}: {e:?}");
            }
        }
        "axiom:request_blueprint" => {
            if let Err(e) = blueprint::handle_request(host, &state.blueprints, player, body) {
                eprintln!("[WARN] [axiom] request_blueprint from {player:032x}: {e:?}");
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

/// What Java's `AxiomPaper.sendInitialPackets` sends once a player is active: world properties, the world's
/// extension (markers), the blueprint manifest, annotations and custom blocks. Heightmaps are not sent (GAP).
fn send_initial(host: &mut AxiomHost<'_, '_>, state: &mut AxiomState, player: u128) {
    let dim = host
        .dimension_of(player)
        .unwrap_or_else(|| VANILLA_DIMENSIONS[0].to_owned());
    state.world.send_world_properties(host, player, &dim);
    state.world.on_player_join(host, player, &dim);
    if let Err(e) = blueprint::send_manifest(host, &state.blueprints, &[player]) {
        eprintln!("[WARN] [axiom] blueprint manifest for {player:032x}: {e:?}");
    }
    if let Err(e) = annotation::send_all(host, &mut state.annotations, &dim, player) {
        eprintln!("[WARN] [axiom] annotations for {player:032x}: {e:?}");
    }
    state.custom_blocks.on_player_join(host, player);
    state.custom_displays.on_player_join(host, player);
}

/// The game as the core sees it. Players are UUIDs, blocks are state ids, dimensions are ids like
/// `minecraft:overworld`. Built per tick with the settings and the active set it needs; the level is the one in place.
struct AxiomHost<'a, 'g> {
    game: &'a mut Game<'g>,
    active: BTreeSet<u128>,
    config: Config,
    filter: EntityTypeFilter,
    /// Entities of the dimensions indexed for this call: client-visible uuid to (dimension, engine id).
    entities: HashMap<u128, (String, u32)>,
}

impl<'a, 'g> AxiomHost<'a, 'g> {
    fn new(game: &'a mut Game<'g>, active: BTreeSet<u128>, config: Config) -> Self {
        let filter = EntityTypeFilter::new(
            config.whitelist_entities.clone(),
            config.blacklist_entities.clone(),
        );
        Self {
            game,
            active,
            config,
            filter,
            entities: HashMap::new(),
        }
    }

    fn world_dir(&self) -> Option<PathBuf> {
        self.game.native_world_dir()
    }

    fn players(&self) -> Vec<NativeServerPlayer> {
        self.game.native_server_players()
    }

    fn online_uuids(&self) -> Vec<u128> {
        self.players().into_iter().map(|p| p.uuid).collect()
    }

    fn dimension_of(&self, player: u128) -> Option<String> {
        self.players()
            .into_iter()
            .find(|p| p.uuid == player)
            .map(|p| p.dimension)
    }

    fn position_of(&self, player: u128) -> Option<[f64; 3]> {
        self.players()
            .into_iter()
            .find(|p| p.uuid == player)
            .map(|p| p.position)
    }

    /// Whether `dimension` is the level being ticked, the one the game holds in place.
    fn here(&self, dimension: &str) -> bool {
        dimension == self.game.dim.id()
    }

    fn kick_player(&mut self, player: u128, reason: &str) {
        let _ = self.game.native_kick(player, &Text::literal(reason));
    }

    fn send_raw(&mut self, player: u128, channel: &str, data: &[u8]) {
        if let Err(e) = self
            .game
            .native_send_custom_payload_to_player(player, channel, data)
        {
            eprintln!("[WARN] [axiom] {channel}: {e}");
        }
    }

    fn tell_colored(&mut self, player: u128, text: &str, color: u32) {
        let _ = self.game.native_tell(player, Text::colored(text, color));
    }

    /// The block state at `pos`: from the level in place, or from a loaded chunk of another level. None where nothing
    /// is loaded.
    fn state_at(&self, dimension: &str, pos: [i32; 3]) -> Option<u32> {
        if self.here(dimension) {
            let w = &self.game.world;
            if pos[1] < w.bottom_y() || pos[1] >= w.top_y() || !w.loaded(pos[0], pos[2]) {
                return None;
            }
            return Some(w.block(pos[0], pos[1], pos[2]).0);
        }
        self.game
            .native_chunk_snapshot(dimension, [pos[0] >> 4, pos[2] >> 4])?
            .block_state(pos[0], pos[1], pos[2])
            .map(|state| state.0)
    }

    /// Block state `state` as a block input reads it: `minecraft:stone` or `minecraft:oak_stairs[facing=east,half=top]`.
    fn palette_name(&self, state: u32) -> Option<String> {
        let s = self.game.pack.assets.registry.state(BlockStateId(state))?;
        let props: Vec<String> = s
            .properties
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        Some(if props.is_empty() {
            s.name.to_string()
        } else {
            format!("{}[{}]", s.name, props.join(","))
        })
    }

    /// The state id of a block with its default properties, as the registry resolves it.
    fn default_state_of(&self, name: &str) -> Option<u32> {
        let descriptor = horizon_world::chunk::BlockState {
            name: name.to_owned(),
            properties: BTreeMap::<String, String>::new().into(),
        };
        self.game
            .pack
            .assets
            .registry
            .resolve(&descriptor)
            .ok()
            .map(|id| id.0)
    }

    fn global_bits_value(&self) -> u8 {
        // Vanilla: the bit width of the global block state map, `ceil(log2(state count))`, at least 4 for the palette.
        let count = self.game.pack.assets.registry.len().max(2) as u32;
        (32 - (count - 1).leading_zeros()).max(4) as u8
    }

    /// The height range of the level in place as `(min_y, height)`.
    fn height(&self) -> (i32, i32) {
        self.game.native_world_height()
    }

    /// Whether the chunk is loaded in the level `dimension`.
    fn chunk_loaded(&self, dimension: &str, cx: i32, cz: i32) -> bool {
        self.game
            .native_chunk_snapshot(dimension, [cx, cz])
            .is_some()
    }

    /// Many single blocks at once, through `native_edit_blocks`: the engine writes them into the chunks, works out
    /// their light and heightmaps, and sends the chunks whole. Block entities are not written here.
    fn put_blocks(
        &mut self,
        dimension: &str,
        blocks: &[([i32; 3], u32)],
        neighbour_updates: bool,
    ) -> Result<u64, String> {
        let mut palette: Vec<String> = Vec::new();
        let mut index: BTreeMap<u32, u32> = BTreeMap::new();
        let mut runs = Vec::with_capacity(blocks.len());
        for &([x, y, z], state) in blocks {
            let block = match index.get(&state) {
                Some(&i) => i,
                None => {
                    let name = self
                        .palette_name(state)
                        .ok_or_else(|| format!("block state {state} is not in the registry"))?;
                    let i = palette.len() as u32;
                    palette.push(name);
                    index.insert(state, i);
                    i
                }
            };
            runs.push(NativeBlockRun {
                x,
                z,
                y,
                height: 1,
                surface: None,
                block,
                only: None,
            });
        }
        let edit = NativeBlockEdit {
            palette,
            filters: Vec::new(),
            runs,
            neighbour_updates,
        };
        self.game.native_edit_blocks(dimension, &edit)
    }

    /// One block with its state and `Level.setBlock` flags (`native_set_block`). Only the level in place: false elsewhere.
    fn put_block(&mut self, dimension: &str, pos: [i32; 3], state: u32, flags: i32) -> bool {
        self.game
            .native_set_block(dimension, pos, BlockStateId(state), flags)
    }

    /// Indexes the entities of `dimension` by uuid, for the calls of this host. Scans every entity of the level
    /// GAP: Entity uuid lookup (GAPS.md): every entity of the level is scanned on each call.
    fn index_dimension(&mut self, dimension: &str) {
        let ids = self
            .game
            .native_entities_in(dimension, [-3.0e7; 3], [3.0e7; 3]);
        for id in ids {
            if let Some(entity) = self.game.native_entity(id) {
                if let Some(uuid) = self.game.native_entity_uuid(entity) {
                    self.entities.insert(uuid, (dimension.to_owned(), id));
                }
            }
        }
    }

    fn index_dimension_of(&mut self, player: u128) {
        if let Some(dim) = self.dimension_of(player) {
            self.index_dimension(&dim);
        }
    }

    /// The engine id and dimension of an indexed entity.
    fn entity_id(&self, uuid: u128) -> Option<u32> {
        self.entities.get(&uuid).map(|(_, id)| *id)
    }

    /// Settings of the block packets, as `SetBlockPacketListener` reads them.
    fn set_block_config(&self) -> SetBlockConfig {
        SetBlockConfig {
            packet_collection_read_limit: self.config.packet_collection_read_limit() as usize,
            log_large_block_buffer_changes: self.config.log_large_block_buffer_changes,
        }
    }

    /// Reads a section of chunk `(cx, sy, cz)` in the level `dimension` from its snapshot.
    fn section_read(&self, dimension: &str, cx: i32, sy: i32, cz: i32) -> SectionRead {
        let Some(snapshot) = self.game.native_chunk_snapshot(dimension, [cx, cz]) else {
            return SectionRead::Missing;
        };
        let (min_y, height) = self.height();
        let base = sy * 16;
        if base < min_y || base + 16 > min_y + height {
            return SectionRead::Missing;
        }
        let air = self.default_state_of("minecraft:air").unwrap_or(0);
        let mut states: BlockStates = [air; 4096];
        let mut all_air = true;
        for y in 0..16i32 {
            for z in 0..16i32 {
                for x in 0..16i32 {
                    let state = snapshot
                        .block_state(cx * 16 + x, base + y, cz * 16 + z)
                        .map_or(air, |s| s.0);
                    all_air &= state == air;
                    states[((y << 8) | (z << 4) | x) as usize] = state;
                }
            }
        }
        if all_air {
            SectionRead::Empty
        } else {
            SectionRead::Blocks(Box::new(states))
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
        Nbt::String(s) => Tag::String(JavaString::from(s.as_str())),
        Nbt::List(v) => Tag::List(v.iter().map(core_to_tag).collect()),
        Nbt::Compound(c) => Tag::Compound(
            c.iter()
                .map(|(k, v)| (JavaString::from(k.as_str()), core_to_tag(v)))
                .collect::<BTreeMap<JavaString, Tag>>(),
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

impl RestrictionsHost for AxiomHost<'_, '_> {
    fn is_op(&self, player: u128) -> bool {
        self.game
            .native_player_permission(player)
            .is_some_and(|level| level > 0)
    }

    fn has_permission(&self, player: u128, node: &str) -> bool {
        // Operators (level 1 and up) get what no permission provider decides, as Bukkit's unset nodes do.
        self.game.native_has_permission(player, node, 1)
    }

    fn permission_value(&self, player: u128, node: &str) -> Option<bool> {
        // The permission providers answer TRUE or FALSE; nobody answering is NOT_SET.
        let mut answer = None;
        self.game
            .native_permission_check(player, node, 1, &mut answer);
        answer
    }

    fn is_online(&self, player: u128) -> bool {
        self.players().iter().any(|p| p.uuid == player)
    }

    fn online_players(&self) -> Vec<u128> {
        self.online_uuids()
    }

    fn world_name(&self, player: u128) -> String {
        // The dimension id, where Bukkit gives the world's folder name.
        self.dimension_of(player).unwrap_or_default()
    }

    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.send_raw(player, channel, &data);
    }

    fn send_message(&mut self, player: u128, text: &str, color: MessageColor) {
        // Minecraft's own red and yellow.
        let color = match color {
            MessageColor::Red => 0xFF5555,
            MessageColor::Yellow => 0xFFFF55,
        };
        self.tell_colored(player, text, color);
    }

    fn kick(&mut self, player: u128, reason: &str) {
        self.kick_player(player, reason);
    }

    fn random_i64(&mut self) -> i64 {
        random_u64() as i64
    }

    fn server_protocol_version(&self) -> i32 {
        horizon_protocol::PROTOCOL_VERSION
    }

    fn server_data_version(&self) -> i32 {
        horizon_world::DATA_VERSION
    }

    fn plugin_enabled(&self, _name: &str) -> bool {
        // Horizon runs no Bukkit plugins: ViaVersion, LuckPerms, PlotSquared, WorldGuard and CoreProtect are absent.
        false
    }

    fn protocol_name(&self, protocol: i32) -> String {
        if protocol == horizon_protocol::PROTOCOL_VERSION {
            horizon_core::version::VERSION_ID.to_owned()
        } else {
            protocol.to_string()
        }
    }

    fn map_client_block_registry(&mut self, _player: u128, _protocol: i32) -> Result<(), String> {
        // Horizon serves its own protocol only (a client on another one is refused before this is asked), so there
        // is no ViaVersion block mapping to build.
        Err("Horizon has no ViaVersion: only its own protocol is served".to_owned())
    }

    fn is_plot_world(&self, _player: u128) -> bool {
        false
    }

    fn editable_plot(&self, _player: u128) -> Option<PlotBounds> {
        None
    }

    fn matches_regex(&self, _pattern: &str, _text: &str) -> bool {
        // GAP: regex matcher (GAPS.md, "World whitelist and blacklist"). `false` refuses the world, so a configured world filter fails closed.
        false
    }

    fn on_axiom_handshake(&mut self, _event: &mut AxiomHandshakeEvent) {
        // No other plugin listens on Horizon.
    }

    fn on_modify_world(&mut self, _event: &mut AxiomModifyWorldEvent) {}
}

/// Worlds, time, weather, game modes, teleports and chunk tracking. Implemented on the engine's level in place and the
/// native helpers; what Horizon has no function for is a STUB that logs once and changes nothing.
impl WorldHost for AxiomHost<'_, '_> {
    fn world_names(&self) -> Vec<String> {
        // The three vanilla dimensions. Which of them are loaded is not checked here.
        VANILLA_DIMENSIONS.iter().map(|d| (*d).to_owned()).collect()
    }

    fn world_exists(&self, dim: &str) -> bool {
        VANILLA_DIMENSIONS.contains(&dim)
    }

    fn player_dimension(&self, player: u128) -> Option<String> {
        self.dimension_of(player)
    }

    fn players_in_dimension(&self, dim: &str) -> Vec<u128> {
        self.players()
            .into_iter()
            .filter(|p| p.dimension == dim)
            .map(|p| p.uuid)
            .collect()
    }

    fn is_axiom_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }

    fn has_permission(&self, player: u128, node: &str) -> bool {
        // The Bukkit node only: operators pass, others need a permission provider's answer. The core's permission set
        // (defaults, inheritance) is not applied yet, so a node that defaults to true in plugin.yml is refused here.
        self.game.native_has_permission(player, node, 1)
    }

    fn can_modify_world(&self, _player: u128, _dim: &str) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn allow_teleport_between_worlds(&self) -> bool {
        self.config.allow_teleport_between_worlds
    }

    fn is_plot_world(&self, _dim: &str) -> bool {
        false
    }

    fn set_day_time(&mut self, _dim: &str, _time: i32) {
        // GAP: time of day setter (GAPS.md, "Set the time of day"). Logs once, changes nothing.
        stub_once("set_day_time");
    }

    fn set_time_frozen(&mut self, _dim: &str, _frozen: bool) {
        // GAP: time of day setter (GAPS.md, "Set the time of day"). Logs once, changes nothing.
        stub_once("set_time_frozen");
    }

    fn weather_paused(&self, _dim: &str) -> bool {
        false
    }

    fn set_weather_paused(&mut self, _dim: &str, _paused: bool) {
        // GAP: game rule and weather setters (GAPS.md, "Game rule and weather setters"). Logs once, changes nothing.
        stub_once("set_weather_paused");
    }

    fn set_weather(
        &mut self,
        _dim: &str,
        _clear_time: i32,
        _rain_time: i32,
        _raining: bool,
        _thundering: bool,
    ) {
        // GAP: game rule and weather setters (GAPS.md, "Game rule and weather setters"). Logs once, changes nothing.
        stub_once("set_weather");
    }

    fn random_int(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        let span = (max as i64 - min as i64 + 1) as u64;
        (min as i64 + (random_u64() % span) as i64) as i32
    }

    fn set_game_mode(&mut self, player: u128, mode: GameMode) {
        let _ = self.game.native_set_game_mode(player, mode.id() as i32);
    }

    fn set_fly_speed(&mut self, player: u128, speed: f32) {
        // Called from a server tick, which the engine asks for (`native_set_flying_speed`).
        let _ = self.game.native_set_flying_speed(player, Some(speed));
    }

    fn teleport_player(&mut self, player: u128, dim: &str, at: Location) {
        let _ = self
            .game
            .native_teleport_player(player, dim, [at.x, at.y, at.z], at.yaw, at.pitch);
    }

    fn is_chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool {
        self.chunk_loaded(dim, cx, cz)
    }

    fn players_tracking_chunk(&self, dim: &str, _cx: i32, _cz: i32) -> Vec<u128> {
        // Approximation: every player in the dimension (over-sends, never misses one).
        self.players_in_dimension(dim)
    }

    fn send_chunk_with_light(&mut self, _players: &[u128], _dim: &str, _cx: i32, _cz: i32) {
        // No-op by design: `native_edit_blocks` sends the chunks it changed whole, with their light.
    }

    fn relight_chunks(&mut self, _dim: &str, _chunks: &BTreeSet<(i32, i32)>) {
        // No-op by design: `native_edit_blocks` relights the chunks it changed (`dirty_light`).
    }

    fn marker_snapshot(&self, _dim: &str) -> Vec<MarkerEntry> {
        // GAP: marker listing (GAPS.md, "Entity NBT reload and listing for display, marker and interaction kinds").
        Vec::new()
    }

    fn anti_xray_enabled(&self, _player: u128) -> bool {
        // Horizon has no anti-xray.
        false
    }

    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.send_raw(player, channel, &data);
    }

    fn send_warning(&mut self, player: u128, text: &str) {
        self.tell_colored(player, text, 0xFF5555);
    }

    fn send_chat_to_dimension(&mut self, dim: &str, text: &str) {
        for player in self.players_in_dimension(dim) {
            let _ = self.game.native_tell(player, Text::literal(text));
        }
    }
}

impl SetBlockHost for AxiomHost<'_, '_> {
    fn player_dimension(&self, player: u128) -> String {
        self.dimension_of(player)
            .unwrap_or_else(|| VANILLA_DIMENSIONS[0].to_owned())
    }

    fn player_block_pos(&self, player: u128) -> BlockPos {
        let [x, y, z] = self.position_of(player).unwrap_or([0.0; 3]);
        (x.floor() as i32, y.floor() as i32, z.floor() as i32)
    }

    fn scoreboard_name(&self, player: u128) -> String {
        self.players()
            .into_iter()
            .find(|p| p.uuid == player)
            .map(|p| p.name)
            .unwrap_or_default()
    }

    fn is_axiom_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }

    fn has_axiom_permission(&self, player: u128, node: &str) -> bool {
        self.game.native_has_permission(player, node, 1)
    }

    fn has_bukkit_permission(&self, player: u128, node: &str) -> bool {
        self.game.native_has_permission(player, node, 1)
    }

    fn can_use_game_master_blocks(&self, player: u128) -> bool {
        // Bukkit `canUseGameMasterBlocks`: operator level 2 and up.
        self.game
            .native_player_permission(player)
            .is_some_and(|level| level >= 2)
    }

    fn allowed_dispatch_sends_per_second(&self, player: u128) -> i64 {
        // The first `axiomlimits.<key>` override the player holds, else the default.
        for (key, sends) in self.config.dispatch_send_overrides() {
            if self
                .game
                .native_has_permission(player, &format!("axiomlimits.{key}"), 1)
            {
                return sends as i64;
            }
        }
        self.config.default_dispatch_sends_per_second() as i64
    }

    fn kick(&mut self, player: u128, reason: &str) {
        self.kick_player(player, reason);
    }

    fn send_system_message(&mut self, player: u128, text: &str, red: bool) {
        self.tell_colored(player, text, if red { 0xFF5555 } else { 0xFFFF55 });
    }

    fn broadcast_system_message(&mut self, text: &str) {
        self.game.native_broadcast_quiet(Text::literal(text));
    }

    fn log_info(&mut self, text: &str) {
        self.game.native_log("INFO", text);
    }

    fn world_allowed_by_config(&self, _dim: &str) -> bool {
        // GAP: world whitelist and blacklist regexes (GAPS.md, "World whitelist and blacklist"). Fails closed when either is set.
        self.config.whitelist_world_regex.is_empty() && self.config.blacklist_world_regex.is_empty()
    }

    fn modify_world_event(&mut self, _player: u128, _dim: &str, cancelled: bool) -> bool {
        !cancelled
    }

    fn max_chunk_load_distance(&self, _dim: &str) -> i32 {
        self.config.max_chunk_load_distance_for(false)
    }

    fn block_state_from_wire(&self, _player: u128, wire_id: i32) -> Option<u32> {
        // Horizon serves its own protocol, so a wire id is the global state id.
        let state = u32::try_from(wire_id).ok()?;
        self.game
            .pack
            .assets
            .registry
            .state(BlockStateId(state))
            .map(|_| state)
    }

    fn is_air(&self, state: u32) -> bool {
        self.game.res.facts().is_air(BlockStateId(state))
    }

    fn empty_state(&self) -> u32 {
        self.default_state_of("minecraft:air").unwrap_or(0)
    }

    fn global_bits(&self) -> u8 {
        self.global_bits_value()
    }

    fn same_block(&self, a: u32, b: u32) -> bool {
        match (self.palette_name(a), self.palette_name(b)) {
            (Some(x), Some(y)) => x.split('[').next() == y.split('[').next(),
            _ => a == b,
        }
    }

    fn state_can_be_replaced(&self, state: u32) -> bool {
        // Approximation of `BlockState.canBeReplaced`: air and liquids only (replaceable plants are not listed).
        self.is_air(state) || self.is_liquid_block(state)
    }

    fn has_block_entity(&self, _state: u32) -> bool {
        // GAP: block entity types per state (GAPS.md, "Block-state properties"). False: no block entity to clear.
        false
    }

    fn fluid_is_empty(&self, state: u32) -> bool {
        !self.is_liquid_block(state)
    }

    fn is_liquid_block(&self, state: u32) -> bool {
        matches!(self.palette_name(state).as_deref(), Some(n) if n.starts_with("minecraft:water") || n.starts_with("minecraft:lava"))
    }

    fn is_bed_double_plant_or_door(&self, state: u32) -> bool {
        // Approximation by block name (GAPS.md, "Block-state properties").
        match self.palette_name(state) {
            Some(n) => {
                let name = n.split('[').next().unwrap_or("");
                name.ends_with("_bed") || name.ends_with("_door") || name.ends_with("_tall_grass")
            }
            None => false,
        }
    }

    fn light_properties_differ(&self, old: u32, new: u32) -> bool {
        // Conservative: relight whenever the state changes (GAPS.md, "Light and send budgets per tick").
        old != new
    }

    fn block_entity_must_be_removed(&self, old: u32, new: u32) -> bool {
        old != new && self.has_block_entity(old)
    }

    fn is_game_master_block(&self, dim: &str, pos: BlockPos) -> bool {
        let Some(state) = self.state_at(dim, [pos.0, pos.1, pos.2]) else {
            return false;
        };
        matches!(
            self.palette_name(state)
                .as_deref()
                .and_then(|n| n.split('[').next()),
            Some(
                "minecraft:command_block"
                    | "minecraft:chain_command_block"
                    | "minecraft:repeating_command_block"
                    | "minecraft:structure_block"
                    | "minecraft:jigsaw"
                    | "minecraft:barrier"
                    | "minecraft:structure_void"
            )
        )
    }

    fn update_from_neighbour_shapes(&self, _dim: &str, _pos: BlockPos, state: u32) -> u32 {
        // GAP: neighbour shape updates (GAPS.md, "Block-state properties"). The state is written as given.
        state
    }

    fn is_block_loaded(&self, dim: &str, pos: BlockPos) -> bool {
        self.state_at(dim, [pos.0, pos.1, pos.2]).is_some()
    }

    fn is_chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool {
        self.chunk_loaded(dim, cx, cz)
    }

    fn request_chunk_load(&mut self, dim: &str, cx: i32, cz: i32) {
        self.game
            .native_force_chunk_in(CHUNK_TICKET_OWNER, dim, [cx, cz], true);
    }

    fn chunk_load_state(&self, dim: &str, cx: i32, cz: i32) -> ChunkLoadState {
        if self.chunk_loaded(dim, cx, cz) {
            ChunkLoadState::Loaded
        } else {
            ChunkLoadState::Pending
        }
    }

    fn section_y_range(&self, _dim: &str) -> (i32, i32) {
        let (min_y, height) = self.height();
        (min_y.div_euclid(16), (min_y + height).div_euclid(16) - 1)
    }

    fn section_has_only_air(&self, _dim: &str, _cx: i32, _cy: i32, _cz: i32) -> bool {
        // GAP: section emptiness (GAPS.md, "Section export for chunk requests"). False is conservative: the section counts as non-empty.
        false
    }

    fn block_state(&self, dim: &str, pos: BlockPos) -> u32 {
        let air = self.empty_state();
        self.state_at(dim, [pos.0, pos.1, pos.2]).unwrap_or(air)
    }

    fn set_block_raw(&mut self, dim: &str, pos: BlockPos, state: u32) -> u32 {
        let old = self.block_state(dim, pos);
        // Flags 2 (send to clients), no neighbour updates: Java's raw write.
        self.put_block(dim, [pos.0, pos.1, pos.2], state, 2);
        old
    }

    fn update_heightmaps(&mut self, _dim: &str, _pos: BlockPos, _new_state: u32) {
        // Maintained by the engine on every write (`native_set_block`, `native_edit_blocks`).
    }

    fn mark_chunk_unsaved(&mut self, _dim: &str, _cx: i32, _cz: i32) {
        // Maintained by the engine: a written chunk is saved with the level.
    }

    fn mark_block_changed(&mut self, _dim: &str, _pos: BlockPos) {
        // GAP: block change hooks for bulk writes (GAPS.md, "Actor on bulk block changes (CoreProtect logging)").
    }

    fn check_block_light(&mut self, _dim: &str, _pos: BlockPos) {
        // Light is worked out by the engine on each write.
    }

    fn update_section_emptiness(
        &mut self,
        _dim: &str,
        _cx: i32,
        _cy: i32,
        _cz: i32,
        _has_only_air: bool,
    ) {
        // Kept by the engine's section storage.
    }

    fn update_poi(&mut self, _dim: &str, _pos: BlockPos, _old: u32, _new: u32) {
        // GAP: point-of-interest updates (GAPS.md, "Block-state properties"). Nothing to update.
    }

    fn remove_block_entity(&mut self, _dim: &str, _pos: BlockPos) {
        // Writing an air block over the position removes its block entity.
    }

    fn sync_block_entity(&mut self, dim: &str, pos: BlockPos, _state: u32) -> bool {
        self.game
            .native_block_entity_tag(dim, [pos.0, pos.1, pos.2])
            .is_some()
    }

    fn load_block_entity_nbt(&mut self, dim: &str, pos: BlockPos, nbt: &[u8]) {
        let mut r = Reader::new(nbt);
        let Ok(Nbt::Compound(map)) = r.read_nbt() else {
            eprintln!("[WARN] [axiom] block entity NBT at {pos:?} is not a compound; not written");
            return;
        };
        let Tag::Compound(compound) = core_to_tag(&Nbt::Compound(map)) else {
            return;
        };
        self.game
            .native_set_block_tag(dim, [pos.0, pos.1, pos.2], Tag::Compound(compound));
    }

    fn send_chunk(&mut self, _dim: &str, _cx: i32, _cz: i32) {
        // No-op by design: `native_set_block` and `native_edit_blocks` send the chunks they change.
    }

    fn light_chunk(&mut self, _dim: &str, _cx: i32, _cz: i32) {
        // No-op by design: the engine relights the chunks it changes.
    }

    fn biome_exists(&self, _key: &str) -> bool {
        // GAP: biome write (GAPS.md, "Write biomes"). No biome is writable, so biome buffers are refused.
        false
    }

    fn set_biome_quart(&mut self, _dim: &str, _qx: i32, _qy: i32, _qz: i32, _key: &str) {
        // GAP: biome write (GAPS.md, "Write biomes"). Nothing changed.
    }

    fn players_tracking_chunk(&self, dim: &str, _cx: i32, _cz: i32) -> Vec<u128> {
        // Approximation: every player in the dimension.
        self.players_in_dimension(dim)
    }

    fn send_chunk_biomes(&mut self, _player: u128, _dim: &str, _chunks: &[(i32, i32)]) {
        // GAP: biome write (GAPS.md, "Write biomes"). Nothing to send.
    }

    fn region_can_break(&self, _player: u128, _dim: &str, _pos: BlockPos) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn region_can_place(&self, _player: u128, _dim: &str, _pos: BlockPos) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn region_section(
        &self,
        _player: u128,
        _dim: &str,
        _cx: i32,
        _cy: i32,
        _cz: i32,
    ) -> SectionPermissionChecker {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). No block of the section is allowed.
        SectionPermissionChecker::from_allowed_boxes(Vec::new())
    }

    fn ack_block_changes(&mut self, _player: u128, _seq: i32) {
        // GAP: block change acknowledgement (GAPS.md, "Block change acknowledgement"). Not sent.
    }

    fn fire_player_interact(
        &mut self,
        _player: u128,
        _breaking: bool,
        _hit: &BlockHit,
        _hand: Hand,
        _dim: &str,
    ) -> bool {
        // No Bukkit events on Horizon: never cancelled.
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
        // No Bukkit events on Horizon: never cancelled.
        true
    }

    fn clicked_pos(&self, _player: u128, _hand: Hand, dim: &str, hit: &BlockHit) -> BlockPos {
        // The hit block, unless it cannot be replaced: then the block on the clicked side (BlockPlaceContext).
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
        // GAP: item components onto block entities (GAPS.md, "Block-state properties"). Nothing applied.
    }

    fn set_placed_by(
        &mut self,
        _player: u128,
        _hand: Hand,
        _dim: &str,
        _pos: BlockPos,
        _state: u32,
    ) {
        // GAP: placed-by hook (GAPS.md, "Block-state properties"). Nothing to call.
    }

    fn set_block(&mut self, dim: &str, pos: BlockPos, state: u32, flags: u32) {
        self.put_block(dim, [pos.0, pos.1, pos.2], state, flags as i32);
    }

    fn tick_fluid(&mut self, _dim: &str, _pos: BlockPos, _state: u32) {
        // GAP: scheduled fluid ticks (GAPS.md, "Block-state properties"). The engine ticks its own fluids.
    }

    fn tick_block(&mut self, _dim: &str, _pos: BlockPos, _state: u32) {
        // GAP: scheduled block ticks (GAPS.md, "Block-state properties"). The engine ticks its own blocks.
    }
}

impl EntityHost for AxiomHost<'_, '_> {
    fn is_axiom_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }

    fn has_axiom_permission(&self, player: u128, node: &str) -> bool {
        self.game.native_has_permission(player, node, 1)
    }

    fn can_modify_world(&self, _player: u128, _dim: &str) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
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
    ) -> Result<Option<Nbt>, horizon_client::axiom::core::buf::BufError> {
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

    fn can_place_block(&self, _player: u128, _dim: &str, _x: i32, _y: i32, _z: i32) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn can_break_block(&self, _player: u128, _dim: &str, _x: i32, _y: i32, _z: i32) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn is_in_spawnable_bounds(&self, x: i32, y: i32, z: i32) -> bool {
        // The packed block position limits (`AxiomConstants.MIN_POSITION_LONG` and its opposite end).
        (-33_554_432..33_554_432).contains(&x)
            && (-2048..2048).contains(&y)
            && (-33_554_432..33_554_432).contains(&z)
    }

    fn fire_spawn(&mut self, _player: u128, _uuid: u128) -> bool {
        // GAP: cancellable entity events (GAPS.md, "Cancel entity spawn, removal and manipulation"). Never cancelled.
        false
    }

    fn fire_manipulate(&mut self, _player: u128, _uuid: u128) -> bool {
        // GAP: cancellable entity events (GAPS.md, "Cancel entity spawn, removal and manipulation"). Never cancelled.
        false
    }

    fn fire_after_manipulate(&mut self, _player: u128, _uuid: u128) {}

    fn fire_remove(&mut self, _player: u128, _uuid: u128) -> bool {
        // GAP: cancellable entity events (GAPS.md, "Cancel entity spawn, removal and manipulation"). Never cancelled.
        false
    }

    fn has_entity(&self, dim: &str, uuid: u128) -> bool {
        self.entities.get(&uuid).is_some_and(|(d, _)| d == dim)
    }

    fn entity_type_id(&self, uuid: u128) -> Option<String> {
        let id = self.entity_id(uuid)?;
        let entity = self.game.native_entity(id)?;
        self.game.native_entity_kind(entity)
    }

    fn type_can_serialize(&self, type_id: &str) -> bool {
        // Players are never saved as entities.
        type_id != entity::TYPE_PLAYER
    }

    fn position(&self, uuid: u128) -> Pos {
        let position = self
            .entity_id(uuid)
            .and_then(|id| self.game.native_entity(id))
            .and_then(|entity| self.game.native_entity_position(entity))
            .unwrap_or([0.0; 3]);
        (position[0], position[1], position[2])
    }

    fn yaw(&self, uuid: u128) -> f32 {
        self.rotation_of(uuid)[0]
    }

    fn pitch(&self, uuid: u128) -> f32 {
        self.rotation_of(uuid)[1]
    }

    fn is_passenger(&self, _uuid: u128) -> bool {
        // GAP: riding and passengers (GAPS.md, "Riding, passengers and hanging entities"). Nobody rides.
        false
    }

    fn vehicle(&self, _uuid: u128) -> Option<u128> {
        // GAP: riding and passengers (GAPS.md, "Riding, passengers and hanging entities").
        None
    }

    fn passengers(&self, _uuid: u128) -> Vec<u128> {
        // GAP: riding and passengers (GAPS.md, "Riding, passengers and hanging entities").
        Vec::new()
    }

    fn is_removed(&self, uuid: u128) -> bool {
        !self.entities.contains_key(&uuid)
    }

    fn item_frame_on_y_axis(&self, _uuid: u128) -> bool {
        // GAP: item frame orientation (GAPS.md, "Riding, passengers and hanging entities"). Treated as horizontal.
        false
    }

    fn entity_as_passenger_nbt(&mut self, dim: &str, uuid: u128) -> Option<Nbt> {
        self.saved_entity(dim, uuid)
    }

    fn load_entity_tree(
        &mut self,
        _dim: &str,
        _tag: &Nbt,
        _root_uuid: u128,
    ) -> Option<entity::EntityNode> {
        // GAP: spawn with a client-chosen uuid (GAPS.md, "Entity spawn with the client's uuid"). Nothing is summoned.
        None
    }

    fn set_hanging_turn(&mut self, _uuid: u128, _turn: entity::Turn) {
        // GAP: hanging entity orientation (GAPS.md, "Riding, passengers and hanging entities").
    }

    fn adjust_item_frame_rotation(&mut self, _uuid: u128, _delta: i32) {
        // GAP: item frame rotation (GAPS.md, "Riding, passengers and hanging entities").
    }

    fn snap_to(&mut self, uuid: u128, pos: Pos, yaw: f32, pitch: f32) {
        if let Some(entity) = self
            .entity_id(uuid)
            .and_then(|id| self.game.native_entity(id))
        {
            let _ = self.game.native_entity_teleport_to(
                entity,
                [pos.0, pos.1, pos.2],
                Some([yaw, pitch]),
            );
        }
    }

    fn set_head_yaw(&mut self, uuid: u128, yaw: f32) {
        if let Some(entity) = self
            .entity_id(uuid)
            .and_then(|id| self.game.native_entity(id))
        {
            let _ = self.game.native_entity_set_head_yaw(entity, yaw);
        }
    }

    fn set_yaw(&mut self, uuid: u128, yaw: f32) {
        let pitch = self.pitch(uuid);
        if let Some(entity) = self
            .entity_id(uuid)
            .and_then(|id| self.game.native_entity(id))
        {
            self.game.native_entity_set_rotation(entity, yaw, pitch);
        }
    }

    fn set_pitch(&mut self, uuid: u128, pitch: f32) {
        let yaw = self.yaw(uuid);
        if let Some(entity) = self
            .entity_id(uuid)
            .and_then(|id| self.game.native_entity(id))
        {
            self.game.native_entity_set_rotation(entity, yaw, pitch);
        }
    }

    fn set_pos_raw(&mut self, uuid: u128, pos: Pos) {
        if let Some((dim, id)) = self.entities.get(&uuid).cloned() {
            // Keep the entity's rotation: the engine moves and turns in one call.
            let (yaw, pitch) = match self.entity_data(uuid) {
                Some(Tag::Compound(c)) => match c.get(&JavaString::from("Rotation")) {
                    Some(Tag::List(l)) if l.len() == 2 => match (&l[0], &l[1]) {
                        (Tag::Float(y), Tag::Float(p)) => (f32::from_bits(*y), f32::from_bits(*p)),
                        _ => (0.0, 0.0),
                    },
                    _ => (0.0, 0.0),
                },
                _ => (0.0, 0.0),
            };
            self.game
                .native_move_entity(&dim, id, [pos.0, pos.1, pos.2], yaw, pitch);
        }
    }

    fn save_without_id(&mut self, uuid: u128) -> Nbt {
        // Empty when the engine has no data for the entity; the caller then merges into nothing.
        match self.entity_data(uuid) {
            Some(tag) => tag_to_core(&tag),
            None => Nbt::Compound(BTreeMap::new()),
        }
    }

    fn load_nbt(&mut self, uuid: u128, tag: &Nbt) {
        let Tag::Compound(compound) = core_to_tag(tag) else {
            return;
        };
        if let Some(entity) = self
            .entity_id(uuid)
            .and_then(|id| self.game.native_entity(id))
        {
            let _ = self.game.native_entity_merge_nbt(entity, compound);
        }
    }

    fn eject_passengers(&mut self, _uuid: u128) {
        // GAP: riding and passengers (GAPS.md, "Riding, passengers and hanging entities").
    }

    fn start_riding(&mut self, _passenger: u128, _vehicle: u128) {
        // GAP: riding and passengers (GAPS.md, "Riding, passengers and hanging entities").
    }

    fn stop_riding(&mut self, _passenger: u128) {
        // GAP: riding and passengers (GAPS.md, "Riding, passengers and hanging entities").
    }

    fn add_fresh_entity_with_passengers(&mut self, _dim: &str, _uuid: u128) -> bool {
        // GAP: spawn with a client-chosen uuid (GAPS.md, "Entity spawn with the client's uuid"). Nothing is added.
        false
    }

    fn remove(&mut self, uuid: u128) {
        if let Some((dim, id)) = self.entities.remove(&uuid) {
            self.game.native_remove_entity(&dim, id);
        }
    }
}

impl AxiomHost<'_, '_> {
    /// Yaw and pitch of an indexed entity, `[0, 0]` where there is none.
    fn rotation_of(&self, uuid: u128) -> [f32; 2] {
        self.entity_id(uuid)
            .and_then(|id| self.game.native_entity(id))
            .and_then(|entity| self.game.native_entity_rotation(entity))
            .unwrap_or([0.0; 2])
    }
}

impl RequestHost for AxiomHost<'_, '_> {
    fn is_axiom_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }

    fn has_permission(&self, player: u128, node: &str) -> bool {
        self.game.native_has_permission(player, node, 1)
    }

    fn is_mismatched_data_version(&self, _player: u128) -> bool {
        // Horizon serves one data version: no client is on another.
        false
    }

    fn is_player_online(&self, player: u128) -> bool {
        self.players().iter().any(|p| p.uuid == player)
    }

    fn player_dimension(&self, player: u128) -> Option<String> {
        self.dimension_of(player)
    }

    fn player_block_pos(&self, player: u128) -> (i32, i32, i32) {
        self.player_block_pos_of(player)
    }

    fn can_modify_world(&self, _player: u128, _dimension: &str) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn can_place_block(&self, _player: u128, _dimension: &str, _x: i32, _y: i32, _z: i32) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
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

    fn build_height(&self, _dimension: &str) -> (i32, i32) {
        self.height()
    }

    fn block_state_global_bits(&self) -> u8 {
        self.global_bits_value()
    }

    fn request_chunk(&mut self, dimension: &str, chunk_x: i32, chunk_z: i32) {
        // GAP: release of a request's chunk tickets (GAPS.md, "Chunk loading for far edits, with release").
        self.game
            .native_force_chunk_in(CHUNK_TICKET_OWNER, dimension, [chunk_x, chunk_z], true);
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
        self.game
            .native_block_entity_tag(dimension, [x, y, z])
            .map(|tag| tag_to_core(&tag))
    }

    fn section_block_entities(
        &mut self,
        dimension: &str,
        chunk_x: i32,
        section_y: i32,
        chunk_z: i32,
    ) -> Vec<(i32, i32, i32, Nbt)> {
        // The engine lists the level's block entities; the ones of this section are read one by one.
        let positions: Vec<[i32; 3]> = self
            .game
            .native_level_block_entities(dimension)
            .into_iter()
            .map(|(pos, _)| pos)
            .filter(|[x, y, z]| x >> 4 == chunk_x && y >> 4 == section_y && z >> 4 == chunk_z)
            .collect();
        positions
            .into_iter()
            .filter_map(|[x, y, z]| {
                let tag = self.game.native_block_entity_tag(dimension, [x, y, z])?;
                Some((x, y, z, tag_to_core(&tag)))
            })
            .collect()
    }

    fn entity_info(&self, dimension: &str, uuid: u128) -> Option<EntityInfo> {
        let (dim, id) = self.entities.get(&uuid)?;
        if dim.as_str() != dimension {
            return None;
        }
        let entity = self.game.native_entity(*id)?;
        let entity_type = self.game.native_entity_kind(entity)?;
        let [x, y, z] = self
            .game
            .native_entity_position(entity)?
            .map(|v| v.floor() as i32);
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

impl AxiomHost<'_, '_> {
    /// `Entity.saveWithoutId` of an indexed entity, as the engine reads it (`native_entity_nbt`).
    fn entity_data(&mut self, uuid: u128) -> Option<Tag> {
        let id = self.entity_id(uuid)?;
        let entity = self.game.native_entity(id)?;
        self.game.native_entity_nbt(entity).map(Tag::Compound)
    }

    /// `Entity.save` of an indexed entity in `dimension`: its data with its `id`, for copies and entity data
    /// requests. The engine's data has no id, so the id comes from the entity's kind.
    fn saved_entity(&mut self, dimension: &str, uuid: u128) -> Option<Nbt> {
        let (dim, id) = self.entities.get(&uuid)?.clone();
        if dim.as_str() != dimension {
            return None;
        }
        let entity = self.game.native_entity(id)?;
        let kind = self.game.native_entity_kind(entity)?;
        let mut data = self.game.native_entity_nbt(entity)?;
        data.insert("id".into(), Tag::String(JavaString::from(kind.as_str())));
        Some(tag_to_core(&Tag::Compound(data)))
    }

    fn player_block_pos_of(&self, player: u128) -> (i32, i32, i32) {
        let [x, y, z] = self.position_of(player).unwrap_or([0.0; 3]);
        (x.floor() as i32, y.floor() as i32, z.floor() as i32)
    }
}

impl BlueprintHost for AxiomHost<'_, '_> {
    fn data_version(&self) -> i32 {
        horizon_world::DATA_VERSION
    }

    fn default_state(&self, block: &str) -> u32 {
        self.default_state_of(block).unwrap_or(0)
    }

    fn global_state_bits(&self) -> u8 {
        self.global_bits_value()
    }

    fn state_to_nbt(&self, state: u32) -> Nbt {
        let Some(s) = self.game.pack.assets.registry.state(BlockStateId(state)) else {
            return Nbt::End;
        };
        let mut map = BTreeMap::new();
        map.insert("Name".to_owned(), Nbt::String(s.name.to_string()));
        if !s.properties.is_empty() {
            let props: BTreeMap<String, Nbt> = s
                .properties
                .iter()
                .map(|(k, v)| (k.to_string(), Nbt::String(v.to_string())))
                .collect();
            map.insert("Properties".to_owned(), Nbt::Compound(props));
        }
        Nbt::Compound(map)
    }

    fn state_from_nbt(&self, nbt: &Nbt) -> Option<u32> {
        let Nbt::Compound(map) = nbt else {
            return None;
        };
        let Some(Nbt::String(name)) = map.get("Name") else {
            return None;
        };
        let mut properties = BTreeMap::<String, String>::new();
        if let Some(Nbt::Compound(props)) = map.get("Properties") {
            for (k, v) in props {
                if let Nbt::String(v) = v {
                    properties.insert(k.clone(), v.clone());
                }
            }
        }
        let descriptor = horizon_world::chunk::BlockState {
            name: name.clone(),
            properties: properties.into(),
        };
        self.game
            .pack
            .assets
            .registry
            .resolve(&descriptor)
            .ok()
            .map(|id| id.0)
    }

    fn block_entity_type_of(&self, _state: u32) -> Option<String> {
        // GAP: block entity types per state (GAPS.md, "Block-state properties").
        None
    }

    fn block_entity_accepts(&self, _type_id: &str, _state: u32) -> bool {
        // GAP: block entity types per state (GAPS.md, "Block-state properties").
        false
    }

    fn upgrade_block_state(&self, _nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        // GAP: data fixers for old blueprints (GAPS.md, "Block entity and entity data fixers for old blueprints"). Refused, not guessed.
        Err("no block state data fixer on Horizon yet".to_owned())
    }

    fn upgrade_block_entity(&self, _nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        // GAP: data fixers for old blueprints (GAPS.md, "Block entity and entity data fixers for old blueprints"). Refused, not guessed.
        Err("no block entity data fixer on Horizon yet".to_owned())
    }

    fn upgrade_entity(&self, _nbt: Nbt, _from_version: i32) -> Result<Nbt, String> {
        // GAP: data fixers for old blueprints (GAPS.md, "Block entity and entity data fixers for old blueprints"). Refused, not guessed.
        Err("no entity data fixer on Horizon yet".to_owned())
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
        self.active.contains(&player) && self.game.native_has_permission(player, permission, 1)
    }

    fn is_mismatched_data_version(&self, _player: u128) -> bool {
        false
    }

    fn game_version_name(&self) -> String {
        horizon_core::version::VERSION_ID.to_owned()
    }

    fn send_message(&mut self, player: u128, text: &str) {
        let _ = self.game.native_tell(player, Text::literal(text));
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

impl AnnotationHost for AxiomHost<'_, '_> {
    fn allow_annotations(&self) -> bool {
        self.config.allow_annotations
    }

    fn send_markers(&self) -> bool {
        self.config.send_markers
    }

    fn world_folder(&self, dim: &str) -> Option<PathBuf> {
        // GAP: writable mod data folder (GAPS.md, "Writable mod data folder"). Kept inside the world for now.
        self.game.native_world_dir().map(|d| {
            d.join("axiom")
                .join("annotations")
                .join(dim.replace(':', "_"))
        })
    }

    fn player_dimension(&self, player: u128) -> Option<String> {
        self.dimension_of(player)
    }

    fn world_players(&self, dim: &str) -> Vec<u128> {
        self.players_in_dimension(dim)
    }

    fn can_use_axiom(&self, player: u128) -> bool {
        self.active.contains(&player)
    }

    fn has_permission(&self, player: u128, node: &str) -> bool {
        self.game.native_has_permission(player, node, 1)
    }

    fn can_modify_world(&self, _player: u128, _dim: &str) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn markers(&self, _dim: &str) -> Vec<MarkerEntity> {
        // GAP: marker listing (GAPS.md, "Entity NBT reload and listing for display, marker and interaction kinds").
        Vec::new()
    }

    fn marker_custom_data(&self, _dim: &str, _uuid: u128) -> Option<Nbt> {
        // GAP: marker custom data (GAPS.md, "Entity data by numeric id in any loaded level").
        None
    }

    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.send_raw(player, channel, &data);
    }
}

impl CustomHost for AxiomHost<'_, '_> {
    fn axiom_players(&self) -> Vec<u128> {
        self.active.iter().copied().collect()
    }

    fn protocol_mismatch(&self, _player: u128) -> bool {
        false
    }

    fn block_state_string(&self, state: u32) -> String {
        self.palette_name(state)
            .unwrap_or_else(|| "minecraft:air".to_owned())
    }

    fn send_payload(&mut self, player: u128, channel: &str, data: &[u8]) {
        self.send_raw(player, channel, data);
    }
}

impl DebugHost for AxiomHost<'_, '_> {
    fn is_op(&self, player: u128) -> bool {
        self.game
            .native_player_permission(player)
            .is_some_and(|level| level > 0)
    }

    fn has_permission(&self, player: u128, node: &str) -> bool {
        self.game.native_has_permission(player, node, 1)
    }

    fn can_use_axiom(&self, player: u128) -> bool {
        self.active.contains(&player)
    }

    fn is_mismatched_data_version(&self, _player: u128) -> bool {
        false
    }

    fn can_modify_world(&self, _player: u128) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn is_client_listening(&self, player: u128, channel: &str) -> bool {
        self.game
            .native_player_channels(player)
            .iter()
            .any(|c| c == channel)
    }

    fn restrictions(&self, _player: u128) -> Option<String> {
        // GAP: the restrictions of a player are held by the core state, not reachable from the host (GAPS.md, "Debug text of the restrictions").
        None
    }

    fn can_break_block(&self, _player: u128, _provider: Option<Provider>) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn can_place_block(&self, _player: u128, _provider: Option<Provider>) -> bool {
        // GAP: region and plot build checks (GAPS.md, "Region and plot build check"). Denies every edit.
        false
    }

    fn is_plot_world(&self, _player: u128) -> bool {
        false
    }

    fn current_editable_plot(&self, _player: u128) -> String {
        "null".to_owned()
    }
}
