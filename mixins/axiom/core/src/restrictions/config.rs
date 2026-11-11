// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): src/main/resources/config.yml
// (keys and defaults) and the config reads in AxiomPaper.onEnable / onAxiomActive / canModifyWorld.
//
// Keys are the config.yml keys with '-' replaced by '_' in Rust and written with '-' in TOML.
// Defaults are the config.yml values. Keys the Java code reads without a config.yml entry
// (`max-block-buffer-packet-size`) default to Java's getInt() fallback, 0.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::yaml::{self, Node};

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

    /// Parses the original `plugins/AxiomPaper/config.yml` (Paper's file, read with the vendored YAML reader).
    /// Returns the settings and every key path the loader asked for, whether the file has it or not. A key of
    /// the wrong type keeps its default. `limits` is read as Java reads it: one entry per key, `example` included
    /// (it is skipped when the overrides are used, see `dispatch_send_overrides`).
    pub fn from_original_yaml(text: &str) -> Result<(Self, BTreeSet<String>), yaml::Error> {
        let root = yaml::parse(text)?;
        let mut keys = Keys { root: &root, read: BTreeSet::new() };
        let mut c = Config::default();
        keys.int("max-chunk-sends-per-tick", &mut c.max_chunk_sends_per_tick);
        keys.int("max-chunk-relights-per-tick", &mut c.max_chunk_relights_per_tick);
        keys.int("max-chunk-load-distance", &mut c.max_chunk_load_distance);
        keys.bool("allow-teleport-between-worlds", &mut c.allow_teleport_between_worlds);
        keys.bool("blueprint-sharing", &mut c.blueprint_sharing);
        keys.bool("allow-large-chunk-data-request", &mut c.allow_large_chunk_data_request);
        keys.bool("allow-large-payload-for-all-packets", &mut c.allow_large_payload_for_all_packets);
        keys.string("incompatible-data-version", &mut c.incompatible_data_version);
        keys.string("unsupported-axiom-version", &mut c.unsupported_axiom_version);
        keys.string("whitelist-world-regex", &mut c.whitelist_world_regex);
        keys.string("blacklist-world-regex", &mut c.blacklist_world_regex);
        keys.int("block-buffer-rate-limit", &mut c.block_buffer_rate_limit);
        keys.int("tunnel-split-size", &mut c.tunnel_split_size);
        keys.int("maximum-tunnel-packet-size", &mut c.maximum_tunnel_packet_size);
        keys.bool("log-large-block-buffer-changes", &mut c.log_large_block_buffer_changes);
        keys.list("whitelist-entities", &mut c.whitelist_entities);
        keys.list("blacklist-entities", &mut c.blacklist_entities);
        keys.bool("disable-entity-sanitization", &mut c.disable_entity_sanitization);
        keys.bool("send-markers", &mut c.send_markers);
        keys.bool("allow-annotations", &mut c.allow_annotations);
        keys.list("disallowed-blocks", &mut c.disallowed_blocks);
        keys.int("infinite-reach-limit", &mut c.infinite_reach_limit);
        keys.bool("log-core-protect-changes", &mut c.log_core_protect_changes);

        // Every entry of `limits` is an override; a missing `limits` means none (not the `example` default).
        c.limits = BTreeMap::new();
        if let Some(limits) = keys.node("limits") {
            for entry in limits.entries() {
                let path = format!("limits.{}.block-buffer-rate-limit", entry.key);
                keys.read.insert(path);
                let mut limit = LimitOverride::default();
                if let Some(v) = entry.value.get("block-buffer-rate-limit").and_then(int_of) {
                    limit.block_buffer_rate_limit = v;
                }
                c.limits.insert(entry.key.clone(), limit);
            }
        }
        Ok((c, keys.read))
    }

    /// Reads and parses the original config file (see `from_original_yaml`). Errors name the file, not its text.
    pub fn read_original(path: &Path) -> Result<(Self, BTreeSet<String>), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_original_yaml(&text).map_err(|e| format!("{}: {e}", path.display()))
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

/// The original file's path in a server folder (`plugins/AxiomPaper/config.yml`).
pub fn original_path(server_dir: &Path) -> PathBuf {
    server_dir.join("plugins").join("AxiomPaper").join("config.yml")
}

/// A whole number as Bukkit's `getInt`: a float is truncated, anything else that is not a number is absent.
fn int_of(node: &Node) -> Option<i32> {
    let v = node.as_i64().or_else(|| node.as_f64().map(|f| f as i64))?;
    i32::try_from(v).ok()
}

/// Reads keys of the original file and records the path of each one asked for.
struct Keys<'a> {
    root: &'a Node,
    read: BTreeSet<String>,
}

impl<'a> Keys<'a> {
    fn node(&mut self, path: &str) -> Option<&'a Node> {
        self.read.insert(path.to_string());
        self.root.path(path)
    }

    fn int(&mut self, path: &str, into: &mut i32) {
        if let Some(v) = self.node(path).and_then(int_of) {
            *into = v;
        }
    }

    fn bool(&mut self, path: &str, into: &mut bool) {
        if let Some(v) = self.node(path).and_then(Node::as_bool) {
            *into = v;
        }
    }

    fn string(&mut self, path: &str, into: &mut String) {
        if let Some(v) = self.node(path).and_then(Node::as_str) {
            *into = v.to_string();
        }
    }

    /// Bukkit's `getStringList`: a missing or non-list value is an empty list.
    fn list(&mut self, path: &str, into: &mut Vec<String>) {
        *into = self.node(path).map(Node::str_list).unwrap_or_default();
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

    /// A file with the structure of AxiomPaper 6.0.1's config.yml (same keys, same order, same nesting), with the
    /// default values. Hand-written: no server file is copied here.
    const ORIGINAL_FIXTURE: &str = "\
# Max chunk sends per tick (per-world), 0 = no limit
max-chunk-sends-per-tick: 0
max-chunk-relights-per-tick: 0
max-chunk-load-distance: 256
allow-teleport-between-worlds: true
blueprint-sharing: false
allow-large-chunk-data-request: false
allow-large-payload-for-all-packets: false
incompatible-data-version: \"warn\"
unsupported-axiom-version: \"warn\"
whitelist-world-regex: \"\"
blacklist-world-regex: \"\"
block-buffer-rate-limit: 0
tunnel-split-size: 31000
maximum-tunnel-packet-size: 2097152
log-large-block-buffer-changes: false
whitelist-entities: []
blacklist-entities: []
disable-entity-sanitization: false
send-markers: false
allow-annotations: false
disallowed-blocks: []
infinite-reach-limit: 256
log-core-protect-changes: true
limits:
  example:
    # Block buffer rate-limit (in chunk sections per second), 0 = default limit
    block-buffer-rate-limit: 20
";

    /// Every key of `ORIGINAL_FIXTURE` the loader must read: the 24 top-level keys and the one nested key.
    const ORIGINAL_KEYS: [&str; 25] = [
        "max-chunk-sends-per-tick",
        "max-chunk-relights-per-tick",
        "max-chunk-load-distance",
        "allow-teleport-between-worlds",
        "blueprint-sharing",
        "allow-large-chunk-data-request",
        "allow-large-payload-for-all-packets",
        "incompatible-data-version",
        "unsupported-axiom-version",
        "whitelist-world-regex",
        "blacklist-world-regex",
        "block-buffer-rate-limit",
        "tunnel-split-size",
        "maximum-tunnel-packet-size",
        "log-large-block-buffer-changes",
        "whitelist-entities",
        "blacklist-entities",
        "disable-entity-sanitization",
        "send-markers",
        "allow-annotations",
        "disallowed-blocks",
        "infinite-reach-limit",
        "log-core-protect-changes",
        "limits",
        "limits.example.block-buffer-rate-limit",
    ];

    /// Leaf key paths of a tree (`limits.example.block-buffer-rate-limit`); lists are leaves.
    fn leaf_paths(node: &Node, prefix: &str, out: &mut Vec<String>) {
        for entry in node.entries() {
            let path = if prefix.is_empty() { entry.key.clone() } else { format!("{prefix}.{}", entry.key) };
            if entry.value.is_map() {
                leaf_paths(&entry.value, &path, out);
            } else {
                out.push(path);
            }
        }
    }

    #[test]
    fn original_fixture_equals_defaults_except_the_ignored_example() {
        let (c, _) = Config::from_original_yaml(ORIGINAL_FIXTURE).unwrap();
        // The `example` limit is read like any other entry, and only dispatch skips it, so the whole value
        // equals Config::default().
        assert_eq!(c, Config::default());
        assert!(c.dispatch_send_overrides().is_empty());
    }

    #[test]
    fn every_key_of_the_original_file_is_read() {
        let root = yaml::parse(ORIGINAL_FIXTURE).unwrap();
        let mut in_file = Vec::new();
        leaf_paths(&root, "", &mut in_file);
        let (_, read) = Config::from_original_yaml(ORIGINAL_FIXTURE).unwrap();
        for key in &in_file {
            assert!(read.contains(key), "key {key} of the file is not read");
        }
        let expected: BTreeSet<String> = ORIGINAL_KEYS.iter().map(|k| k.to_string()).collect();
        assert_eq!(read, expected);
    }

    #[test]
    fn missing_keys_keep_their_defaults_and_no_limits_means_no_overrides() {
        let (c, read) = Config::from_original_yaml("").unwrap();
        let mut expected = Config::default();
        expected.limits.clear();
        assert_eq!(c, expected);
        assert!(read.contains("infinite-reach-limit"));
    }

    #[test]
    fn limits_map_is_read_per_key_and_example_is_skipped_on_dispatch() {
        let text = "limits:\n  example:\n    block-buffer-rate-limit: 20\n  vip:\n    block-buffer-rate-limit: 50\n  zero:\n    block-buffer-rate-limit: 0\n";
        let (c, read) = Config::from_original_yaml(text).unwrap();
        assert_eq!(c.limits.len(), 3);
        assert_eq!(c.limits["vip"].block_buffer_rate_limit, 50);
        assert!(read.contains("limits.vip.block-buffer-rate-limit"));
        assert_eq!(c.dispatch_send_overrides(), vec![("vip".to_string(), 50)]);
    }

    #[test]
    fn values_of_the_wrong_type_keep_their_defaults() {
        let text = "infinite-reach-limit: \"abc\"\nallow-annotations: yes-ish\nblueprint-sharing: true\nmax-chunk-load-distance: 12.0\n";
        let (c, _) = Config::from_original_yaml(text).unwrap();
        assert_eq!(c.infinite_reach_limit, 256);
        assert!(!c.allow_annotations);
        assert!(c.blueprint_sharing);
        assert_eq!(c.max_chunk_load_distance, 12);
    }

    #[test]
    fn quoted_mismatch_actions_are_read_as_text() {
        let (c, _) = Config::from_original_yaml("incompatible-data-version: 'ignore'\nunsupported-axiom-version: \"kick\"\n").unwrap();
        assert_eq!(c.incompatible_data_action(), MismatchAction::Ignore);
        assert_eq!(c.unsupported_axiom_action(), MismatchAction::Kick);
    }

    #[test]
    fn a_tab_indented_file_is_a_parse_error() {
        assert!(Config::from_original_yaml("limits:\n\tvip: 1\n").is_err());
    }

    #[test]
    fn original_path_is_the_plugin_config() {
        assert_eq!(
            original_path(Path::new("/srv/server")),
            PathBuf::from("/srv/server/plugins/AxiomPaper/config.yml")
        );
    }

    #[test]
    fn read_original_reads_a_file_and_names_a_missing_one() {
        let dir = std::env::temp_dir().join(format!("axiom-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.yml");
        std::fs::write(&file, ORIGINAL_FIXTURE).unwrap();
        let (c, _) = Config::read_original(&file).unwrap();
        assert_eq!(c, Config::default());
        let missing = dir.join("no-such-file.yml");
        let err = Config::read_original(&missing).unwrap_err();
        assert!(err.contains("no-such-file.yml"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
