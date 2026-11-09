// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): AxiomPaper.consumeDispatchSends

//! Per-player budget for block buffer sends (rate limit). Values are kept in "send units x 20" as in Java.

use std::collections::HashMap;

use super::host::SetBlockHost;

pub const TOO_FAST_KICK: &str = "You are sending updates too fast!";

#[derive(Default)]
pub struct DispatchBudget {
    available: HashMap<u128, i64>,
}

impl DispatchBudget {
    pub fn new() -> Self {
        Self::default()
    }

    /// Java `consumeDispatchSends(player, sends, clientAvailableDispatchSends)`. Returns false after kicking
    /// the player when the budget is exhausted.
    pub fn consume(&mut self, host: &mut dyn SetBlockHost, player: u128, sends: i64, client_available: i64) -> bool {
        let allowed = host.allowed_dispatch_sends_per_second(player);
        let mut current = self.available.get(&player).copied().unwrap_or(allowed * 20);
        current -= sends * 20;
        current = current.min(client_available * 20);
        self.available.insert(player, current);

        if current < -allowed * 20 {
            host.kick(player, TOO_FAST_KICK);
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setblock::test_support::FakeHost;

    #[test]
    fn budget_kicks_after_the_limit_is_exceeded() {
        let mut host = FakeHost::new("d");
        let mut budget = DispatchBudget::new();
        // allowed = 1000 per second (host), client has 0 available, so the budget starts at 0.
        assert!(budget.consume(&mut host, 1, 0, 0));
        assert!(budget.consume(&mut host, 1, 1000, 0));
        assert!(host.kicks.is_empty());
        assert!(!budget.consume(&mut host, 1, 1, 0));
        assert_eq!(host.kicks, vec![(1, TOO_FAST_KICK.to_string())]);
    }
}
