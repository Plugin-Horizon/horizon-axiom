// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): WorldExtension.java
// (per-world chunk resend and relight queues, marker change sync, join handling).
//
// Budget semantics follow the Java exactly: a per-tick budget <= 0 means "unlimited" (drain everything).
// Chunks that are not loaded, or have no tracking player, stay queued and do not use budget.

use std::collections::{BTreeMap, BTreeSet};

use crate::buf::Writer;

use super::{WorldHost, ANTI_XRAY_WARNING, CH_MARKER_DATA};

#[derive(Default, Debug)]
pub struct WorldExtension {
    pending_chunks_to_send: BTreeSet<(i32, i32)>,
    pending_chunks_to_light: BTreeSet<(i32, i32)>,
    /// Last encoded marker per UUID, to detect changes.
    previous_markers: BTreeMap<u128, Vec<u8>>,
}

impl WorldExtension {
    pub fn queue_send(&mut self, cx: i32, cz: i32) {
        self.pending_chunks_to_send.insert((cx, cz));
    }

    pub fn queue_light(&mut self, cx: i32, cz: i32) {
        self.pending_chunks_to_light.insert((cx, cz));
    }

    /// Join handling: sends the known markers to the new player, then the anti-xray warning if needed.
    pub fn on_player_join(&mut self, host: &mut dyn WorldHost, player: u128) {
        if !self.previous_markers.is_empty() {
            let encoded: Vec<Vec<u8>> = self.previous_markers.values().cloned().collect();
            host.send_payload(player, CH_MARKER_DATA, encode_marker_update(&encoded, &[]));
        }
        if host.anti_xray_enabled(player) {
            host.send_warning(player, ANTI_XRAY_WARNING);
        }
    }

    /// Per-tick work for one dimension: markers (if enabled), then chunk sends, then relights.
    pub fn tick(
        &mut self,
        host: &mut dyn WorldHost,
        dim: &str,
        send_markers: bool,
        max_chunk_relights: i32,
        max_chunk_sends: i32,
    ) {
        if send_markers {
            self.tick_markers(host, dim);
        }
        self.tick_chunk_sends(host, dim, max_chunk_sends);
        self.tick_chunk_relights(host, dim, max_chunk_relights);
    }

    fn tick_markers(&mut self, host: &mut dyn WorldHost, dim: &str) {
        let mut changed: Vec<Vec<u8>> = Vec::new();
        let mut present: BTreeSet<u128> = BTreeSet::new();

        for marker in host.marker_snapshot(dim) {
            present.insert(marker.uuid);
            let differs = self.previous_markers.get(&marker.uuid) != Some(&marker.encoded);
            if differs {
                self.previous_markers
                    .insert(marker.uuid, marker.encoded.clone());
                changed.push(marker.encoded);
            }
        }

        let missing: Vec<u128> = self
            .previous_markers
            .keys()
            .filter(|u| !present.contains(*u))
            .copied()
            .collect();
        for uuid in &missing {
            self.previous_markers.remove(uuid);
        }

        if changed.is_empty() && missing.is_empty() {
            return;
        }

        let payload = encode_marker_update(&changed, &missing);
        let candidates = host.players_in_dimension(dim);
        let targets: Vec<u128> = candidates
            .into_iter()
            .filter(|p| host.is_axiom_active(*p))
            .collect();
        for player in targets {
            host.send_payload(player, CH_MARKER_DATA, payload.clone());
        }
    }

    fn tick_chunk_sends(&mut self, host: &mut dyn WorldHost, dim: &str, max_sends: i32) {
        let send_all = max_sends <= 0;
        let mut budget = max_sends;
        let pending: Vec<(i32, i32)> = self.pending_chunks_to_send.iter().copied().collect();

        for (cx, cz) in pending {
            if !host.is_chunk_loaded(dim, cx, cz) {
                continue;
            }
            let players = host.players_tracking_chunk(dim, cx, cz);
            if players.is_empty() {
                continue;
            }
            host.send_chunk_with_light(&players, dim, cx, cz);

            if !send_all {
                self.pending_chunks_to_send.remove(&(cx, cz));
                budget -= 1;
                if budget <= 0 {
                    break;
                }
            }
        }

        if send_all {
            self.pending_chunks_to_send.clear();
        }
    }

    fn tick_chunk_relights(&mut self, host: &mut dyn WorldHost, dim: &str, max_relights: i32) {
        let mut chunks: BTreeSet<(i32, i32)> = BTreeSet::new();

        if max_relights <= 0 {
            chunks.extend(self.pending_chunks_to_light.iter().copied());
            self.pending_chunks_to_light.clear();
        } else {
            let mut budget = max_relights;
            let pending: Vec<(i32, i32)> = self.pending_chunks_to_light.iter().copied().collect();
            for chunk in pending {
                chunks.insert(chunk);
                self.pending_chunks_to_light.remove(&chunk);
                budget -= 1;
                if budget <= 0 {
                    break;
                }
            }
        }

        // Java calls the light engine even for an empty set. That is a no-op, so it is skipped here.
        if !chunks.is_empty() {
            host.relight_chunks(dim, &chunks);
        }
    }
}

/// Body of `axiom:marker_data`: changed markers (already encoded), then the removed UUIDs.
fn encode_marker_update(changed: &[Vec<u8>], missing: &[u128]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_varint(changed.len() as i32);
    for entry in changed {
        w.write_bytes(entry);
    }
    w.write_varint(missing.len() as i32);
    for uuid in missing {
        w.write_uuid(*uuid);
    }
    w.into_bytes()
}
