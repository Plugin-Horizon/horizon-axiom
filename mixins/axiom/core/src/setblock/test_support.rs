//! Fake [`SetBlockHost`] for unit tests. Block states are plain ids: state 0 is air, and two states are in the
//! same block when they share `state >> 4`. Every write is recorded in `calls`.

use std::collections::{HashMap, HashSet};

use super::block_packet::{BlockHit, Hand};
use super::host::{BlockPos, ChunkLoadState, SetBlockHost};
use super::permission::SectionPermissionChecker;
use super::integration::BYPASS_REGION_CHECKS;

#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Ack(i32),
    SetBlock(BlockPos, u32, u32),
    RawSet(BlockPos, u32),
    RequestLoad(i32, i32),
    SendChunk(i32, i32),
    ApplyItem(BlockPos),
    SetPlacedBy(BlockPos, u32),
    TickFluid(BlockPos),
    TickBlock(BlockPos),
    PlaceEvent(usize),
}

pub struct FakeHost {
    pub dim: String,
    pub player_pos: BlockPos,
    pub active: bool,
    pub perms: HashSet<String>,
    pub bypass: bool,
    pub blocks: HashMap<(String, BlockPos), u32>,
    pub loaded: HashSet<(String, i32, i32)>,
    pub all_loaded: bool,
    pub chunk_states: HashMap<(String, i32, i32), ChunkLoadState>,
    pub max_dist: i32,
    pub section_range: (i32, i32),
    pub region: SectionPermissionChecker,
    pub region_ok: bool,
    pub interact_ok: bool,
    pub place_ok: bool,
    pub clicked: Option<BlockPos>,
    pub calls: Vec<Call>,
    pub messages: Vec<String>,
    pub kicks: Vec<(u128, String)>,
}

impl FakeHost {
    pub fn new(dim: &str) -> Self {
        FakeHost {
            dim: dim.to_string(),
            player_pos: (0, 64, 0),
            active: true,
            perms: HashSet::new(),
            bypass: false,
            blocks: HashMap::new(),
            loaded: HashSet::new(),
            all_loaded: true,
            chunk_states: HashMap::new(),
            max_dist: 256,
            section_range: (-4, 19),
            region: SectionPermissionChecker::All,
            region_ok: true,
            interact_ok: true,
            place_ok: true,
            clicked: None,
            calls: Vec::new(),
            messages: Vec::new(),
            kicks: Vec::new(),
        }
    }

    pub fn grant(&mut self, _player: u128, node: &str) {
        self.perms.insert(node.to_string());
    }

    pub fn set(&mut self, dim: &str, pos: BlockPos, state: u32) {
        self.blocks.insert((dim.to_string(), pos), state);
    }
}

impl SetBlockHost for FakeHost {
    fn player_dimension(&self, _player: u128) -> String {
        self.dim.clone()
    }
    fn player_block_pos(&self, _player: u128) -> BlockPos {
        self.player_pos
    }
    fn scoreboard_name(&self, _player: u128) -> String {
        "Tester".to_string()
    }
    fn is_axiom_active(&self, _player: u128) -> bool {
        self.active
    }
    fn has_axiom_permission(&self, _player: u128, node: &str) -> bool {
        self.perms.contains(node)
    }
    fn has_bukkit_permission(&self, _player: u128, node: &str) -> bool {
        self.bypass && node == BYPASS_REGION_CHECKS
    }
    fn can_use_game_master_blocks(&self, _player: u128) -> bool {
        false
    }
    fn allowed_dispatch_sends_per_second(&self, _player: u128) -> i64 {
        1000
    }
    fn kick(&mut self, player: u128, reason: &str) {
        self.kicks.push((player, reason.to_string()));
    }
    fn send_system_message(&mut self, _player: u128, text: &str, _red: bool) {
        self.messages.push(text.to_string());
    }
    fn broadcast_system_message(&mut self, text: &str) {
        self.messages.push(text.to_string());
    }
    fn world_allowed_by_config(&self, _dim: &str) -> bool {
        true
    }
    fn modify_world_event(&mut self, _player: u128, _dim: &str, cancelled: bool) -> bool {
        cancelled
    }
    fn max_chunk_load_distance(&self, _dim: &str) -> i32 {
        self.max_dist
    }
    fn block_state_from_wire(&self, _player: u128, wire_id: i32) -> Option<u32> {
        if wire_id >= 0 { Some(wire_id as u32) } else { None }
    }
    fn empty_state(&self) -> u32 {
        0
    }
    fn global_bits(&self) -> u8 {
        15
    }
    fn is_air(&self, state: u32) -> bool {
        state == 0
    }
    fn same_block(&self, a: u32, b: u32) -> bool {
        (a >> 4) == (b >> 4)
    }
    fn state_can_be_replaced(&self, state: u32) -> bool {
        state == 0
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
    fn light_properties_differ(&self, _old: u32, _new: u32) -> bool {
        false
    }
    fn block_entity_must_be_removed(&self, _old: u32, _new: u32) -> bool {
        false
    }
    fn is_game_master_block(&self, _dim: &str, _pos: BlockPos) -> bool {
        false
    }
    fn update_from_neighbour_shapes(&self, _dim: &str, _pos: BlockPos, state: u32) -> u32 {
        state ^ 1
    }
    fn is_block_loaded(&self, dim: &str, pos: BlockPos) -> bool {
        self.is_chunk_loaded(dim, pos.0 >> 4, pos.2 >> 4)
    }
    fn is_chunk_loaded(&self, dim: &str, cx: i32, cz: i32) -> bool {
        self.all_loaded || self.loaded.contains(&(dim.to_string(), cx, cz))
    }
    fn request_chunk_load(&mut self, _dim: &str, cx: i32, cz: i32) {
        self.calls.push(Call::RequestLoad(cx, cz));
    }
    fn release_chunk_load(&mut self, _dim: &str, _cx: i32, _cz: i32) {}

    fn chunk_load_state(&self, dim: &str, cx: i32, cz: i32) -> ChunkLoadState {
        self.chunk_states.get(&(dim.to_string(), cx, cz)).copied().unwrap_or(ChunkLoadState::Loaded)
    }
    fn section_y_range(&self, _dim: &str) -> (i32, i32) {
        self.section_range
    }
    fn section_has_only_air(&self, dim: &str, cx: i32, cy: i32, cz: i32) -> bool {
        !self.blocks.iter().any(|((d, p), s)| {
            d == dim && *s != 0 && p.0 >> 4 == cx && p.1 >> 4 == cy && p.2 >> 4 == cz
        })
    }
    fn block_state(&self, dim: &str, pos: BlockPos) -> u32 {
        self.blocks.get(&(dim.to_string(), pos)).copied().unwrap_or(0)
    }
    fn set_block_raw(&mut self, dim: &str, pos: BlockPos, state: u32) -> u32 {
        let old = self.block_state(dim, pos);
        self.blocks.insert((dim.to_string(), pos), state);
        self.calls.push(Call::RawSet(pos, state));
        old
    }
    fn update_heightmaps(&mut self, _dim: &str, _pos: BlockPos, _new_state: u32) {}
    fn mark_chunk_unsaved(&mut self, _dim: &str, _cx: i32, _cz: i32) {}
    fn mark_block_changed(&mut self, _dim: &str, _pos: BlockPos) {}
    fn check_block_light(&mut self, _dim: &str, _pos: BlockPos) {}
    fn update_section_emptiness(&mut self, _dim: &str, _cx: i32, _cy: i32, _cz: i32, _has_only_air: bool) {}
    fn update_poi(&mut self, _dim: &str, _pos: BlockPos, _old: u32, _new: u32) {}
    fn remove_block_entity(&mut self, _dim: &str, _pos: BlockPos) {}
    fn sync_block_entity(&mut self, _dim: &str, _pos: BlockPos, _state: u32) -> bool {
        false
    }
    fn load_block_entities(&mut self, _dim: &str, _entries: &[(BlockPos, Vec<u8>)]) {}
    fn send_chunk(&mut self, _dim: &str, cx: i32, cz: i32) {
        self.calls.push(Call::SendChunk(cx, cz));
    }
    fn light_chunk(&mut self, _dim: &str, _cx: i32, _cz: i32) {}
    fn biome_exists(&self, _key: &str) -> bool {
        true
    }
    fn set_biome_quart(&mut self, _dim: &str, _qx: i32, _qy: i32, _qz: i32, _key: &str) {}
    fn players_tracking_chunk(&self, _dim: &str, _cx: i32, _cz: i32) -> Vec<u128> {
        Vec::new()
    }
    fn send_chunk_biomes(&mut self, _player: u128, _dim: &str, _chunks: &[(i32, i32)]) {}
    fn region_can_break(&self, _player: u128, _dim: &str, _pos: BlockPos) -> bool {
        self.region_ok
    }
    fn region_can_place(&self, _player: u128, _dim: &str, _pos: BlockPos) -> bool {
        self.region_ok
    }
    fn region_section(&self, _player: u128, _dim: &str, _cx: i32, _cy: i32, _cz: i32) -> SectionPermissionChecker {
        self.region.clone()
    }
    fn ack_block_changes(&mut self, _player: u128, seq: i32) {
        self.calls.push(Call::Ack(seq));
    }
    fn fire_player_interact(&mut self, _player: u128, _breaking: bool, _hit: &BlockHit, _hand: Hand, _dim: &str) -> bool {
        self.interact_ok
    }
    fn fire_block_place(&mut self, _player: u128, _hand: Hand, _dim: &str, _against: BlockPos, placed: &[(BlockPos, u32)]) -> bool {
        self.calls.push(Call::PlaceEvent(placed.len()));
        self.place_ok
    }
    fn clicked_pos(&self, _player: u128, _hand: Hand, _dim: &str, hit: &BlockHit) -> BlockPos {
        self.clicked.unwrap_or(hit.pos)
    }
    fn apply_item_to_block_entity(&mut self, _player: u128, _hand: Hand, _dim: &str, pos: BlockPos) {
        self.calls.push(Call::ApplyItem(pos));
    }
    fn set_placed_by(&mut self, _player: u128, _hand: Hand, _dim: &str, pos: BlockPos, state: u32) {
        self.calls.push(Call::SetPlacedBy(pos, state));
    }
    fn set_block(&mut self, dim: &str, pos: BlockPos, state: u32, flags: u32) {
        self.calls.push(Call::SetBlock(pos, state, flags));
        self.blocks.insert((dim.to_string(), pos), state);
    }
    fn tick_fluid(&mut self, _dim: &str, pos: BlockPos, _state: u32) {
        self.calls.push(Call::TickFluid(pos));
    }
    fn tick_block(&mut self, _dim: &str, pos: BlockPos, _state: u32) {
        self.calls.push(Call::TickBlock(pos));
    }
}
