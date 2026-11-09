fn axiom_payload(player: u128, channel: &str, data: &[u8]) {
    // native_custom_payload_from_client: every channel outside horizon:. Axiom's are axiom:*; queued for the tick.
    if channel.starts_with("axiom:") {
        crate::game::axiom::payload(player, channel, data);
    }
}
