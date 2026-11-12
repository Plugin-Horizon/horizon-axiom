// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): tests for the classes ported in this module.
// Unit tests for the world module, against a recording `TestHost`. Compiled only under `cargo test`.

use std::collections::{BTreeMap, BTreeSet};

use crate::buf::{Reader, Writer};

use super::packets::{clamp_fly_speed, set_fly_speed, set_gamemode, set_world_time, teleport};
use super::properties::{Category, PropValue, Widget, WorldPropertiesRegistry};
use super::{
    parse_identifier, GameMode, Location, MarkerEntry, WorldError, WorldHost, WorldState,
    CH_ACK_WORLD_PROPERTIES, CH_MARKER_DATA, CH_SET_WORLD_PROPERTY, PERM_PLAYER_GAMEMODE_CREATIVE,
    PERM_PLAYER_GAMEMODE_SURVIVAL, PERM_PLAYER_SET_NO_PHYSICAL_TRIGGER, PERM_PLAYER_SPEED,
    PERM_PLAYER_TELEPORT, PERM_WORLD_PROPERTY, PERM_WORLD_TIME,
};

const OVERWORLD: &str = "minecraft:overworld";
const NETHER: &str = "minecraft:the_nether";
const PLAYER: u128 = 0x1234_5678_9abc_def0_1122_3344_5566_7788;

#[derive(Clone, Default)]
struct TestPlayer {
    dim: String,
    active: bool,
    op: bool,
    perms: BTreeSet<String>,
}

#[derive(Default)]
struct TestHost {
    worlds: Vec<String>,
    players: BTreeMap<u128, TestPlayer>,
    allow_cross_world: bool,
    can_modify: bool,
    plot: bool,
    cancel_events: bool,
    paused: bool,
    loaded: BTreeSet<(String, i32, i32)>,
    markers: BTreeMap<String, Vec<MarkerEntry>>,
    anti_xray: BTreeSet<u128>,
    payloads: Vec<(u128, String, Vec<u8>)>,
    /// Human-readable log of world and player actions, asserted by the tests.
    calls: Vec<String>,
}

impl TestHost {
    /// Two worlds, one active player (PLAYER) in the overworld with the given permission nodes.
    fn with_player(perms: &[&str]) -> Self {
        let mut host = TestHost {
            worlds: vec![OVERWORLD.to_string(), NETHER.to_string()],
            can_modify: true,
            ..Default::default()
        };
        host.players.insert(
            PLAYER,
            TestPlayer {
                dim: OVERWORLD.to_string(),
                active: true,
                op: false,
                perms: perms.iter().map(|p| p.to_string()).collect(),
            },
        );
        host
    }

    fn payloads_on(&self, channel: &str) -> Vec<(u128, Vec<u8>)> {
        self.payloads
            .iter()
            .filter(|(_, c, _)| c == channel)
            .map(|(p, _, d)| (*p, d.clone()))
            .collect()
    }
}

impl WorldHost for TestHost {
    fn world_names(&self) -> Vec<String> {
        self.worlds.clone()
    }

    fn world_exists(&self, dim: &str) -> bool {
        self.worlds.iter().any(|w| w == dim)
    }

    fn player_dimension(&self, player: u128) -> Option<String> {
        self.players.get(&player).map(|p| p.dim.clone())
    }

    fn players_in_dimension(&self, dim: &str) -> Vec<u128> {
        self.players
            .iter()
            .filter(|(_, p)| p.dim == dim)
            .map(|(id, _)| *id)
            .collect()
    }

    fn is_axiom_active(&self, player: u128) -> bool {
        self.players.get(&player).is_some_and(|p| p.active)
    }

    fn has_permission(&self, player: u128, node: &str) -> bool {
        self.players
            .get(&player)
            .is_some_and(|p| p.op || p.perms.contains(node))
    }

    fn can_modify_world(&self, _player: u128, _dim: &str) -> bool {
        self.can_modify
    }

    fn allow_teleport_between_worlds(&self) -> bool {
        self.allow_cross_world
    }

    fn is_plot_world(&self, _dim: &str) -> bool {
        self.plot
    }

    fn set_day_time(&mut self, dim: &str, time: i32) {
        self.calls.push(format!("day_time {dim} {time}"));
    }

    fn set_time_frozen(&mut self, dim: &str, frozen: bool) {
        self.calls.push(format!("frozen {dim} {frozen}"));
    }

    fn weather_paused(&self, _dim: &str) -> bool {
        self.paused
    }

    fn set_weather_paused(&mut self, dim: &str, paused: bool) {
        self.paused = paused;
        self.calls.push(format!("paused {dim} {paused}"));
    }

    fn set_weather(
        &mut self,
        dim: &str,
        clear_time: i32,
        rain_time: i32,
        raining: bool,
        thundering: bool,
    ) {
        self.calls.push(format!(
            "weather {dim} {clear_time} {rain_time} {raining} {thundering}"
        ));
    }

    /// Deterministic: always the lower bound.
    fn random_int(&mut self, min: i32, _max: i32) -> i32 {
        min
    }

    fn set_game_mode(&mut self, player: u128, mode: GameMode) {
        self.calls.push(format!("gamemode {player} {mode:?}"));
    }

    fn set_fly_speed(&mut self, player: u128, speed: f32) {
        self.calls.push(format!("fly {player} {speed}"));
    }

    fn teleport_player(&mut self, player: u128, dim: &str, at: Location) {
        self.calls.push(format!(
            "teleport {player} {dim} {} {} {}",
            at.x, at.y, at.z
        ));
    }

    fn is_chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool {
        self.loaded.contains(&(dim.to_string(), cx, cz))
    }

    fn players_tracking_chunk(&self, dim: &str, _cx: i32, _cz: i32) -> Vec<u128> {
        self.players_in_dimension(dim)
    }

    fn send_chunk_with_light(&mut self, _players: &[u128], dim: &str, cx: i32, cz: i32) {
        self.calls.push(format!("chunk {dim} {cx} {cz}"));
    }

    fn relight_chunks(&mut self, dim: &str, chunks: &BTreeSet<(i32, i32)>) {
        self.calls.push(format!("relight {dim} {chunks:?}"));
    }

    fn marker_snapshot(&mut self, dim: &str) -> Vec<MarkerEntry> {
        self.markers.get(dim).cloned().unwrap_or_default()
    }

    fn anti_xray_enabled(&self, player: u128) -> bool {
        self.anti_xray.contains(&player)
    }

    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        self.payloads.push((player, channel.to_string(), data));
    }

    fn send_warning(&mut self, player: u128, text: &str) {
        self.calls.push(format!("warning {player} {text}"));
    }

    fn send_chat_to_dimension(&mut self, dim: &str, text: &str) {
        self.calls.push(format!("chat {dim} {text}"));
    }

    fn on_time_change(&mut self, _player: u128, _time: Option<i32>, _freeze: Option<bool>) -> bool {
        !self.cancel_events
    }

    fn on_game_mode_change(&mut self, _player: u128, _mode: GameMode) -> bool {
        !self.cancel_events
    }

    fn on_fly_speed_change(&mut self, _player: u128, _speed: f32) -> bool {
        !self.cancel_events
    }

    fn on_unknown_teleport(&mut self, _player: u128, _dim: &str, _at: Location) -> bool {
        !self.cancel_events
    }

    fn on_teleport(&mut self, _player: u128, _dim: &str, _at: Location) -> bool {
        !self.cancel_events
    }
}

fn payload(build: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    build(&mut w);
    w.into_bytes()
}

fn time_packet(dim: &str, time: Option<i32>, freeze: Option<bool>) -> Vec<u8> {
    payload(|w| {
        w.write_identifier(dim);
        w.write_bool(time.is_some());
        if let Some(t) = time {
            w.write_i32(t);
        }
        w.write_bool(freeze.is_some());
        if let Some(f) = freeze {
            w.write_bool(f);
        }
    })
}

fn set_property_packet(id: &str, type_id: i32, value: &[u8], update_id: i32) -> Vec<u8> {
    payload(|w| {
        w.write_identifier(id);
        w.write_varint(type_id);
        w.write_byte_array(value);
        w.write_varint(update_id);
    })
}

// --- identifiers and small pure functions ---

#[test]
fn identifiers_default_to_minecraft_namespace() {
    assert_eq!(parse_identifier("time").unwrap(), "minecraft:time");
    assert_eq!(
        parse_identifier("axiom:pause_weather").unwrap(),
        "axiom:pause_weather"
    );
    assert!(parse_identifier("Bad:Name").is_err());
    assert!(parse_identifier("a:b:c").is_err());
    assert!(parse_identifier("").is_err());
}

#[test]
fn fly_speed_is_clamped_to_unit_range() {
    assert_eq!(clamp_fly_speed(5.0), 1.0);
    assert_eq!(clamp_fly_speed(-3.0), -1.0);
    assert_eq!(clamp_fly_speed(0.5), 0.5);
}

#[test]
fn unknown_game_mode_id_falls_back_to_survival() {
    assert_eq!(GameMode::from_id(9), GameMode::Survival);
    assert_eq!(GameMode::from_id(-1), GameMode::Survival);
    assert_eq!(GameMode::from_id(3), GameMode::Spectator);
}

// --- codecs ---

#[test]
fn slider_widget_encodes_kind_and_range() {
    let mut w = Writer::new();
    Widget::Slider { min: 0, max: 8 }.write(&mut w);
    assert_eq!(w.into_bytes(), vec![1, 0, 0, 0, 0, 0, 0, 0, 8]);
}

#[test]
fn registration_payload_lists_built_in_time_category_first() {
    let host = TestHost::with_player(&[]);
    let registry = WorldPropertiesRegistry::new(&host, OVERWORLD);
    let bytes = registry.encode_registration();

    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_varint().unwrap(), 2, "two built-in categories");
    assert_eq!(
        r.read_string(32767).unwrap(),
        "axiom.editorui.window.world_properties.time"
    );
    assert!(r.read_bool().unwrap());
    assert_eq!(r.read_varint().unwrap(), 1);
    assert_eq!(r.read_identifier().unwrap(), "axiom:time");
}

#[test]
fn integer_values_use_varint_bytes() {
    // Java writeVarInt(-1) is five bytes.
    let bytes = super::properties::DataType::Integer.serialize(&PropValue::Int(-1));
    assert_eq!(bytes, vec![0xff, 0xff, 0xff, 0xff, 0x0f]);
}

// --- time ---

#[test]
fn time_packet_applies_when_permitted() {
    let mut host = TestHost::with_player(&[PERM_WORLD_TIME]);
    set_world_time(
        &mut host,
        PLAYER,
        &time_packet(OVERWORLD, Some(6000), Some(true)),
    )
    .unwrap();
    assert_eq!(
        host.calls,
        vec![
            "day_time minecraft:overworld 6000",
            "frozen minecraft:overworld true"
        ]
    );
}

#[test]
fn time_packet_without_permission_does_nothing() {
    let mut host = TestHost::with_player(&[]);
    set_world_time(&mut host, PLAYER, &time_packet(OVERWORLD, Some(6000), None)).unwrap();
    assert!(host.calls.is_empty());
}

#[test]
fn time_packet_for_other_dimension_or_empty_is_ignored() {
    let mut host = TestHost::with_player(&[PERM_WORLD_TIME]);
    set_world_time(&mut host, PLAYER, &time_packet(NETHER, Some(6000), None)).unwrap();
    set_world_time(&mut host, PLAYER, &time_packet(OVERWORLD, None, None)).unwrap();
    assert!(host.calls.is_empty());
}

#[test]
fn time_packet_cancelled_by_event() {
    let mut host = TestHost::with_player(&[PERM_WORLD_TIME]);
    host.cancel_events = true;
    set_world_time(&mut host, PLAYER, &time_packet(OVERWORLD, Some(1), None)).unwrap();
    assert!(host.calls.is_empty());
}

// --- game mode and fly speed ---

#[test]
fn game_mode_needs_the_matching_node() {
    let mut host = TestHost::with_player(&[PERM_PLAYER_GAMEMODE_SURVIVAL]);
    set_gamemode(&mut host, PLAYER, &[1]).unwrap(); // creative, not granted
    assert!(host.calls.is_empty());
    set_gamemode(&mut host, PLAYER, &[0]).unwrap(); // survival, granted
    assert_eq!(host.calls, vec![format!("gamemode {PLAYER} Survival")]);
}

#[test]
fn game_mode_truncated_packet_is_an_error() {
    let mut host = TestHost::with_player(&[PERM_PLAYER_GAMEMODE_CREATIVE]);
    assert!(matches!(
        set_gamemode(&mut host, PLAYER, &[]),
        Err(WorldError::Buf(_))
    ));
}

#[test]
fn fly_speed_is_clamped_before_the_event() {
    let mut host = TestHost::with_player(&[PERM_PLAYER_SPEED]);
    set_fly_speed(&mut host, PLAYER, &5.0f32.to_be_bytes()).unwrap();
    assert_eq!(host.calls, vec![format!("fly {PLAYER} 1")]);
}

// --- teleport ---

fn teleport_packet(dim: &str, x: f64) -> Vec<u8> {
    payload(|w| {
        w.write_identifier(dim);
        w.write_f64(x);
        w.write_f64(64.0);
        w.write_f64(0.0);
        w.write_f32(90.0);
        w.write_f32(0.0);
    })
}

#[test]
fn teleport_across_worlds_is_blocked_when_disallowed() {
    let mut host = TestHost::with_player(&[PERM_PLAYER_TELEPORT]);
    teleport(&mut host, PLAYER, &teleport_packet(NETHER, 1.0)).unwrap();
    assert!(host.calls.is_empty());
}

#[test]
fn teleport_to_loaded_world_when_allowed() {
    let mut host = TestHost::with_player(&[PERM_PLAYER_TELEPORT]);
    host.allow_cross_world = true;
    teleport(&mut host, PLAYER, &teleport_packet(NETHER, 1.0)).unwrap();
    assert_eq!(
        host.calls,
        vec![format!("teleport {PLAYER} {NETHER} 1 64 0")]
    );
}

#[test]
fn teleport_to_unknown_world_does_nothing() {
    let mut host = TestHost::with_player(&[PERM_PLAYER_TELEPORT]);
    host.allow_cross_world = true;
    teleport(&mut host, PLAYER, &teleport_packet("minecraft:end", 1.0)).unwrap();
    assert!(host.calls.is_empty());
}

// --- no physical trigger ---

#[test]
fn no_physical_trigger_toggles_per_player() {
    let mut host = TestHost::with_player(&[PERM_PLAYER_SET_NO_PHYSICAL_TRIGGER]);
    let mut state = WorldState::new();
    state
        .on_set_no_physical_trigger(&mut host, PLAYER, &[1])
        .unwrap();
    assert!(state.is_no_physical_trigger(PLAYER));
    state
        .on_set_no_physical_trigger(&mut host, PLAYER, &[0])
        .unwrap();
    assert!(!state.is_no_physical_trigger(PLAYER));
}

// --- world properties ---

#[test]
fn set_world_property_unknown_id_only_acks() {
    let mut host = TestHost::with_player(&[PERM_WORLD_PROPERTY]);
    let mut state = WorldState::new();
    let data = set_property_packet("axiom:nope", 0, &[1], 7);
    state
        .on_set_world_property(&mut host, PLAYER, &data)
        .unwrap();
    let acks = host.payloads_on(CH_ACK_WORLD_PROPERTIES);
    assert_eq!(acks, vec![(PLAYER, vec![7])]);
    assert!(host.payloads_on(CH_SET_WORLD_PROPERTY).is_empty());
}

#[test]
fn set_world_property_without_permission_sends_nothing_not_even_an_ack() {
    let mut host = TestHost::with_player(&[]);
    let mut state = WorldState::new();
    state
        .on_set_world_property(
            &mut host,
            PLAYER,
            &set_property_packet("axiom:pause_weather", 0, &[1], 3),
        )
        .unwrap();
    assert!(host.payloads.is_empty());
    assert!(host.calls.is_empty());
}

#[test]
fn checkbox_update_without_sync_changes_world_and_acks() {
    let mut host = TestHost::with_player(&[PERM_WORLD_PROPERTY]);
    let mut state = WorldState::new();
    state
        .on_set_world_property(
            &mut host,
            PLAYER,
            &set_property_packet("axiom:pause_weather", 0, &[1], 3),
        )
        .unwrap();
    assert_eq!(host.calls, vec!["paused minecraft:overworld true"]);
    // UPDATE_WITHOUT_SYNC: no set_world_property broadcast.
    assert!(host.payloads_on(CH_SET_WORLD_PROPERTY).is_empty());
    assert_eq!(
        host.payloads_on(CH_ACK_WORLD_PROPERTIES),
        vec![(PLAYER, vec![3])]
    );
}

#[test]
fn type_mismatch_is_ignored_but_acked() {
    let mut host = TestHost::with_player(&[PERM_WORLD_PROPERTY]);
    let mut state = WorldState::new();
    state
        .on_set_world_property(
            &mut host,
            PLAYER,
            &set_property_packet("axiom:pause_weather", 1, &[1], 4),
        )
        .unwrap();
    assert!(host.calls.is_empty());
    assert_eq!(
        host.payloads_on(CH_ACK_WORLD_PROPERTIES),
        vec![(PLAYER, vec![4])]
    );
}

#[test]
fn weather_type_index_one_starts_rain() {
    let mut host = TestHost::with_player(&[PERM_WORLD_PROPERTY]);
    let mut state = WorldState::new();
    // Value 1 as a varint (one byte).
    state
        .on_set_world_property(
            &mut host,
            PLAYER,
            &set_property_packet("axiom:weather_type", 1, &[1], 0),
        )
        .unwrap();
    assert_eq!(
        host.calls,
        vec!["weather minecraft:overworld 0 12000 true false"]
    );
}

#[test]
fn server_side_value_change_syncs_active_players() {
    let mut host = TestHost::with_player(&[]);
    let mut state = WorldState::new();
    assert!(state.registry_or_create(&mut host, OVERWORLD).is_some());
    assert!(state.set_property_value(
        &mut host,
        OVERWORLD,
        "axiom:pause_weather",
        PropValue::Bool(true)
    ));
    let syncs = host.payloads_on(CH_SET_WORLD_PROPERTY);
    assert_eq!(syncs.len(), 1);
    assert_eq!(syncs[0].0, PLAYER);
    // Same value again: no sync.
    assert!(state.set_property_value(
        &mut host,
        OVERWORLD,
        "axiom:pause_weather",
        PropValue::Bool(true)
    ));
    assert_eq!(host.payloads_on(CH_SET_WORLD_PROPERTY).len(), 1);
}

#[test]
fn duplicate_property_ids_are_rejected_without_partial_add() {
    let host = TestHost::with_player(&[]);
    let mut registry = WorldPropertiesRegistry::new(&host, OVERWORLD);
    let props = vec![super::properties::WorldProperty::new(
        "axiom:pause_weather",
        "dup",
        false,
        Widget::Checkbox,
        |_h, _d| PropValue::Bool(false),
        |_h, _p, _d, _v| super::PropertyUpdateResult::UpdateWithoutSync,
    )
    .unwrap()];
    let cat = Category {
        name: "dup".to_string(),
        localize_name: false,
    };
    assert!(matches!(
        registry.add_category(&host, OVERWORLD, cat, props),
        Err(WorldError::DuplicateProperty)
    ));
    assert!(registry.get("axiom:pause_weather").is_some());
}

// --- markers and chunks ---

#[test]
fn marker_changes_then_removals_are_sent_once() {
    let mut host = TestHost::with_player(&[]);
    host.markers.insert(
        OVERWORLD.to_string(),
        vec![MarkerEntry {
            uuid: 5,
            encoded: vec![1, 2, 3],
        }],
    );
    let mut state = WorldState::new();

    state.tick(&mut host, true, 0, 0);
    let first = host.payloads_on(CH_MARKER_DATA);
    assert_eq!(first.len(), 1);
    // changed: count 1, the entry, removed: count 0
    assert_eq!(first[0].1, vec![1, 1, 2, 3, 0]);

    // Unchanged: nothing new.
    state.tick(&mut host, true, 0, 0);
    assert_eq!(host.payloads_on(CH_MARKER_DATA).len(), 1);

    // Marker gone: removal with the uuid (16 bytes, big-endian).
    host.markers.clear();
    state.tick(&mut host, true, 0, 0);
    let after = host.payloads_on(CH_MARKER_DATA);
    assert_eq!(after.len(), 2);
    let mut expected = vec![0u8, 1];
    expected.extend_from_slice(&5u128.to_be_bytes());
    assert_eq!(after[1].1, expected);
}

#[test]
fn join_sends_known_markers_and_anti_xray_warning() {
    let mut host = TestHost::with_player(&[]);
    host.markers.insert(
        OVERWORLD.to_string(),
        vec![MarkerEntry {
            uuid: 9,
            encoded: vec![7],
        }],
    );
    let mut state = WorldState::new();
    state.tick(&mut host, true, 0, 0);
    host.payloads.clear();

    host.anti_xray.insert(PLAYER);
    state.on_player_join(&mut host, PLAYER, OVERWORLD);
    let markers = host.payloads_on(CH_MARKER_DATA);
    assert_eq!(markers, vec![(PLAYER, vec![1, 7, 0])]);
    assert!(host.calls.iter().any(|c| c.starts_with("warning ")));
}

#[test]
fn chunk_sends_respect_the_per_tick_budget() {
    let mut host = TestHost::with_player(&[]);
    for cz in 0..3 {
        host.loaded.insert((OVERWORLD.to_string(), 0, cz));
    }
    let mut state = WorldState::new();
    for cz in 0..3 {
        state.send_chunk(OVERWORLD, 0, cz);
    }

    state.tick(&mut host, false, 0, 2);
    assert_eq!(
        host.calls,
        vec![
            "chunk minecraft:overworld 0 0",
            "chunk minecraft:overworld 0 1"
        ]
    );

    state.tick(&mut host, false, 0, 2);
    assert_eq!(host.calls.len(), 3);
    assert_eq!(host.calls[2], "chunk minecraft:overworld 0 2");
}

#[test]
fn unloaded_chunks_stay_queued_and_do_not_use_budget() {
    let mut host = TestHost::with_player(&[]);
    let mut state = WorldState::new();
    state.send_chunk(OVERWORLD, 4, 4);
    state.tick(&mut host, false, 0, 1);
    assert!(host.calls.is_empty());

    host.loaded.insert((OVERWORLD.to_string(), 4, 4));
    state.tick(&mut host, false, 0, 1);
    assert_eq!(host.calls, vec!["chunk minecraft:overworld 4 4"]);
}

#[test]
fn relight_budget_drains_in_order() {
    let mut host = TestHost::with_player(&[]);
    let mut state = WorldState::new();
    for cz in 0..3 {
        state.light_chunk(OVERWORLD, 0, cz);
    }
    state.tick(&mut host, false, 1, 0);
    state.tick(&mut host, false, 0, 0); // unlimited drains the rest
    assert_eq!(
        host.calls,
        vec![
            format!("relight minecraft:overworld {:?}", BTreeSet::from([(0, 0)])),
            format!(
                "relight minecraft:overworld {:?}",
                BTreeSet::from([(0, 1), (0, 2)])
            ),
        ]
    );
}

#[test]
fn forgetting_a_world_clears_its_registry() {
    let mut host = TestHost::with_player(&[]);
    let mut state = WorldState::new();
    assert!(state.registry_or_create(&mut host, OVERWORLD).is_some());
    state.forget_world(OVERWORLD);
    assert!(!state.set_property_value(
        &mut host,
        OVERWORLD,
        "axiom:pause_weather",
        PropValue::Bool(true)
    ));
}

#[test]
fn teleport_packet_location_is_read_in_order() {
    let mut host = TestHost::with_player(&[PERM_PLAYER_TELEPORT]);
    host.allow_cross_world = true;
    teleport(&mut host, PLAYER, &teleport_packet(OVERWORLD, 2.5)).unwrap();
    assert_eq!(
        host.calls,
        vec![format!("teleport {PLAYER} {OVERWORLD} 2.5 64 0")]
    );
}

#[test]
fn permission_nodes_are_leaf_nodes_of_the_axiom_tree() {
    assert_eq!(PERM_WORLD_TIME, "axiom.world.time");
    assert_eq!(PERM_PLAYER_TELEPORT, "axiom.player.teleport");
}
