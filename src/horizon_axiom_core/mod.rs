//! Axiom server protocol, ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd).
//! No Horizon types here: each module defines the small host trait it needs, which the game glue implements.

pub mod regex;        // small regex matcher (Java String.matches) for the world filters
pub mod buf;          // byte reader/writer: varints, strings, block positions, uuids, minimal NBT
pub mod constants;    // channels, protocol version, permission names, disallowed blocks
pub mod restrictions; // permissions, restrictions payload, hello/enable handshake
pub mod buffer;       // BlockBuffer, BiomeBuffer, Position2ByteMap, PositionSet, compressed block entities
pub mod setblock;     // set_block / set_buffer payloads and the operation queue
pub mod request;      // chunk and entity data requests
pub mod entity;       // spawn / manipulate / delete entities, NBT sanitizing
pub mod world;        // world properties, time, game mode, fly speed, teleport, no-physical-trigger
pub mod blueprint;    // blueprint manifest, upload, request, file IO
pub mod annotation;   // annotations and markers
pub mod custom;       // custom blocks/displays API, debug command
pub mod yaml;         // vendored horizon-yaml: reads the original plugins/AxiomPaper/config.yml
