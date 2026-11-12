// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): restrictions/AxiomPermission.java,
// restrictions/AxiomPermissionSet.java, restrictions/Restrictions.java, packet/impl/HelloPacketListener.java,
// listener/LuckPermsListener.java, event/AxiomHandshakeEvent.java, event/AxiomModifyWorldEvent.java, and the
// handshake / enable / goodbye / restrictions / dispatch-rate logic of AxiomPaper.java.
//
// Players are `u128` UUIDs. Everything the Bukkit/Paper/PlotSquared/ViaVersion side provides is reached
// through `RestrictionsHost`, which the game glue implements.

mod config;
mod payload;
mod permission;
mod permission_set;
mod state;

pub use config::{original_path, Config, LimitOverride, MismatchAction};
pub use payload::{PlotBounds, PlotBox, Restrictions, CHANNEL_RESTRICTIONS};
pub use permission::{AxiomPermission, ALL_PERMISSIONS};
pub use permission_set::AxiomPermissionSet;
pub use state::{send_goodbye, RestrictionsState};

/// Axiom API version the server speaks (`AxiomConstants.API_VERSION`).
pub const API_VERSION: i32 = 10;

/// Blueprint version written into the enable packet (`buf.writeVarInt(2)`).
pub const BLUEPRINT_VERSION: i32 = 2;

/// Clientbound: handshake request, payload is an i64 handshake id.
pub const CHANNEL_HELLO: &str = "axiom:hello";
/// Clientbound: enable (`true` + config) or disable (`false`) Axiom for the player.
pub const CHANNEL_ENABLE: &str = "axiom:enable";
/// Clientbound: a reason why Axiom is not usable (string).
pub const CHANNEL_GOODBYE: &str = "axiom:goodbye";
/// Clientbound: dispatch-send budget update (varint add, varint max).
pub const CHANNEL_UPDATE_AVAILABLE_DISPATCH_SENDS: &str = "axiom:update_available_dispatch_sends";

/// Colour of a chat line sent to a player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageColor {
    Red,
    Yellow,
}

/// `AxiomHandshakeEvent`: fired when a player completes the handshake. A listener may set `cancelled`
/// to keep Axiom disabled for that player. Changes to `max_buffer_size` are not used afterwards, as in Java.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiomHandshakeEvent {
    pub player: u128,
    pub max_buffer_size: i32,
    pub cancelled: bool,
}

/// `AxiomModifyWorldEvent`: fired by `RestrictionsState::can_modify_world`. `cancelled` starts as
/// "the player is not in this world"; a listener may reset it to allow cross-world edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiomModifyWorldEvent {
    pub player: u128,
    pub world: String,
    pub cancelled: bool,
}

/// Everything the restrictions logic needs from the game. Implemented by the glue.
pub trait RestrictionsHost {
    /// Bukkit `Player#isOp`.
    fn is_op(&self, player: u128) -> bool;
    /// Bukkit `Player#hasPermission(node)`, including the host's own op/default handling.
    fn has_permission(&self, player: u128, node: &str) -> bool;
    /// Bukkit `Player#permissionValue(node)`: `Some(true)` = TRUE, `Some(false)` = FALSE, `None` = NOT_SET.
    fn permission_value(&self, player: u128, node: &str) -> Option<bool>;
    /// Whether the player is online on this proxy-backed server.
    fn is_online(&self, player: u128) -> bool;
    /// All online players.
    fn online_players(&self) -> Vec<u128>;
    /// Name of the world the player is in (Java `player.getWorld().getName()`).
    fn world_name(&self, player: u128) -> String;

    /// Send a custom payload on `channel`.
    fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>);
    /// Send a chat line.
    fn send_message(&mut self, player: u128, text: &str, color: MessageColor);
    /// Disconnect the player with `reason`.
    fn kick(&mut self, player: u128, reason: &str);
    /// Random i64 for handshake ids (Java `ThreadLocalRandom.nextLong()`).
    fn random_i64(&mut self) -> i64;

    /// Server Minecraft protocol version (`SharedConstants.getProtocolVersion()`).
    fn server_protocol_version(&self) -> i32;
    /// Server data version (`DFUHelper.DATA_VERSION`), used in the incompatible-data message.
    fn server_data_version(&self) -> i32;
    /// Whether a plugin is enabled (`Bukkit.getPluginManager().isPluginEnabled`), e.g. "ViaVersion", "LuckPerms".
    fn plugin_enabled(&self, name: &str) -> bool;
    /// Human name of a protocol number (`ProtocolVersion.getProtocol(n).toString()`).
    fn protocol_name(&self, protocol: i32) -> String;
    /// ViaVersion path: build the block-state mapping for a client on `protocol`, for `player`.
    /// `Err(message)` when no mapping exists. The glue stores the mapping; this module only tracks the protocol.
    fn map_client_block_registry(&mut self, player: u128, protocol: i32) -> Result<(), String>;

    /// PlotSquared: whether the player's world is a plot world.
    fn is_plot_world(&self, player: u128) -> bool;
    /// PlotSquared: the plot the player may currently edit, if any (`getCurrentEditablePlot`).
    fn editable_plot(&self, player: u128) -> Option<PlotBounds>;

    /// Java `String.matches(pattern)`: the whole of `text` must match the regex `pattern`.
    fn matches_regex(&self, pattern: &str, text: &str) -> bool;

    /// Event hook for `AxiomHandshakeEvent`. Listeners may set `event.cancelled`.
    fn on_axiom_handshake(&mut self, event: &mut AxiomHandshakeEvent);
    /// Event hook for `AxiomModifyWorldEvent`. Listeners may change `event.cancelled`.
    fn on_modify_world(&mut self, event: &mut AxiomModifyWorldEvent);
}

#[cfg(test)]
pub(crate) mod test_host {
    //! A recording host used by the state tests.
    use super::*;
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    #[derive(Default)]
    pub struct FakeHost {
        pub ops: BTreeSet<u128>,
        pub nodes: HashMap<(u128, String), bool>,
        pub online: BTreeSet<u128>,
        pub worlds: BTreeMap<u128, String>,
        pub plot_world: bool,
        pub editable: Option<PlotBounds>,
        pub via_installed: bool,
        pub via_error: Option<String>,
        pub next_random: i64,
        pub server_protocol: i32,
        pub sent: Vec<(u128, String, Vec<u8>)>,
        pub messages: Vec<(u128, String, MessageColor)>,
        pub kicks: Vec<(u128, String)>,
        pub cancel_handshake: bool,
        pub cancel_modify: Option<bool>,
    }

    impl FakeHost {
        pub fn sent_on(&self, channel: &str) -> Vec<&Vec<u8>> {
            self.sent
                .iter()
                .filter(|s| s.1 == channel)
                .map(|s| &s.2)
                .collect()
        }
    }

    impl RestrictionsHost for FakeHost {
        fn is_op(&self, player: u128) -> bool {
            self.ops.contains(&player)
        }
        fn has_permission(&self, player: u128, node: &str) -> bool {
            self.is_op(player) || self.nodes.get(&(player, node.to_string())) == Some(&true)
        }
        fn permission_value(&self, player: u128, node: &str) -> Option<bool> {
            self.nodes.get(&(player, node.to_string())).copied()
        }
        fn is_online(&self, player: u128) -> bool {
            self.online.contains(&player)
        }
        fn online_players(&self) -> Vec<u128> {
            self.online.iter().copied().collect()
        }
        fn world_name(&self, player: u128) -> String {
            self.worlds
                .get(&player)
                .cloned()
                .unwrap_or_else(|| "world".into())
        }
        fn send_payload(&mut self, player: u128, channel: &str, data: Vec<u8>) {
            self.sent.push((player, channel.to_string(), data));
        }
        fn send_message(&mut self, player: u128, text: &str, color: MessageColor) {
            self.messages.push((player, text.to_string(), color));
        }
        fn kick(&mut self, player: u128, reason: &str) {
            self.kicks.push((player, reason.to_string()));
        }
        fn random_i64(&mut self) -> i64 {
            self.next_random
        }
        fn server_protocol_version(&self) -> i32 {
            self.server_protocol
        }
        fn server_data_version(&self) -> i32 {
            4000
        }
        fn plugin_enabled(&self, name: &str) -> bool {
            name == "ViaVersion" && self.via_installed
        }
        fn protocol_name(&self, protocol: i32) -> String {
            format!("proto{protocol}")
        }
        fn map_client_block_registry(
            &mut self,
            _player: u128,
            _protocol: i32,
        ) -> Result<(), String> {
            match &self.via_error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
        fn is_plot_world(&self, _player: u128) -> bool {
            self.plot_world
        }
        fn editable_plot(&self, _player: u128) -> Option<PlotBounds> {
            self.editable.clone()
        }
        fn matches_regex(&self, pattern: &str, text: &str) -> bool {
            pattern == text
        }
        fn on_axiom_handshake(&mut self, event: &mut AxiomHandshakeEvent) {
            event.cancelled = self.cancel_handshake;
        }
        fn on_modify_world(&mut self, event: &mut AxiomModifyWorldEvent) {
            if let Some(c) = self.cancel_modify {
                event.cancelled = c;
            }
        }
    }
}
