// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): listener/NoPhysicalTriggerListener.java,
// AxiomPaper.setNoPhysicalTrigger / isNoPhysicalTrigger

//! "No physical trigger" mode: players in the set cannot trigger pressure plates, tripwires and similar.

use std::collections::HashSet;

#[derive(Default)]
pub struct NoPhysicalTrigger {
    players: HashSet<u128>,
}

impl NoPhysicalTrigger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, player: u128, enabled: bool) {
        if enabled {
            self.players.insert(player);
        } else {
            self.players.remove(&player);
        }
    }

    pub fn is_enabled(&self, player: u128) -> bool {
        self.players.contains(&player)
    }

    /// Java `onGenericGameEvent`: cancel when the entity is a player in the set. `entity` is `None` for
    /// non-player entities.
    pub fn cancels_generic_game_event(&self, entity: Option<u128>) -> bool {
        entity.map_or(false, |p| self.is_enabled(p))
    }

    /// Java `onPlayerInteract`: cancel physical (pressure plate) interactions for a player in the set.
    pub fn cancels_physical_interact(&self, player: u128, physical_action: bool) -> bool {
        physical_action && self.is_enabled(player)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listed_players_are_cancelled() {
        let mut n = NoPhysicalTrigger::new();
        n.set(7, true);
        assert!(n.cancels_generic_game_event(Some(7)));
        assert!(!n.cancels_generic_game_event(Some(8)));
        assert!(!n.cancels_generic_game_event(None));
        assert!(n.cancels_physical_interact(7, true));
        assert!(!n.cancels_physical_interact(7, false));
        n.set(7, false);
        assert!(!n.cancels_physical_interact(7, true));
    }
}
