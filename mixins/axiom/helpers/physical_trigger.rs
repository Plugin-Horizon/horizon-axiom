fn axiom_physical_trigger(player: u128, cancel: &mut bool) {
    if crate::game::axiom::no_physical_trigger(player) {
        *cancel = true;
    }
}
