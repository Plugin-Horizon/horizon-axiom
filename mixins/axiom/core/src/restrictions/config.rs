// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): src/main/resources/config.yml
// (keys and defaults) and the config reads in AxiomPaper.onEnable / onAxiomActive / canModifyWorld.
//
// Keys are the config.yml keys with '-' replaced by '_' in Rust and written with '-' in TOML.
// Defaults are the config.yml values. Keys the Java code reads without a config.yml entry
// (`max-block-buffer-packet-size`) default to Java's getInt() fallback, 0.

use std::collections::BTreeMap;

use serde::Deserialize;

/// What to do when a client's Axiom or data version does not match the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MismatchAction {
    Kick,
    Warn,
    Ignore,
}

/// Per-permission override of the block buffer rate limit (`limits.<key>` in config.yml).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LimitOverride {
    /// Chunk sections per second. Values <= 0 are ignored.
    pub block_buffer_rate_limit: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    /// Max chunk sends per tick (per world), 0 = no limit.
    pub max_chunk_sends_per_tick: i32,
    /// Max chunk relights per tick (per world), 0 = no limit.
    pub max_chunk_relights_per_tick: i32,
    /// Max distance (chunks) from which players may load chunks. 0 = only render distance.
    pub max_chunk_load_distance: i32,
    /// Whether players may teleport between worlds using views.
    pub allow_teleport_between_worlds: bool,
    /// Whether clients may save/load/share blueprints through the server.
    pub blueprint_sharing: bool,
    /// Allow large chunk and entity data requests (not recommended for public servers).
    pub allow_large_chunk_data_request: bool,
    /// Raise the size limit of all packets to 2 MB instead of 32 KB (not recommended for public servers).
    pub allow_large_payload_for_all_packets: bool,
    /// `kick`, `warn` or `ignore`: client data version differs from the server.
    pub incompatible_data_version: String,
    /// `kick`, `warn` or `ignore`: client Axiom API version differs from the server.
    pub unsupported_axiom_version: String,
    /// Regex (full match) a world name must match to be modifiable. Empty = no whitelist.
    pub whitelist_world_regex: String,
    /// Regex (full match) that makes a world unmodifiable. Empty = no blacklist.
    pub blacklist_world_regex: String,
    /// Block buffer rate limit (chunk sections per second), 0 = default (1024).
    pub block_buffer_rate_limit: i32,
    /// Tunnel split size in bytes.
    pub tunnel_split_size: i32,
    /// Maximum tunnel packet size in bytes.
    pub maximum_tunnel_packet_size: i32,
    /// Log large block buffer changes.
    pub log_large_block_buffer_changes: bool,
    /// Entities clients may spawn/manipulate/delete (Java `whitelist-entities`).
    pub whitelist_entities: Vec<String>,
    /// Entities clients may not spawn/manipulate/delete.
    pub blacklist_entities: Vec<String>,
    /// Disables entity data sanitization (not recommended for public servers).
    pub disable_entity_sanitization: bool,
    /// Allow players to see and manipulate marker entities.
    pub send_markers: bool,
    /// Enable annotations on this server.
    pub allow_annotations: bool,
    /// Block ids/states clients may not use, e.g. `minecraft:wheat`.
    pub disallowed_blocks: Vec<String>,
    /// Infinite reach limit sent to clients.
    pub infinite_reach_limit: i32,
    /// Log Axiom changes with CoreProtect (if installed).
    pub log_core_protect_changes: bool,
    /// Per-permission overrides. The `example` key is ignored, as in Java.
    pub limits: BTreeMap<String, LimitOverride>,
    /// Not present in config.yml; Java reads it with getInt() and default 0.
    pub max_block_buffer_packet_size: i32,
}

impl Default for Config {
    fn default() -> Self {
        let mut limits = BTreeMap::new();
        limits.insert(
            "example".to_string(),
            LimitOverride {
                block_buffer_rate_limit: 20,
            },
        );
        Self {
            max_chunk_sends_per_tick: 0,
            max_chunk_relights_per_tick: 0,
            max_chunk_load_distance: 256,
            allow_teleport_between_worlds: true,
            blueprint_sharing: false,
            allow_large_chunk_data_request: false,
            allow_large_payload_for_all_packets: false,
            incompatible_data_version: "warn".to_string(),
            unsupported_axiom_version: "warn".to_string(),
            whitelist_world_regex: String::new(),
            blacklist_world_regex: String::new(),
            block_buffer_rate_limit: 0,
            tunnel_split_size: 31000,
            maximum_tunnel_packet_size: 2097152,
            log_large_block_buffer_changes: false,
            whitelist_entities: Vec::new(),
            blacklist_entities: Vec::new(),
            disable_entity_sanitization: false,
            send_markers: false,
            allow_annotations: false,
            disallowed_blocks: Vec::new(),
            infinite_reach_limit: 256,
            log_core_protect_changes: true,
            limits,
            max_block_buffer_packet_size: 0,
        }
    }
}

/// Java `AxiomPaper.onEnable` default for the packet collection read limit.
const DEFAULT_PACKET_COLLECTION_READ_LIMIT: i32 = 1024;
/// `Short.MAX_VALUE`, used when large payloads are allowed.
const LARGE_PACKET_COLLECTION_READ_LIMIT: i32 = i16::MAX as i32;
/// Java `AxiomPaper.onEnable` default when block-buffer-rate-limit is <= 0.
const DEFAULT_DISPATCH_SENDS_PER_SECOND: i32 = 1024;

impl Config {
    /// Parses a TOML document. Missing keys take the config.yml defaults.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Action for an Axiom API version mismatch. Unknown strings mean `kick`, as in Java.
    pub fn unsupported_axiom_action(&self) -> MismatchAction {
        parse_action(&self.unsupported_axiom_version)
    }

    /// Action for a data version mismatch. Unknown strings mean `kick`, as in Java.
    pub fn incompatible_data_action(&self) -> MismatchAction {
        parse_action(&self.incompatible_data_version)
    }

    /// Default block buffer rate limit (sections per second) after the <= 0 fallback.
    pub fn default_dispatch_sends_per_second(&self) -> i32 {
        if self.block_buffer_rate_limit <= 0 {
            DEFAULT_DISPATCH_SENDS_PER_SECOND
        } else {
            self.block_buffer_rate_limit
        }
    }

    /// `limits` entries in key order that set a positive rate limit, `example` skipped (case-insensitive).
    /// Java iterates a LinkedHashMap in YAML order; here keys are in sorted order, see the port notes.
    pub fn dispatch_send_overrides(&self) -> Vec<(String, i32)> {
        self.limits
            .iter()
            .filter(|(key, _)| !key.eq_ignore_ascii_case("example"))
            .filter(|(_, values)| values.block_buffer_rate_limit > 0)
            .map(|(key, values)| (key.clone(), values.block_buffer_rate_limit))
            .collect()
    }

    /// Packet collection read limit (Java `getPacketCollectionReadLimit`).
    pub fn packet_collection_read_limit(&self) -> i32 {
        if self.allow_large_payload_for_all_packets {
            LARGE_PACKET_COLLECTION_READ_LIMIT
        } else {
            DEFAULT_PACKET_COLLECTION_READ_LIMIT
        }
    }

    /// Chunk load distance for a player's world. Plot worlds are limited to 0 (Java `getMaxChunkLoadDistance`).
    pub fn max_chunk_load_distance_for(&self, is_plot_world: bool) -> i32 {
        if is_plot_world {
            0
        } else {
            self.max_chunk_load_distance
        }
    }
}

fn parse_action(value: &str) -> MismatchAction {
    match value {
        "warn" => MismatchAction::Warn,
        "ignore" => MismatchAction::Ignore,
        _ => MismatchAction::Kick,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_config_yml() {
        let c = Config::default();
        assert_eq!(c.max_chunk_load_distance, 256);
        assert_eq!(c.tunnel_split_size, 31000);
        assert_eq!(c.maximum_tunnel_packet_size, 2097152);
        assert_eq!(c.infinite_reach_limit, 256);
        assert_eq!(c.incompatible_data_version, "warn");
        assert_eq!(c.unsupported_axiom_version, "warn");
        assert!(c.allow_teleport_between_worlds);
        assert!(c.log_core_protect_changes);
        assert!(!c.blueprint_sharing);
        assert_eq!(c.packet_collection_read_limit(), 1024);
        assert_eq!(c.default_dispatch_sends_per_second(), 1024);
    }

    #[test]
    fn empty_toml_gives_defaults() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn toml_uses_hyphenated_keys() {
        let c = Config::from_toml(
            "infinite-reach-limit = 64\nblueprint-sharing = true\n\n[limits.vip]\nblock-buffer-rate-limit = 50\n",
        )
        .unwrap();
        assert_eq!(c.infinite_reach_limit, 64);
        assert!(c.blueprint_sharing);
        assert_eq!(c.dispatch_send_overrides(), vec![("vip".to_string(), 50)]);
        // Unset keys keep their default.
        assert_eq!(c.tunnel_split_size, 31000);
    }

    #[test]
    fn example_limit_and_non_positive_limits_are_skipped() {
        let c = Config::from_toml(
            "[limits.Example]\nblock-buffer-rate-limit = 5\n[limits.zero]\nblock-buffer-rate-limit = 0\n[limits.fast]\nblock-buffer-rate-limit = 99\n",
        )
        .unwrap();
        assert_eq!(c.dispatch_send_overrides(), vec![("fast".to_string(), 99)]);
    }

    #[test]
    fn unknown_mismatch_strings_mean_kick() {
        let mut c = Config::default();
        assert_eq!(c.unsupported_axiom_action(), MismatchAction::Warn);
        c.unsupported_axiom_version = "ignore".into();
        assert_eq!(c.unsupported_axiom_action(), MismatchAction::Ignore);
        c.unsupported_axiom_version = "bogus".into();
        assert_eq!(c.unsupported_axiom_action(), MismatchAction::Kick);
    }

    #[test]
    fn large_payload_flag_raises_read_limit() {
        let mut c = Config::default();
        c.allow_large_payload_for_all_packets = true;
        assert_eq!(c.packet_collection_read_limit(), 32767);
    }

    #[test]
    fn plot_world_chunk_load_distance_is_zero() {
        let c = Config::default();
        assert_eq!(c.max_chunk_load_distance_for(true), 0);
        assert_eq!(c.max_chunk_load_distance_for(false), 256);
    }
}
