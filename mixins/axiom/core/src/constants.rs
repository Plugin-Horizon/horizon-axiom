// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): AxiomConstants.java, DisallowedBlocks.java
// (default list), VersionHelper.java (channel naming), AxiomPaper.java (channel registrations and config defaults),
// config.yml (default values), packet/impl/*.java and blueprint/BlueprintHeader.java (version numbers).

use crate::buf::pack_block_pos;

// ---------------------------------------------------------------------------------------------------------------------
// Protocol versions and flags
// ---------------------------------------------------------------------------------------------------------------------

/// Axiom API version the server speaks. The client must send the same value in `hello`.
pub const API_VERSION: i32 = 10;

/// Blueprint file/packet header version (`BlueprintHeader.CURRENT_VERSION`).
pub const BLUEPRINT_HEADER_VERSION: i32 = 2;

/// First Minecraft protocol version whose NBT is sent without a root name (1.20.2). Older clients need conversion.
pub const UNNAMED_COMPOUND_TAG_PROTOCOL: i32 = 764;

/// Flag bit in a tunnel packet header: body is ZSTD compressed.
pub const TUNNEL_PACKET_FLAG_ZSTD_COMPRESSED: u8 = 1;

/// Flag bit in a tunnel buffer header: first chunk of a split packet.
pub const TUNNEL_BUFFER_FLAG_FIRST: u8 = 1;
/// Flag bit in a tunnel buffer header: last chunk of a split packet.
pub const TUNNEL_BUFFER_FLAG_LAST: u8 = 2;

/// `BlockPos` packed as `AxiomConstants.MIN_POSITION_LONG` (the minimum block position in the packed form).
pub const MIN_POSITION_LONG: i64 = pack_block_pos(-33_554_432, -2048, -33_554_432);

// ---------------------------------------------------------------------------------------------------------------------
// Config defaults (config.yml) that the protocol code depends on
// ---------------------------------------------------------------------------------------------------------------------

/// `tunnel-split-size`: maximum size of one tunnel buffer in bytes.
pub const DEFAULT_TUNNEL_SPLIT_SIZE: usize = 31_000;
/// `maximum-tunnel-packet-size`: maximum uncompressed size of one tunnel packet in bytes.
pub const DEFAULT_MAXIMUM_TUNNEL_PACKET_SIZE: usize = 2_097_152;
/// Collection read limit for normal packets (`packetCollectionReadLimit`).
pub const DEFAULT_PACKET_COLLECTION_READ_LIMIT: usize = 1024;
/// Collection read limit with `allow-large-payload-for-all-packets` (`Short.MAX_VALUE`).
pub const LARGE_PACKET_COLLECTION_READ_LIMIT: usize = 32_767;
/// NBT decompression budget in bytes (`maxNbtDecompressLimit`). The large-payload setting lifts it entirely.
pub const DEFAULT_MAX_NBT_DECOMPRESS_LIMIT: u64 = 131_072;
/// `infinite-reach-limit` default.
pub const DEFAULT_INFINITE_REACH_LIMIT: i32 = 256;
/// `max-chunk-load-distance` default, in chunks.
pub const DEFAULT_MAX_CHUNK_LOAD_DISTANCE: i32 = 256;

/// `disallowed-blocks` default: nothing is disallowed. Entries use block-state syntax such as `minecraft:wheat` or
/// `minecraft:oak_stairs[waterlogged=true]`, and `#tag` forms are allowed too. The host resolves them against its
/// block registry.
pub const DEFAULT_DISALLOWED_BLOCKS: &[&str] = &[];

// ---------------------------------------------------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------------------------------------------------

/// Namespace of every Axiom plugin channel.
pub const CHANNEL_NAMESPACE: &str = "axiom";

/// Serverbound envelope: every other serverbound packet is wrapped in this one.
pub const CHANNEL_TUNNEL: &str = "axiom:tunnel";

// Serverbound packets, dispatched through the tunnel. Each is also registered as a plugin channel of its own.
pub const CHANNEL_HELLO: &str = "axiom:hello";
pub const CHANNEL_SET_GAMEMODE: &str = "axiom:set_gamemode";
pub const CHANNEL_SET_FLY_SPEED: &str = "axiom:set_fly_speed";
pub const CHANNEL_TELEPORT: &str = "axiom:teleport";
pub const CHANNEL_SET_WORLD_TIME: &str = "axiom:set_world_time";
pub const CHANNEL_SET_NO_PHYSICAL_TRIGGER: &str = "axiom:set_no_physical_trigger";
pub const CHANNEL_SET_WORLD_PROPERTY: &str = "axiom:set_world_property";
pub const CHANNEL_REQUEST_CHUNK_DATA: &str = "axiom:request_chunk_data";
pub const CHANNEL_REQUEST_ENTITY_DATA: &str = "axiom:request_entity_data";
pub const CHANNEL_SPAWN_ENTITY: &str = "axiom:spawn_entity";
pub const CHANNEL_MANIPULATE_ENTITY: &str = "axiom:manipulate_entity";
pub const CHANNEL_DELETE_ENTITY: &str = "axiom:delete_entity";
pub const CHANNEL_MARKER_NBT_REQUEST: &str = "axiom:marker_nbt_request";
pub const CHANNEL_SET_BLOCK: &str = "axiom:set_block";
pub const CHANNEL_SET_BUFFER: &str = "axiom:set_buffer";
pub const CHANNEL_TICK_BLOCKS: &str = "axiom:tick_blocks";
pub const CHANNEL_UPLOAD_BLUEPRINT: &str = "axiom:upload_blueprint";
pub const CHANNEL_REQUEST_BLUEPRINT: &str = "axiom:request_blueprint";
pub const CHANNEL_ANNOTATION_UPDATE: &str = "axiom:annotation_update";

// Clientbound packets.
pub const CHANNEL_ENABLE: &str = "axiom:enable";
pub const CHANNEL_HELLO_RESPONSE: &str = "axiom:hello";
pub const CHANNEL_GOODBYE: &str = "axiom:goodbye";
pub const CHANNEL_RESTRICTIONS: &str = "axiom:restrictions";
pub const CHANNEL_RESPONSE_CHUNK_DATA: &str = "axiom:response_chunk_data";
pub const CHANNEL_RESPONSE_ENTITY_DATA: &str = "axiom:response_entity_data";
pub const CHANNEL_REGISTER_WORLD_PROPERTIES: &str = "axiom:register_world_properties";
pub const CHANNEL_ACK_WORLD_PROPERTIES: &str = "axiom:ack_world_properties";
pub const CHANNEL_MARKER_DATA: &str = "axiom:marker_data";
pub const CHANNEL_MARKER_NBT_RESPONSE: &str = "axiom:marker_nbt_response";
pub const CHANNEL_ADD_SERVER_HEIGHTMAP: &str = "axiom:add_server_heightmap";
pub const CHANNEL_CUSTOM_BLOCKS: &str = "axiom:custom_blocks";
pub const CHANNEL_REGISTER_CUSTOM_BLOCK_V2: &str = "axiom:register_custom_block_v2";
pub const CHANNEL_IGNORE_DISPLAY_ENTITIES: &str = "axiom:ignore_display_entities";
pub const CHANNEL_REGISTER_CUSTOM_ITEMS: &str = "axiom:register_custom_items";
pub const CHANNEL_UPDATE_AVAILABLE_DISPATCH_SENDS: &str = "axiom:update_available_dispatch_sends";
pub const CHANNEL_BLUEPRINT_MANIFEST: &str = "axiom:blueprint_manifest";
pub const CHANNEL_RESPONSE_BLUEPRINT: &str = "axiom:response_blueprint";

/// Serverbound packet names sent through the tunnel (`AxiomPaper.registerPacketHandler`), without the namespace.
/// `upload_blueprint` and `request_blueprint` are registered only when `blueprint-sharing` is on, and
/// `annotation_update` only with `allow-annotations`.
pub const SERVERBOUND_PACKET_NAMES: &[&str] = &[
    "hello",
    "set_gamemode",
    "set_fly_speed",
    "teleport",
    "set_world_time",
    "set_no_physical_trigger",
    "set_world_property",
    "request_chunk_data",
    "request_entity_data",
    "spawn_entity",
    "manipulate_entity",
    "delete_entity",
    "marker_nbt_request",
    "set_block",
    "set_buffer",
    "tick_blocks",
    "upload_blueprint",
    "request_blueprint",
    "annotation_update",
];

/// Every serverbound plugin channel the server listens on: the tunnel plus each packet's own channel.
pub const SERVERBOUND_CHANNELS: &[&str] = &[
    CHANNEL_TUNNEL,
    CHANNEL_HELLO,
    CHANNEL_SET_GAMEMODE,
    CHANNEL_SET_FLY_SPEED,
    CHANNEL_TELEPORT,
    CHANNEL_SET_WORLD_TIME,
    CHANNEL_SET_NO_PHYSICAL_TRIGGER,
    CHANNEL_SET_WORLD_PROPERTY,
    CHANNEL_REQUEST_CHUNK_DATA,
    CHANNEL_REQUEST_ENTITY_DATA,
    CHANNEL_SPAWN_ENTITY,
    CHANNEL_MANIPULATE_ENTITY,
    CHANNEL_DELETE_ENTITY,
    CHANNEL_MARKER_NBT_REQUEST,
    CHANNEL_SET_BLOCK,
    CHANNEL_SET_BUFFER,
    CHANNEL_TICK_BLOCKS,
    CHANNEL_UPLOAD_BLUEPRINT,
    CHANNEL_REQUEST_BLUEPRINT,
    CHANNEL_ANNOTATION_UPDATE,
];

/// Every clientbound plugin channel the server sends on.
pub const CLIENTBOUND_CHANNELS: &[&str] = &[
    CHANNEL_ENABLE,
    CHANNEL_HELLO_RESPONSE,
    CHANNEL_GOODBYE,
    CHANNEL_RESTRICTIONS,
    CHANNEL_RESPONSE_CHUNK_DATA,
    CHANNEL_RESPONSE_ENTITY_DATA,
    CHANNEL_REGISTER_WORLD_PROPERTIES,
    CHANNEL_SET_WORLD_PROPERTY,
    CHANNEL_ACK_WORLD_PROPERTIES,
    CHANNEL_MARKER_DATA,
    CHANNEL_MARKER_NBT_RESPONSE,
    CHANNEL_ANNOTATION_UPDATE,
    CHANNEL_ADD_SERVER_HEIGHTMAP,
    CHANNEL_CUSTOM_BLOCKS,
    CHANNEL_REGISTER_CUSTOM_BLOCK_V2,
    CHANNEL_IGNORE_DISPLAY_ENTITIES,
    CHANNEL_REGISTER_CUSTOM_ITEMS,
    CHANNEL_UPDATE_AVAILABLE_DISPATCH_SENDS,
    CHANNEL_BLUEPRINT_MANIFEST,
    CHANNEL_RESPONSE_BLUEPRINT,
];

// ---------------------------------------------------------------------------------------------------------------------
// Custom block property keys and NBT keys (`axiom:*` names that are not channels)
// ---------------------------------------------------------------------------------------------------------------------

/// Standard property keys a custom block can declare (`ImplAxiomCustomBlock`).
pub const CUSTOM_BLOCK_PROPERTY_AXIS: &str = "axiom:axis";
pub const CUSTOM_BLOCK_PROPERTY_FACING: &str = "axiom:facing";
pub const CUSTOM_BLOCK_PROPERTY_FACING_OPPOSITE: &str = "axiom:facing_opposite";
pub const CUSTOM_BLOCK_PROPERTY_FACING_CLICKED: &str = "axiom:facing_clicked";
pub const CUSTOM_BLOCK_PROPERTY_FACING_CLICKED_OPPOSITE: &str = "axiom:facing_clicked_opposite";
pub const CUSTOM_BLOCK_PROPERTY_WATERLOGGED: &str = "axiom:waterlogged";
pub const CUSTOM_BLOCK_PROPERTY_HALF: &str = "axiom:half";

/// NBT key in a manipulate-entity payload: when present, the right-hand tag replaces the entity data instead of merging.
pub const NBT_KEY_MODIFY: &str = "axiom:modify";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::unpack_block_pos;
    use std::collections::HashSet;

    #[test]
    fn min_position_long_matches_java_literal() {
        // The Java static check compares against this binary literal.
        assert_eq!(
            MIN_POSITION_LONG,
            0b1000000000000000000000000010000000000000000000000000100000000000u64 as i64
        );
        assert_eq!(unpack_block_pos(MIN_POSITION_LONG), (-33_554_432, -2048, -33_554_432));
    }

    #[test]
    fn protocol_numbers() {
        assert_eq!(API_VERSION, 10);
        assert_eq!(BLUEPRINT_HEADER_VERSION, 2);
        assert_eq!(UNNAMED_COMPOUND_TAG_PROTOCOL, 764);
        assert_eq!(TUNNEL_PACKET_FLAG_ZSTD_COMPRESSED, 1);
        assert_eq!(TUNNEL_BUFFER_FLAG_FIRST, 1);
        assert_eq!(TUNNEL_BUFFER_FLAG_LAST, 2);
        assert_eq!(DEFAULT_TUNNEL_SPLIT_SIZE, 31_000);
        assert_eq!(DEFAULT_MAXIMUM_TUNNEL_PACKET_SIZE, 2_097_152);
        assert_eq!(DEFAULT_PACKET_COLLECTION_READ_LIMIT, 1024);
        assert_eq!(LARGE_PACKET_COLLECTION_READ_LIMIT, i16::MAX as usize);
        assert_eq!(DEFAULT_MAX_NBT_DECOMPRESS_LIMIT, 131_072);
        assert!(DEFAULT_DISALLOWED_BLOCKS.is_empty());
    }

    #[test]
    fn channel_names_are_namespaced_and_unique() {
        let all: Vec<&str> = SERVERBOUND_CHANNELS.iter().chain(CLIENTBOUND_CHANNELS).copied().collect();
        for name in &all {
            assert!(name.starts_with("axiom:"), "{name}");
            assert!(!name["axiom:".len()..].is_empty(), "{name}");
        }
        // hello, set_world_property and annotation_update exist in both directions by design.
        let server: HashSet<&str> = SERVERBOUND_CHANNELS.iter().copied().collect();
        let client: HashSet<&str> = CLIENTBOUND_CHANNELS.iter().copied().collect();
        assert_eq!(server.len(), SERVERBOUND_CHANNELS.len());
        assert_eq!(client.len(), CLIENTBOUND_CHANNELS.len());
        let both: HashSet<&str> = server.intersection(&client).copied().collect();
        assert_eq!(
            both,
            HashSet::from(["axiom:hello", "axiom:set_world_property", "axiom:annotation_update"])
        );
    }

    #[test]
    fn serverbound_packet_names_match_channels() {
        assert_eq!(SERVERBOUND_PACKET_NAMES.len(), SERVERBOUND_CHANNELS.len() - 1);
        for name in SERVERBOUND_PACKET_NAMES {
            let channel = format!("axiom:{name}");
            assert!(SERVERBOUND_CHANNELS.contains(&channel.as_str()), "{channel}");
        }
        let names: HashSet<&str> = SERVERBOUND_PACKET_NAMES.iter().copied().collect();
        assert_eq!(names.len(), SERVERBOUND_PACKET_NAMES.len());
        assert!(!names.contains("tunnel"));
    }

    #[test]
    fn tunnel_channel_and_namespace() {
        assert_eq!(CHANNEL_TUNNEL, "axiom:tunnel");
        assert_eq!(CHANNEL_NAMESPACE, "axiom");
        assert_eq!(format!("{CHANNEL_NAMESPACE}:tunnel"), CHANNEL_TUNNEL);
    }

    #[test]
    fn custom_block_property_keys() {
        let keys = [
            CUSTOM_BLOCK_PROPERTY_AXIS,
            CUSTOM_BLOCK_PROPERTY_FACING,
            CUSTOM_BLOCK_PROPERTY_FACING_OPPOSITE,
            CUSTOM_BLOCK_PROPERTY_FACING_CLICKED,
            CUSTOM_BLOCK_PROPERTY_FACING_CLICKED_OPPOSITE,
            CUSTOM_BLOCK_PROPERTY_WATERLOGGED,
            CUSTOM_BLOCK_PROPERTY_HALF,
        ];
        let unique: HashSet<&str> = keys.iter().copied().collect();
        assert_eq!(unique.len(), keys.len());
        assert_eq!(NBT_KEY_MODIFY, "axiom:modify");
    }
}
