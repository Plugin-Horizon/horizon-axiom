// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): blueprint/ServerBlueprintManager.java
// (sendManifest), packet/impl/UploadBlueprintPacketListener.java and packet/impl/BlueprintRequestPacketListener.java,
// and the path checks of the upload listener. The Java upload listener parses on the network thread and writes on
// the server thread; here the whole handler runs where the host calls it (the server's main thread).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::buf::{Reader, Writer, DEFAULT_STRING_MAX_CHARS};

use super::{
    check_string, file, BlueprintError, BlueprintHost, BlueprintStore, RawBlueprint, StateIds, CHANNEL_MANIFEST,
    CHANNEL_RESPONSE, MANIFEST_MAX_PACKET_BYTES, PERMISSION_MANIFEST, PERMISSION_REQUEST, PERMISSION_UPLOAD,
};

/// Result of an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadOutcome {
    /// Permission, version or path check failed. Nothing was read or written, as in Java.
    Ignored,
    /// File written and registry updated under `key`.
    Stored { key: String },
    /// The file could not be written. Java drops this silently and so does this code.
    Dropped,
}

/// Result of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestOutcome {
    Ignored,
    /// The blueprint was sent back to the player.
    Sent,
    /// No blueprint under the requested key.
    NotFound,
}

/// Builds the manifest packets for a registry. Each packet starts with a replace flag (true on the first only),
/// then `(key, header, thumbnail)` entries and an empty key as terminator. A packet is closed once it grows past
/// `MANIFEST_MAX_PACKET_BYTES`.
pub fn manifest_packets(blueprints: &BTreeMap<String, RawBlueprint>) -> Result<Vec<Vec<u8>>, BlueprintError> {
    let mut packets = Vec::new();
    let mut w = Writer::new();
    w.write_bool(true); // replace

    for (key, blueprint) in blueprints {
        check_string(key)?;
        let mut entry = Writer::new();
        entry.write_string(key);
        blueprint.write_header_wire(&mut entry)?;
        w.write_bytes(entry.as_bytes());

        if w.len() > MANIFEST_MAX_PACKET_BYTES {
            // Finish this packet and continue in a new one that does not replace.
            w.write_string("");
            packets.push(std::mem::take(&mut w).into_bytes());
            w.write_bool(false);
        }
    }

    w.write_string("");
    packets.push(w.into_bytes());
    Ok(packets)
}

/// `ServerBlueprintManager.sendManifest`: sends the manifest to each listed player who may see it.
pub fn send_manifest<H: BlueprintHost + ?Sized>(
    host: &mut H,
    store: &BlueprintStore,
    players: &[u128],
) -> Result<(), BlueprintError> {
    let recipients: Vec<u128> = players
        .iter()
        .copied()
        .filter(|&player| host.can_use_axiom(player, PERMISSION_MANIFEST))
        .collect();
    if recipients.is_empty() {
        return Ok(());
    }
    for packet in manifest_packets(store.blueprints())? {
        for &player in &recipients {
            host.send_payload(player, CHANNEL_MANIFEST, packet.clone());
        }
    }
    Ok(())
}

/// Handles one upload. `payload` is the packet body: path string, then the full blueprint.
pub fn handle_upload<H: BlueprintHost + ?Sized>(
    host: &mut H,
    store: &mut BlueprintStore,
    player: u128,
    payload: &[u8],
) -> Result<UploadOutcome, BlueprintError> {
    if !host.can_use_axiom(player, PERMISSION_UPLOAD) {
        return Ok(UploadOutcome::Ignored);
    }
    if host.is_mismatched_data_version(player) {
        let text = version_mismatch_text(&*host);
        host.send_message(player, &text);
        return Ok(UploadOutcome::Ignored);
    }

    let ids = StateIds::of(&*host);
    let mut reader = Reader::new(payload);
    let path = reader.read_string(DEFAULT_STRING_MAX_CHARS)?;
    let blueprint = RawBlueprint::read_wire(&mut reader, ids)?;

    let Some((relative, path_name)) = validate_upload_path(&path) else {
        return Ok(UploadOutcome::Ignored);
    };

    // Encode first: Java opens the output file before encoding and can leave a truncated file behind.
    let bytes = match file::write_raw(&*host, &blueprint) {
        Ok(bytes) => bytes,
        Err(e) => {
            host.kick(player, &format!("An error occured while uploading blueprint: {e}"));
            return Ok(UploadOutcome::Dropped);
        }
    };

    let target = store.root().join(&relative);
    if write_file(&target, &bytes).is_err() {
        return Ok(UploadOutcome::Dropped);
    }

    let key = format!("/{path_name}");
    store.insert(key.clone(), blueprint);

    let players = host.online_players();
    send_manifest(host, store, &players)?;
    Ok(UploadOutcome::Stored { key })
}

/// Handles one request. `payload` is the requested key.
pub fn handle_request<H: BlueprintHost + ?Sized>(
    host: &mut H,
    store: &BlueprintStore,
    player: u128,
    payload: &[u8],
) -> Result<RequestOutcome, BlueprintError> {
    if !host.can_use_axiom(player, PERMISSION_REQUEST) {
        return Ok(RequestOutcome::Ignored);
    }
    if host.is_mismatched_data_version(player) {
        let text = version_mismatch_text(&*host);
        host.send_message(player, &text);
        return Ok(RequestOutcome::Ignored);
    }

    let mut reader = Reader::new(payload);
    let path = reader.read_string(DEFAULT_STRING_MAX_CHARS)?;
    let Some(blueprint) = store.get(&path) else {
        return Ok(RequestOutcome::NotFound);
    };

    let mut w = Writer::new();
    w.write_string(&path);
    blueprint.write_wire(&mut w, StateIds::of(&*host))?;
    host.send_payload(player, CHANNEL_RESPONSE, w.into_bytes());
    Ok(RequestOutcome::Sent)
}

fn version_mismatch_text<H: BlueprintHost + ?Sized>(host: &H) -> String {
    format!(
        "Axiom+ViaVersion: This feature isn't supported. Switch your client version to {} to use this",
        host.game_version_name()
    )
}

/// The upload path rules of the Java listener: backslashes become slashes, the path must start with `/`, end in
/// `.bp` and contain no `..`. Returns the relative file path and the key name without `.bp`. The key keeps any
/// `.` segments, as the Java code does.
fn validate_upload_path(raw: &str) -> Option<(PathBuf, String)> {
    let path = raw.replace('\\', "/");
    if !path.ends_with(".bp") || path.contains("..") || !path.starts_with('/') {
        return None;
    }
    let path = &path[1..];

    let mut relative = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if relative.as_os_str().is_empty() {
        return None;
    }

    let path_name = path[..path.len() - 3].to_owned();
    Some((relative, path_name))
}

fn write_file(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::test_host::{sample_blueprint, FakeHost};
    use crate::blueprint::{BlueprintHeader, RawBlueprint, CHANNEL_MANIFEST};

    const PLAYER: u128 = 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef;
    const OTHER: u128 = 42;

    fn store_with(blueprints: &[(&str, RawBlueprint)]) -> BlueprintStore {
        let mut store = BlueprintStore::default();
        for (key, bp) in blueprints {
            store.insert((*key).to_owned(), bp.clone());
        }
        store
    }

    /// Decodes one manifest packet: replace flag, entries until the empty key.
    fn decode_manifest(packet: &[u8]) -> (bool, Vec<(String, BlueprintHeader, usize)>) {
        let mut r = Reader::new(packet);
        let replace = r.read_bool().unwrap();
        let mut entries = Vec::new();
        loop {
            let key = r.read_string(DEFAULT_STRING_MAX_CHARS).unwrap();
            if key.is_empty() {
                break;
            }
            let header = BlueprintHeader::read_wire(&mut r).unwrap();
            let thumbnail = r.read_byte_array(usize::MAX).unwrap();
            entries.push((key, header, thumbnail.len()));
        }
        assert!(r.is_finished(), "packet has trailing bytes");
        (replace, entries)
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("horizon-axiom-bp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_registry_is_one_replacing_packet() {
        let packets = manifest_packets(&BTreeMap::new()).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(decode_manifest(&packets[0]), (true, Vec::new()));
    }

    #[test]
    fn manifest_lists_each_key_with_header_and_thumbnail() {
        let mut blueprints = BTreeMap::new();
        blueprints.insert("/a/one".to_owned(), sample_blueprint("one"));
        blueprints.insert("/two".to_owned(), sample_blueprint("two"));
        let packets = manifest_packets(&blueprints).unwrap();
        assert_eq!(packets.len(), 1);

        let (replace, entries) = decode_manifest(&packets[0]);
        assert!(replace);
        let keys: Vec<&str> = entries.iter().map(|(k, _, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["/a/one", "/two"]);
        assert_eq!(entries[0].1.name, "one");
        assert_eq!(entries[0].1.tags, vec!["small".to_owned(), "wood".to_owned()]);
        assert_eq!(entries[1].2, 7, "thumbnail length");
    }

    #[test]
    fn manifest_splits_after_one_megabyte_and_only_the_first_replaces() {
        let big = |name: &str| {
            let mut bp = sample_blueprint(name);
            bp.thumbnail = vec![7; 600_000];
            bp
        };
        let mut blueprints = BTreeMap::new();
        blueprints.insert("/a".to_owned(), big("a"));
        blueprints.insert("/b".to_owned(), big("b"));
        blueprints.insert("/c".to_owned(), big("c"));
        let packets = manifest_packets(&blueprints).unwrap();
        assert_eq!(packets.len(), 2);

        let (first_replace, first) = decode_manifest(&packets[0]);
        let (second_replace, second) = decode_manifest(&packets[1]);
        assert!(first_replace);
        assert!(!second_replace);
        // The first packet closes after the entry that passes the limit, so it holds two entries.
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].0, "/c");
    }

    #[test]
    fn manifest_goes_only_to_players_with_the_permission() {
        let mut host = FakeHost::new();
        host.grant(PLAYER, PERMISSION_MANIFEST);
        let store = store_with(&[("/x", sample_blueprint("x"))]);
        send_manifest(&mut host, &store, &[PLAYER, OTHER]).unwrap();
        assert_eq!(host.sent.len(), 1);
        assert_eq!(host.sent[0].0, PLAYER);
        assert_eq!(host.sent[0].1, CHANNEL_MANIFEST);
    }

    #[test]
    fn upload_stores_file_and_resends_manifest() {
        let root = temp_root("upload");
        let mut store = BlueprintStore { root: root.clone(), ..Default::default() };
        let mut host = FakeHost::new();
        host.grant(PLAYER, PERMISSION_UPLOAD);
        host.grant(OTHER, PERMISSION_MANIFEST);
        host.players = vec![PLAYER, OTHER];

        let blueprint = sample_blueprint("uploaded");
        let mut w = Writer::new();
        w.write_string("\\houses\\small.bp");
        blueprint.write_wire(&mut w, StateIds::of(&host)).unwrap();

        let outcome = handle_upload(&mut host, &mut store, PLAYER, &w.into_bytes()).unwrap();
        assert_eq!(outcome, UploadOutcome::Stored { key: "/houses/small".to_owned() });
        assert!(root.join("houses/small.bp").is_file());
        assert_eq!(store.get("/houses/small").map(|b| &b.header.name), Some(&"uploaded".to_owned()));

        let manifest: Vec<_> = host.sent.iter().filter(|(_, ch, _)| ch == CHANNEL_MANIFEST).collect();
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].0, OTHER, "only players with the manifest permission receive it");

        // The file on disk reads back to the same blueprint, minus the block entity dropped on write.
        let file_bytes = std::fs::read(root.join("houses/small.bp")).unwrap();
        let on_disk = crate::blueprint::read_raw(&host, &file_bytes).unwrap();
        assert_eq!(on_disk.blocks, blueprint.blocks);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn upload_without_permission_or_with_bad_path_does_nothing() {
        let root = temp_root("reject");
        let mut store = BlueprintStore { root: root.clone(), ..Default::default() };
        let mut host = FakeHost::new();
        host.grant(PLAYER, PERMISSION_UPLOAD);

        let payload = |path: &str| {
            let mut w = Writer::new();
            w.write_string(path);
            sample_blueprint("x").write_wire(&mut w, StateIds::of(&FakeHost::new())).unwrap();
            w.into_bytes()
        };

        assert_eq!(
            handle_upload(&mut host, &mut store, OTHER, &payload("/ok.bp")).unwrap(),
            UploadOutcome::Ignored
        );
        for bad in ["/../x.bp", "/a/../b.bp", "a.bp", "/a.txt", "/"] {
            assert_eq!(
                handle_upload(&mut host, &mut store, PLAYER, &payload(bad)).unwrap(),
                UploadOutcome::Ignored,
                "{bad}"
            );
        }
        assert!(store.blueprints().is_empty());
        assert!(host.sent.is_empty());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn upload_from_version_mismatched_client_is_refused_with_message() {
        let mut store = BlueprintStore::default();
        let mut host = FakeHost::new();
        host.grant(PLAYER, PERMISSION_UPLOAD);
        host.mismatched.insert(PLAYER);
        assert_eq!(handle_upload(&mut host, &mut store, PLAYER, &[]).unwrap(), UploadOutcome::Ignored);
        assert_eq!(host.messages.len(), 1);
        assert!(host.messages[0].1.ends_with("Switch your client version to 26.3 to use this"));
    }

    #[test]
    fn request_sends_the_key_and_the_full_blueprint() {
        let store = store_with(&[("/houses/small", sample_blueprint("small"))]);
        let mut host = FakeHost::new();
        host.grant(PLAYER, PERMISSION_REQUEST);

        let mut w = Writer::new();
        w.write_string("/houses/small");
        let outcome = handle_request(&mut host, &store, PLAYER, &w.into_bytes()).unwrap();
        assert_eq!(outcome, RequestOutcome::Sent);

        let (to, channel, data) = &host.sent[0];
        assert_eq!(*to, PLAYER);
        assert_eq!(channel, CHANNEL_RESPONSE);
        let mut r = Reader::new(data);
        assert_eq!(r.read_string(DEFAULT_STRING_MAX_CHARS).unwrap(), "/houses/small");
        let back = RawBlueprint::read_wire(&mut r, StateIds::of(&host)).unwrap();
        let stored = store.get("/houses/small").unwrap();
        // The wire header drops the thumbnail camera (Java behaviour), so compare everything else.
        assert_eq!(back.header.name, stored.header.name);
        assert_eq!(back.header.tags, stored.header.tags);
        assert_eq!(back.thumbnail, stored.thumbnail);
        assert_eq!(back.blocks, stored.blocks);
        assert_eq!(back.block_entities, stored.block_entities);
        assert_eq!(back.entities, stored.entities);
    }

    #[test]
    fn request_for_unknown_key_sends_nothing() {
        let store = BlueprintStore::default();
        let mut host = FakeHost::new();
        host.grant(PLAYER, PERMISSION_REQUEST);
        let mut w = Writer::new();
        w.write_string("/missing");
        assert_eq!(
            handle_request(&mut host, &store, PLAYER, &w.into_bytes()).unwrap(),
            RequestOutcome::NotFound
        );
        assert!(host.sent.is_empty());
    }

    #[test]
    fn upload_path_rules_match_the_java_checks() {
        assert_eq!(
            validate_upload_path("/a/./b.bp"),
            Some((PathBuf::from("a/b.bp"), "a/./b".to_owned()))
        );
        assert_eq!(validate_upload_path("/x\\y.bp"), Some((PathBuf::from("x/y.bp"), "x/y".to_owned())));
        assert_eq!(validate_upload_path("/a..b.bp"), None);
        assert_eq!(validate_upload_path("/a.bp.txt"), None);
        // "//x.bp" strips to "/x.bp", which is absolute and refused, as in Java.
        assert_eq!(validate_upload_path("//x.bp"), None);
    }
}
