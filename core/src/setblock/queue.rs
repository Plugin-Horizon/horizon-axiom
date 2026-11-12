// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): operations/OperationQueue.java,
// operations/PendingOperation.java

//! Per-dimension FIFO of long-running block operations, ticked from the server tick.
//! Java guards this with locks for off-thread adds. Here everything runs on the server thread, so the locks are gone.

use std::collections::{BTreeMap, VecDeque};

use super::host::SetBlockHost;
use super::SetBlockError;

/// Java `PendingOperation`.
pub trait PendingOperation {
    fn is_finished(&self) -> bool;
    fn tick(&mut self, host: &mut dyn SetBlockHost, dim: &str) -> Result<(), SetBlockError>;
    /// Player who started the operation; kicked when the operation fails.
    fn executor(&self) -> u128;
}

#[derive(Default)]
pub struct OperationQueue {
    new_ops: BTreeMap<String, VecDeque<Box<dyn PendingOperation>>>,
    ops: BTreeMap<String, VecDeque<Box<dyn PendingOperation>>>,
}

impl OperationQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Java `OperationQueue.add(level, op)`. When nothing is queued for the dimension, the first tick runs
    /// immediately, so a small operation finishes inside the call.
    pub fn add(&mut self, host: &mut dyn SetBlockHost, dim: &str, mut op: Box<dyn PendingOperation>) -> Result<(), SetBlockError> {
        let new_empty = self.new_ops.get(dim).map_or(true, |q| q.is_empty());
        if new_empty {
            let current_empty = self.ops.get(dim).map_or(true, |q| q.is_empty());
            if current_empty {
                op.tick(host, dim)?;
                if op.is_finished() {
                    return Ok(());
                }
            }
        }
        self.new_ops.entry(dim.to_string()).or_default().push_back(op);
        Ok(())
    }

    /// Java `OperationQueue.tick()`. Only the head of each dimension's queue advances; a finished head is
    /// removed and the next one ticks in the same call. A failing operation kicks its executor and is dropped.
    pub fn tick(&mut self, host: &mut dyn SetBlockHost) {
        for (dim, mut new_list) in std::mem::take(&mut self.new_ops) {
            self.ops.entry(dim).or_default().append(&mut new_list);
        }

        let dims: Vec<String> = self.ops.keys().cloned().collect();
        for dim in dims {
            let Some(mut list) = self.ops.remove(&dim) else { continue };
            while let Some(op) = list.front_mut() {
                match op.tick(host, &dim) {
                    Ok(()) => {
                        if op.is_finished() {
                            list.pop_front();
                        } else {
                            break;
                        }
                    }
                    Err(e) => {
                        let executor = op.executor();
                        host.kick(executor, &format!("An error occurred while processing operation: {e}"));
                        list.pop_front();
                    }
                }
            }
            if !list.is_empty() {
                self.ops.insert(dim, list);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ops.values().all(|q| q.is_empty()) && self.new_ops.values().all(|q| q.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setblock::test_support::FakeHost;

    /// Finishes after `ticks_needed` ticks, or fails on the first tick when `fail` is set.
    struct Counting {
        player: u128,
        ticks_needed: u32,
        ticks: u32,
        fail: bool,
    }

    impl PendingOperation for Counting {
        fn is_finished(&self) -> bool {
            self.ticks >= self.ticks_needed
        }
        fn tick(&mut self, _host: &mut dyn SetBlockHost, _dim: &str) -> Result<(), SetBlockError> {
            if self.fail {
                return Err(SetBlockError::UnknownHand(9));
            }
            self.ticks += 1;
            Ok(())
        }
        fn executor(&self) -> u128 {
            self.player
        }
    }

    #[test]
    fn small_operation_finishes_inside_add() {
        let mut host = FakeHost::new("d");
        let mut q = OperationQueue::new();
        q.add(&mut host, "d", Box::new(Counting { player: 1, ticks_needed: 1, ticks: 0, fail: false })).unwrap();
        assert!(q.is_empty());
    }

    #[test]
    fn long_operation_advances_one_tick_at_a_time_in_order() {
        let mut host = FakeHost::new("d");
        let mut q = OperationQueue::new();
        q.add(&mut host, "d", Box::new(Counting { player: 1, ticks_needed: 2, ticks: 0, fail: false })).unwrap();
        q.add(&mut host, "d", Box::new(Counting { player: 2, ticks_needed: 1, ticks: 0, fail: false })).unwrap();
        assert!(!q.is_empty());
        q.tick(&mut host);
        assert!(q.is_empty());
    }

    #[test]
    fn failing_operation_kicks_its_executor_and_is_dropped() {
        let mut host = FakeHost::new("d");
        // A failure on the immediate tick is returned from add, not kicked.
        let mut q = OperationQueue::new();
        assert!(q.add(&mut host, "d", Box::new(Counting { player: 7, ticks_needed: 5, ticks: 0, fail: true })).is_err());
        assert!(q.is_empty());
        assert!(host.kicks.is_empty());

        // A failure on a queued tick kicks the executor and the next operation still runs.
        let mut q = OperationQueue::new();
        q.add(&mut host, "d", Box::new(Counting { player: 1, ticks_needed: 2, ticks: 0, fail: false })).unwrap();
        q.add(&mut host, "d", Box::new(Counting { player: 7, ticks_needed: 1, ticks: 0, fail: true })).unwrap();
        q.tick(&mut host);
        assert!(q.is_empty());
        assert_eq!(host.kicks.len(), 1);
        assert_eq!(host.kicks[0].0, 7);
    }
}
