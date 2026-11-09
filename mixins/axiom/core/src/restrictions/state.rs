// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): AxiomPaper.java (tick, tickPlayer,
// tunnel-free handshake/enable/goodbye parts, dispatch-send budget, calculateRestrictions/calculatePermissions,
// canModifyWorld, sendInitialPackets deprecation notice), packet/impl/HelloPacketListener.java,
// listener/LuckPermsListener.java (as on_permissions_changed).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::buf::{BufError, Reader, Writer};

use super::{
    AxiomHandshakeEvent, AxiomModifyWorldEvent, AxiomPermission, AxiomPermissionSet, Config,
    MessageColor, MismatchAction, PlotBounds, PlotBox, Restrictions, RestrictionsHost, API_VERSION,
    BLUEPRINT_VERSION, CHANNEL_ENABLE, CHANNEL_GOODBYE, CHANNEL_HELLO, CHANNEL_RESTRICTIONS,
    CHANNEL_UPDATE_AVAILABLE_DISPATCH_SENDS,
};

/// Slow-path interval in game ticks (Java checks the handshake state once a second).
const SLOW_TICK_INTERVAL: u32 = 20;
/// Java keeps the budget in 1/20 units per tick; this is the multiplier.
const TICK_BUDGET_FACTOR: i32 = 20;

/// Per-server Axiom state: handshakes, active players, cached permissions and restrictions, dispatch budgets.
pub struct RestrictionsState {
    config: Config,
    /// Serverbound channels advertised in the enable packet, e.g. `axiom:set_block`.
    supported_packets: Vec<String>,
    /// Java clears the permission cache every tick unless LuckPerms is installed (then it relies on events).
    clear_permissions_each_tick: bool,
    tick_counter: u32,
    active: BTreeSet<u128>,
    sent_disable_reason: BTreeSet<u128>,
    pending_handshakes: BTreeMap<u128, i64>,
    permissions: HashMap<u128, AxiomPermissionSet>,
    restrictions: HashMap<u128, Restrictions>,
    protocol_versions: HashMap<u128, i32>,
    dispatch_sends: HashMap<u128, i32>,
    last_plot_bounds: HashMap<u128, PlotBounds>,
    dispatch_overrides: Vec<(String, i32)>,
    default_dispatch_sends: i32,
}

impl RestrictionsState {
    /// `supported_packets`: full channel names (`axiom:hello`, ...) the glue routes to this server.
    /// `luckperms_present`: true when LuckPerms is installed; permission changes then arrive as events.
    pub fn new(config: Config, supported_packets: Vec<String>, luckperms_present: bool) -> Self {
        let dispatch_overrides = config.dispatch_send_overrides();
        let default_dispatch_sends = config.default_dispatch_sends_per_second();
        Self {
            config,
            supported_packets,
            clear_permissions_each_tick: !luckperms_present,
            tick_counter: 0,
            active: BTreeSet::new(),
            sent_disable_reason: BTreeSet::new(),
            pending_handshakes: BTreeMap::new(),
            permissions: HashMap::new(),
            restrictions: HashMap::new(),
            protocol_versions: HashMap::new(),
            dispatch_sends: HashMap::new(),
            last_plot_bounds: HashMap::new(),
            dispatch_overrides,
            default_dispatch_sends,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Java `canUseAxiom(player)`: the player completed the handshake and is still active.
    pub fn is_active(&self, player: u128) -> bool {
        self.active.contains(&player)
    }

    /// Java `isMismatchedDataVersion`: the player was accepted through a ViaVersion mapping.
    pub fn is_mismatched_data_version(&self, player: u128) -> bool {
        self.protocol_versions.contains_key(&player)
    }

    /// Java `getProtocolVersionFor`.
    pub fn protocol_version_for<H: RestrictionsHost>(&self, host: &H, player: u128) -> i32 {
        self.protocol_versions
            .get(&player)
            .copied()
            .unwrap_or_else(|| host.server_protocol_version())
    }

    /// Java `hasPermission(player, perm)`: operators have everything, others go through the cached set.
    pub fn has_permission<H: RestrictionsHost>(
        &mut self,
        host: &H,
        player: u128,
        perm: AxiomPermission,
    ) -> bool {
        if host.is_op(player) {
            return true;
        }
        self.permission_set(host, player).contains(perm)
    }

    /// Java `canUseAxiom(player, perm)`.
    pub fn can_use_axiom<H: RestrictionsHost>(
        &mut self,
        host: &H,
        player: u128,
        perm: AxiomPermission,
    ) -> bool {
        self.is_active(player) && self.has_permission(host, player, perm)
    }

    /// Clientbound `axiom:hello` handshake: the client answers with its versions and the id.
    /// Mirrors `HelloPacketListener.onReceive`. `payload` is the serverbound packet body.
    pub fn on_hello<H: RestrictionsHost>(
        &mut self,
        host: &mut H,
        player: u128,
        payload: &[u8],
    ) -> Result<(), BufError> {
        let mut r = Reader::new(payload);
        let api_version = r.read_varint()?;
        if api_version != API_VERSION {
            let versions = format!(" (C={} S={})", api_version, API_VERSION);
            let message = if api_version < API_VERSION {
                format!("Unable to use Axiom, you're on an outdated version! Please update to the latest version of Axiom to use it on this server.{versions}")
            } else {
                format!("Unable to use Axiom, server hasn't updated Axiom yet.{versions}")
            };

            send_goodbye(host, player, &message);

            match self.config.unsupported_axiom_action() {
                MismatchAction::Warn => {
                    host.send_message(player, &message, MessageColor::Red);
                    return Ok(());
                }
                MismatchAction::Kick => {
                    host.kick(player, &message);
                    return Ok(());
                }
                MismatchAction::Ignore => {}
            }
        }

        let data_version = r.read_varint()?;
        let protocol_version = r.read_varint()?;
        let handshake_id = r.read_i64()?;

        if !self.accept_handshake(player, handshake_id) {
            send_goodbye(host, player, "Invalid handshake ID");
            return Ok(());
        }
        if !self.has_permission(host, player, AxiomPermission::Use) {
            send_goodbye(host, player, "Missing axiom.use permission");
            return Ok(());
        }

        let server_protocol = host.server_protocol_version();
        if protocol_version != server_protocol {
            let action = self.config.incompatible_data_action();

            if !host.plugin_enabled("ViaVersion") {
                let message = format!(
                    "Axiom: Incompatible data version detected (client {}, server {})",
                    data_version,
                    host.server_data_version()
                );
                send_goodbye(host, player, &message);

                match action {
                    MismatchAction::Warn => {
                        host.send_message(player, &message, MessageColor::Red);
                        return Ok(());
                    }
                    MismatchAction::Kick => {
                        host.kick(player, &message);
                        return Ok(());
                    }
                    MismatchAction::Ignore => {}
                }
            } else {
                match host.map_client_block_registry(player, protocol_version) {
                    Err(error) => {
                        let description = format!(
                            "client: {} <-> server: {}",
                            host.protocol_name(protocol_version),
                            host.protocol_name(server_protocol)
                        );
                        let message = format!("Axiom+ViaVersion: {error} ({description})");
                        send_goodbye(host, player, &message);

                        // Java: only `warn` keeps the player connected; `kick` and `ignore` both disconnect.
                        if action == MismatchAction::Warn {
                            host.send_message(player, &message, MessageColor::Red);
                        } else {
                            host.kick(player, &message);
                        }
                        return Ok(());
                    }
                    Ok(()) => {
                        self.protocol_versions.insert(player, protocol_version);
                        host.send_message(
                            player,
                            "Axiom: Warning, client and server versions don't match. \
                             Axiom will try to use ViaVersion conversions, but this process may cause problems",
                            MessageColor::Red,
                        );
                    }
                }
            }
        }

        self.activate(host, player);
        Ok(())
    }

    /// Java `onAxiomActive`: fires the handshake event, sends `axiom:enable`, and sends the first restrictions.
    /// The glue then sends the rest of the initial world data (world properties, blueprint manifest, ...).
    pub fn activate<H: RestrictionsHost>(&mut self, host: &mut H, player: u128) {
        let mut event = AxiomHandshakeEvent {
            player,
            max_buffer_size: self.config.max_block_buffer_packet_size,
            cancelled: false,
        };
        host.on_axiom_handshake(&mut event);
        if event.cancelled {
            return;
        }

        host.send_payload(player, CHANNEL_ENABLE, self.enable_payload());

        self.active.insert(player);
        self.pending_handshakes.remove(&player);
        self.permissions.remove(&player);
        self.restrictions.remove(&player);

        self.tick_player(host, player, true);
    }

    /// Clears the cached permission set of one player (Java `clearCachedPermissionsFor`).
    pub fn on_changed_world(&mut self, player: u128) {
        self.permissions.remove(&player);
    }

    /// LuckPerms `UserDataRecalculateEvent`: drop the cached permissions, and resend restrictions right away
    /// if the player is active. Java only drops the cache and lets the next slow tick resend.
    pub fn on_permissions_changed<H: RestrictionsHost>(&mut self, host: &mut H, player: u128) {
        self.permissions.remove(&player);
        if self.is_active(player) && host.is_online(player) {
            self.sync_restrictions(host, player);
        }
    }

    /// Server tick. Call once per game tick. Slow work runs every 20 ticks, as in Java.
    pub fn tick<H: RestrictionsHost>(&mut self, host: &mut H) {
        if self.clear_permissions_each_tick {
            self.permissions.clear();
        }

        self.tick_counter += 1;
        if self.tick_counter < SLOW_TICK_INTERVAL {
            let active: Vec<u128> = self.active.iter().copied().collect();
            for player in active {
                if host.is_online(player) {
                    self.refill_dispatch(host, player);
                }
            }
            return;
        }
        self.tick_counter = 0;

        let mut still_active = BTreeSet::new();
        let mut all_players = BTreeSet::new();

        for player in host.online_players() {
            all_players.insert(player);

            if self.active.contains(&player) {
                if !self.has_permission(host, player, AxiomPermission::Use) {
                    let mut w = Writer::new();
                    w.write_bool(false);
                    host.send_payload(player, CHANNEL_ENABLE, w.into_bytes());
                } else {
                    still_active.insert(player);
                    self.tick_player(host, player, true);
                }
            } else if self.has_permission(host, player, AxiomPermission::Use) {
                if !self.pending_handshakes.contains_key(&player) {
                    let mut id = host.random_i64();
                    if id == 0 {
                        id += 1;
                    }
                    let mut w = Writer::new();
                    w.write_i64(id);
                    host.send_payload(player, CHANNEL_HELLO, w.into_bytes());

                    self.pending_handshakes.insert(player, id);
                    self.sent_disable_reason.remove(&player);
                }
            } else if !self.sent_disable_reason.contains(&player) {
                send_goodbye(
                    host,
                    player,
                    "You don't have permission to use Axiom. Give yourself /op or axiom.default (if using a permission plugin)",
                );
                self.pending_handshakes.remove(&player);
                self.sent_disable_reason.insert(player);
            }
        }

        self.pending_handshakes
            .retain(|player, _| all_players.contains(player));
        self.sent_disable_reason
            .retain(|player| all_players.contains(player));

        // Players who lost axiom.use drop out here. The glue prunes its own per-player state
        // (tunnel buffers, no-physical-trigger, ...) against `is_active` after this call.
        self.active.retain(|player| still_active.contains(player));
        self.dispatch_sends
            .retain(|player, _| still_active.contains(player));
        self.restrictions
            .retain(|player, _| still_active.contains(player));
        self.protocol_versions
            .retain(|player, _| still_active.contains(player));
        self.last_plot_bounds
            .retain(|player, _| still_active.contains(player));
    }

    /// Consumes `sends` chunk-section sends from the player's budget. Returns false and kicks the player
    /// if the budget is exhausted (Java `consumeDispatchSends`).
    pub fn consume_dispatch_sends<H: RestrictionsHost>(
        &mut self,
        host: &mut H,
        player: u128,
        sends: i32,
        client_available: i32,
    ) -> bool {
        let allowed = self.dispatch_limit(host, player);

        let mut current = self
            .dispatch_sends
            .get(&player)
            .copied()
            .unwrap_or(allowed.wrapping_mul(TICK_BUDGET_FACTOR));
        current = current.wrapping_sub(sends.wrapping_mul(TICK_BUDGET_FACTOR));
        current = current.min(client_available.wrapping_mul(TICK_BUDGET_FACTOR));
        self.dispatch_sends.insert(player, current);

        if current < allowed.wrapping_mul(TICK_BUDGET_FACTOR).wrapping_neg() {
            host.kick(player, "You are sending updates too fast!");
            false
        } else {
            true
        }
    }

    /// Java `canModifyWorld`: whitelist and blacklist regexes, then the `AxiomModifyWorldEvent` veto.
    pub fn can_modify_world<H: RestrictionsHost>(
        &self,
        host: &mut H,
        player: u128,
        world: &str,
    ) -> bool {
        let whitelist = &self.config.whitelist_world_regex;
        if !whitelist.trim().is_empty() && !host.matches_regex(whitelist, world) {
            return false;
        }

        let blacklist = &self.config.blacklist_world_regex;
        if !blacklist.trim().is_empty() && host.matches_regex(blacklist, world) {
            return false;
        }

        let player_world = host.world_name(player);
        let mut event = AxiomModifyWorldEvent {
            player,
            world: world.to_string(),
            cancelled: player_world != world,
        };
        host.on_modify_world(&mut event);
        !event.cancelled
    }

    /// Yellow notice for operators and players with the deprecated `axiom.*` node (sent after the initial packets).
    pub fn send_deprecation_notice<H: RestrictionsHost>(&self, host: &mut H, player: u128) {
        if !host.is_op(player)
            && !host.has_permission(player, "*")
            && host.has_permission(player, "axiom.*")
        {
            host.send_message(
                player,
                "Axiom: Using deprecated axiom.* permission. Please switch to axiom.default for public servers, or axiom.all for private servers",
                MessageColor::Yellow,
            );
        }
    }

    fn accept_handshake(&mut self, player: u128, handshake_id: i64) -> bool {
        match self.pending_handshakes.get(&player) {
            Some(&expected) if expected == handshake_id => {
                self.pending_handshakes.remove(&player);
                true
            }
            _ => false,
        }
    }

    fn enable_payload(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_bool(true);
        w.write_bytes(&[0u8]); // ServerConfig version
        w.write_varint(BLUEPRINT_VERSION);
        w.write_bytes(&self.config.tunnel_split_size.to_be_bytes());
        w.write_bytes(&self.config.maximum_tunnel_packet_size.to_be_bytes());
        w.write_varint(0); // blockWithCustomData: none
        w.write_varint(0); // ignoreRotationSet: none
        w.write_varint(self.supported_packets.len() as i32);
        for id in &self.supported_packets {
            w.write_string(id);
        }
        w.into_bytes()
    }

    fn dispatch_limit<H: RestrictionsHost>(&self, host: &H, player: u128) -> i32 {
        for (key, value) in &self.dispatch_overrides {
            if host.has_permission(player, &format!("axiomlimits.{key}")) {
                return *value;
            }
        }
        self.default_dispatch_sends
    }

    /// Java `tickPlayer`: refill the dispatch budget and, if asked, resend changed restrictions.
    fn tick_player<H: RestrictionsHost>(
        &mut self,
        host: &mut H,
        player: u128,
        update_restrictions: bool,
    ) {
        self.refill_dispatch(host, player);
        if update_restrictions {
            self.sync_restrictions(host, player);
        }
    }

    fn refill_dispatch<H: RestrictionsHost>(&mut self, host: &mut H, player: u128) {
        let allowed = self.dispatch_limit(host, player);
        let max20 = allowed.wrapping_mul(TICK_BUDGET_FACTOR);

        match self.dispatch_sends.get(&player).copied() {
            None => {
                self.dispatch_sends.insert(player, max20);
                send_budget_update(host, player, allowed, allowed);
            }
            Some(previous20) => {
                let new20 = max20.min(previous20.wrapping_add(allowed));
                self.dispatch_sends.insert(player, new20);

                let previous = previous20 / TICK_BUDGET_FACTOR;
                let new = new20 / TICK_BUDGET_FACTOR;
                if previous != new {
                    send_budget_update(host, player, new.wrapping_sub(previous), allowed);
                }
            }
        }
    }

    fn sync_restrictions<H: RestrictionsHost>(&mut self, host: &mut H, player: u128) {
        let restrictions = self.calculate_restrictions(host, player);
        let changed = match self.restrictions.get(&player) {
            Some(old) => *old != restrictions,
            None => true,
        };
        if changed {
            host.send_payload(player, CHANNEL_RESTRICTIONS, restrictions.encode());
            self.restrictions.insert(player, restrictions);
        }
    }

    fn permission_set<H: RestrictionsHost>(
        &mut self,
        host: &H,
        player: u128,
    ) -> &AxiomPermissionSet {
        self.permissions
            .entry(player)
            .or_insert_with(|| calculate_permissions(host, player))
    }

    /// Java `calculateRestrictions`.
    fn calculate_restrictions<H: RestrictionsHost>(
        &mut self,
        host: &H,
        player: u128,
    ) -> Restrictions {
        let reach = self.config.infinite_reach_limit;
        if host.is_op(player) || host.has_permission(player, "axiom.all") {
            return Restrictions::all(reach);
        }

        let set = self.permission_set(host, player).clone();
        if set.contains(AxiomPermission::All) {
            return Restrictions::all(reach);
        }

        let mut bounds: BTreeSet<PlotBox> = BTreeSet::new();
        if !set.contains(AxiomPermission::AllowCopyingOtherPlots) {
            if host.is_plot_world(player) {
                if let Some(editable) = host.editable_plot(player) {
                    bounds = editable.boxes.clone();
                    self.last_plot_bounds.insert(player, editable);
                } else {
                    match self.last_plot_bounds.get(&player) {
                        Some(last) if last.world_name == host.world_name(player) => {
                            bounds = last.boxes.clone();
                        }
                        _ => {
                            bounds = BTreeSet::from([PlotBox::ORIGIN]);
                        }
                    }
                }
            }

            if bounds.len() == 1 && bounds.iter().next().is_some_and(|b| b.is_unbounded()) {
                bounds.clear();
            }
        }

        // A child node already implied by its granted parent is not sent again, as in Java.
        let allowed = filter_implied(set.explicitly_allowed());
        let denied = filter_implied(set.explicitly_denied());

        Restrictions {
            allowed_permissions: allowed,
            denied_permissions: denied,
            infinite_reach_limit: reach,
            bounds,
        }
    }
}

/// Java: skip a permission whose parent is in the same explicit list.
fn filter_implied(list: &BTreeSet<AxiomPermission>) -> BTreeSet<AxiomPermission> {
    list.iter()
        .copied()
        .filter(|p| match p.parent() {
            Some(parent) => !list.contains(&parent),
            None => true,
        })
        .collect()
}

/// Java `calculatePermissions`: reads each node's Bukkit permission value. Operators get nothing here
/// because `has_permission` short-circuits on operator status.
fn calculate_permissions<H: RestrictionsHost>(host: &H, player: u128) -> AxiomPermissionSet {
    if host.is_op(player) {
        return AxiomPermissionSet::none();
    }

    let mut allowed = BTreeSet::new();
    let mut denied = BTreeSet::new();
    for &permission in super::ALL_PERMISSIONS.iter() {
        match host.permission_value(player, permission.node()) {
            Some(false) => {
                denied.insert(permission);
            }
            Some(true) => {
                allowed.insert(permission);
            }
            None => {}
        }
    }
    AxiomPermissionSet::new(allowed, denied)
}

/// Sends `axiom:goodbye` with a reason. The client shows it and disables Axiom.
pub fn send_goodbye<H: RestrictionsHost + ?Sized>(host: &mut H, player: u128, message: &str) {
    let mut w = Writer::new();
    w.write_string(message);
    host.send_payload(player, CHANNEL_GOODBYE, w.into_bytes());
}

fn send_budget_update<H: RestrictionsHost>(host: &mut H, player: u128, add: i32, max: i32) {
    let mut w = Writer::new();
    w.write_varint(add);
    w.write_varint(max);
    host.send_payload(
        player,
        CHANNEL_UPDATE_AVAILABLE_DISPATCH_SENDS,
        w.into_bytes(),
    );
}

#[cfg(test)]
mod tests {
    use super::super::test_host::FakeHost;
    use super::*;
    use crate::buf::Writer as W;

    const ALICE: u128 = 0xA11CE;

    fn hello_body(api: i32, data: i32, protocol: i32, id: i64) -> Vec<u8> {
        let mut w = W::new();
        w.write_varint(api);
        w.write_varint(data);
        w.write_varint(protocol);
        w.write_i64(id);
        w.into_bytes()
    }

    fn state() -> RestrictionsState {
        RestrictionsState::new(
            Config::default(),
            vec!["axiom:hello".into(), "axiom:set_block".into()],
            false,
        )
    }

    fn host_with_use() -> FakeHost {
        let mut host = FakeHost {
            server_protocol: 800,
            next_random: 5,
            ..Default::default()
        };
        host.online.insert(ALICE);
        host.nodes.insert((ALICE, "axiom.use".into()), true);
        host
    }

    #[test]
    fn tick_sends_hello_with_nonzero_id_once() {
        let mut st = state();
        let mut host = host_with_use();
        host.next_random = 0; // zero is replaced by 1
        for _ in 0..SLOW_TICK_INTERVAL {
            st.tick(&mut host);
        }
        let hellos = host.sent_on(CHANNEL_HELLO);
        assert_eq!(hellos.len(), 1);
        assert_eq!(hellos[0].as_slice(), 1i64.to_be_bytes().as_slice());
    }

    #[test]
    fn handshake_activates_and_sends_enable_then_restrictions() {
        let mut st = state();
        let mut host = host_with_use();
        for _ in 0..SLOW_TICK_INTERVAL {
            st.tick(&mut host);
        }
        st.on_hello(&mut host, ALICE, &hello_body(API_VERSION, 4000, 800, 5))
            .unwrap();

        assert!(st.is_active(ALICE));
        assert_eq!(host.sent_on(CHANNEL_ENABLE).len(), 1);
        assert_eq!(host.sent_on(CHANNEL_RESTRICTIONS).len(), 1);
        assert!(host.sent_on(CHANNEL_GOODBYE).is_empty());
    }

    #[test]
    fn enable_payload_lists_config_and_packets() {
        let mut st = state();
        let mut host = host_with_use();
        st.activate(&mut host, ALICE);
        let enable = host.sent_on(CHANNEL_ENABLE)[0].clone();
        let mut r = crate::buf::Reader::new(&enable);
        assert!(r.read_bool().unwrap());
        assert_eq!(r.read_bytes(1).unwrap(), vec![0u8]);
        assert_eq!(r.read_varint().unwrap(), 2);
        assert_eq!(r.read_bytes(4).unwrap(), 31000i32.to_be_bytes().to_vec());
        assert_eq!(r.read_bytes(4).unwrap(), 2097152i32.to_be_bytes().to_vec());
        assert_eq!(r.read_varint().unwrap(), 0);
        assert_eq!(r.read_varint().unwrap(), 0);
        assert_eq!(r.read_varint().unwrap(), 2);
        assert_eq!(r.read_string(64).unwrap(), "axiom:hello");
        assert_eq!(r.read_string(64).unwrap(), "axiom:set_block");
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn wrong_handshake_id_is_refused() {
        let mut st = state();
        let mut host = host_with_use();
        for _ in 0..SLOW_TICK_INTERVAL {
            st.tick(&mut host);
        }
        st.on_hello(&mut host, ALICE, &hello_body(API_VERSION, 4000, 800, 99))
            .unwrap();
        assert!(!st.is_active(ALICE));
        let goodbye = host.sent_on(CHANNEL_GOODBYE);
        assert_eq!(goodbye.len(), 1);
        assert!(goodbye[0].ends_with(b"Invalid handshake ID"));
    }

    #[test]
    fn old_api_version_with_warn_sends_goodbye_and_message_only() {
        let mut st = state();
        let mut host = host_with_use();
        st.on_hello(&mut host, ALICE, &hello_body(API_VERSION - 1, 4000, 800, 5))
            .unwrap();
        assert!(!st.is_active(ALICE));
        assert_eq!(host.sent_on(CHANNEL_GOODBYE).len(), 1);
        assert!(host.kicks.is_empty());
        assert_eq!(host.messages.len(), 1);
        assert_eq!(host.messages[0].2, MessageColor::Red);
        assert!(host.messages[0].1.contains("(C=9 S=10)"));
    }

    #[test]
    fn newer_api_version_with_kick_kicks() {
        let mut cfg = Config::default();
        cfg.unsupported_axiom_version = "kick".into();
        let mut st = RestrictionsState::new(cfg, vec![], false);
        let mut host = host_with_use();
        st.on_hello(&mut host, ALICE, &hello_body(API_VERSION + 1, 4000, 800, 5))
            .unwrap();
        assert_eq!(host.kicks.len(), 1);
        assert!(host.kicks[0].1.contains("server hasn't updated Axiom yet"));
    }

    #[test]
    fn missing_use_permission_refuses_handshake() {
        let mut st = state();
        let mut host = host_with_use();
        // The pending id is created by the slow tick, which needs axiom.use.
        for _ in 0..SLOW_TICK_INTERVAL {
            st.tick(&mut host);
        }
        host.nodes.clear();
        st.on_changed_world(ALICE); // drop the cached permission set from the tick
        st.on_hello(&mut host, ALICE, &hello_body(API_VERSION, 4000, 800, 5))
            .unwrap();
        assert!(!st.is_active(ALICE));
        let goodbye = host.sent_on(CHANNEL_GOODBYE);
        assert!(goodbye
            .last()
            .unwrap()
            .ends_with(b"Missing axiom.use permission"));
    }

    #[test]
    fn cancelled_handshake_event_keeps_player_inactive() {
        let mut st = state();
        let mut host = host_with_use();
        host.cancel_handshake = true;
        st.activate(&mut host, ALICE);
        assert!(!st.is_active(ALICE));
        assert!(host.sent_on(CHANNEL_ENABLE).is_empty());
    }

    #[test]
    fn losing_use_disables_active_player_on_next_slow_tick() {
        let mut st = state();
        let mut host = host_with_use();
        st.activate(&mut host, ALICE);
        host.nodes.clear();
        for _ in 0..SLOW_TICK_INTERVAL {
            st.tick(&mut host);
        }
        assert!(!st.is_active(ALICE));
        let enables = host.sent_on(CHANNEL_ENABLE);
        assert_eq!(enables.last().unwrap().as_slice(), &[0u8]);
    }

    #[test]
    fn dispatch_budget_kicks_when_exhausted() {
        let mut cfg = Config::default();
        cfg.block_buffer_rate_limit = 10;
        let mut st = RestrictionsState::new(cfg, vec![], false);
        let mut host = host_with_use();
        st.activate(&mut host, ALICE);
        // Budget starts at 10 sections per second (200 in 1/20 units). Spending 1000 sections must kick.
        assert!(!st.consume_dispatch_sends(&mut host, ALICE, 1000, 10));
        assert_eq!(host.kicks.len(), 1);
        assert_eq!(host.kicks[0].1, "You are sending updates too fast!");
    }

    #[test]
    fn dispatch_budget_accepts_small_spend() {
        let mut st = state();
        let mut host = host_with_use();
        st.activate(&mut host, ALICE);
        assert!(st.consume_dispatch_sends(&mut host, ALICE, 5, 1024));
        assert!(host.kicks.is_empty());
    }

    #[test]
    fn op_gets_all_restrictions() {
        let mut st = state();
        let mut host = host_with_use();
        host.ops.insert(ALICE);
        st.activate(&mut host, ALICE);
        let payload = host.sent_on(CHANNEL_RESTRICTIONS)[0].clone();
        let mut r = crate::buf::Reader::new(&payload);
        assert_eq!(r.read_varint().unwrap(), 1);
        assert_eq!(r.read_string(64).unwrap(), "axiom.all");
    }

    #[test]
    fn denied_build_node_appears_in_restrictions() {
        let mut st = state();
        let mut host = host_with_use();
        host.nodes
            .insert((ALICE, "axiom.build.place".into()), false);
        st.activate(&mut host, ALICE);
        let payload = host.sent_on(CHANNEL_RESTRICTIONS)[0].clone();
        let mut r = crate::buf::Reader::new(&payload);
        let allowed = r.read_varint().unwrap();
        for _ in 0..allowed {
            r.read_string(64).unwrap();
        }
        assert_eq!(r.read_varint().unwrap(), 1);
        assert_eq!(r.read_string(64).unwrap(), "axiom.build.place");
    }

    #[test]
    fn permission_change_resends_restrictions_only_when_changed() {
        let mut st = state();
        let mut host = host_with_use();
        st.activate(&mut host, ALICE);
        assert_eq!(host.sent_on(CHANNEL_RESTRICTIONS).len(), 1);

        st.on_permissions_changed(&mut host, ALICE);
        assert_eq!(
            host.sent_on(CHANNEL_RESTRICTIONS).len(),
            1,
            "unchanged, no resend"
        );

        host.nodes.insert((ALICE, "axiom.tool.melt".into()), false);
        st.on_permissions_changed(&mut host, ALICE);
        assert_eq!(host.sent_on(CHANNEL_RESTRICTIONS).len(), 2);
    }

    #[test]
    fn plot_world_without_plot_limits_to_origin() {
        let mut st = state();
        let mut host = host_with_use();
        host.plot_world = true;
        st.activate(&mut host, ALICE);
        let payload = host.sent_on(CHANNEL_RESTRICTIONS)[0].clone();
        let mut r = crate::buf::Reader::new(&payload);
        let allowed = r.read_varint().unwrap();
        for _ in 0..allowed {
            r.read_string(64).unwrap();
        }
        let denied = r.read_varint().unwrap();
        for _ in 0..denied {
            r.read_string(64).unwrap();
        }
        r.read_bytes(4).unwrap();
        assert_eq!(r.read_varint().unwrap(), 1);
        assert_eq!(r.read_block_pos().unwrap(), (0, 0, 0));
        assert_eq!(r.read_block_pos().unwrap(), (0, 0, 0));
    }

    #[test]
    fn modify_world_defaults_to_same_world_only() {
        let st = state();
        let mut host = host_with_use();
        host.worlds.insert(ALICE, "plot".into());
        assert!(st.can_modify_world(&mut host, ALICE, "plot"));
        assert!(!st.can_modify_world(&mut host, ALICE, "other"));
        host.cancel_modify = Some(false);
        assert!(st.can_modify_world(&mut host, ALICE, "other"));
    }

    #[test]
    fn blacklist_regex_blocks_world() {
        let mut cfg = Config::default();
        cfg.blacklist_world_regex = "lobby".into();
        let st = RestrictionsState::new(cfg, vec![], false);
        let mut host = host_with_use();
        assert!(!st.can_modify_world(&mut host, ALICE, "lobby"));
    }
}
