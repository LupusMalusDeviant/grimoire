//! Blocks over data outside archetypes (contract §7.1, additive P1 extension).
//!
//! A [`crate::World::par_blocks`] query is not the only data a system may want to split into
//! fixed, executor-independent ranges: resources with their own SoA columns (for example a
//! bullet pool) follow the same block rule. [`slice_block_ranges`] gives the boundaries and
//! [`run_blocks`] runs arbitrary per-block work through an [`Executor`], generalising the
//! internal helper queries already use.

use std::any::Any;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use crate::executor::Executor;
use crate::query::QUERY_BLOCK_SIZE;

/// Consecutive index ranges `[0, B), [B, 2B), ...` with `B = `[`QUERY_BLOCK_SIZE`]; the last range
/// is shorter than `B` unless `len` is a multiple of it. `len == 0` yields no ranges.
///
/// The position of a range in this sequence (`start / QUERY_BLOCK_SIZE`) is its block index and
/// depends only on `len`, never on an executor or thread count.
pub fn slice_block_ranges(len: usize) -> impl ExactSizeIterator<Item = Range<usize>> {
    let block_count = len.div_ceil(QUERY_BLOCK_SIZE);
    (0..block_count).map(move |index| {
        let start = index * QUERY_BLOCK_SIZE;
        let end = (start + QUERY_BLOCK_SIZE).min(len);
        start..end
    })
}

struct Slot<B, T> {
    block: Option<B>,
    output: Option<T>,
    panic: Option<Box<dyn Any + Send>>,
}

/// Runs `f` over every element of `blocks` through `executor` and returns the results in the same
/// order (result `i` belongs to `blocks[i]`).
///
/// With at most one block, `f` runs inline without the executor. Otherwise every block becomes a
/// task that catches its own panic; after every task has finished, the panic with the lowest
/// index is resumed (mirrors the internal block runner behind
/// [`crate::World::par_blocks`]/[`crate::World::par_blocks_mut`], contract §7.1).
///
/// # Panics
///
/// If `f` panics for one or more blocks, once every block has finished, the panic of the block
/// with the lowest index is resumed.
pub fn run_blocks<B: Send, T: Send>(
    executor: &dyn Executor,
    blocks: Vec<B>,
    f: impl Fn(B) -> T + Sync,
) -> Vec<T> {
    if blocks.len() <= 1 {
        return blocks.into_iter().map(f).collect();
    }
    // A shared reference is `Copy` and, because `F: Sync`, `Send`, so every per-block closure
    // below can move its own copy of it into a `move` closure without requiring `F: Send`.
    let f = &f;
    // The access context of the calling thread follows the blocks onto every executing thread
    // (contract §7.1), matching the query block runner.
    #[cfg(debug_assertions)]
    let context = crate::debug_access::current();
    let mut slots: Vec<Slot<B, T>> = blocks
        .into_iter()
        .map(|block| Slot {
            block: Some(block),
            output: None,
            panic: None,
        })
        .collect();
    {
        let mut tasks: Vec<_> = slots
            .iter_mut()
            .map(|slot| {
                #[cfg(debug_assertions)]
                let context = context.clone();
                move || {
                    #[cfg(debug_assertions)]
                    let _guard = crate::debug_access::enter(context.clone());
                    if let Some(block) = slot.block.take() {
                        match catch_unwind(AssertUnwindSafe(|| f(block))) {
                            Ok(output) => slot.output = Some(output),
                            Err(payload) => slot.panic = Some(payload),
                        }
                    }
                }
            })
            .collect();
        let mut refs: Vec<&mut (dyn FnMut() + Send)> = tasks
            .iter_mut()
            .map(|task| task as &mut (dyn FnMut() + Send))
            .collect();
        executor.run(&mut refs);
    }
    let mut outputs = Vec::with_capacity(slots.len());
    let mut first_panic = None;
    for (index, slot) in slots.into_iter().enumerate() {
        if let Some(payload) = slot.panic {
            first_panic.get_or_insert(payload);
        } else if let Some(output) = slot.output {
            outputs.push(output);
        } else if first_panic.is_none() {
            panic!("executor did not run block task {index}");
        }
    }
    if let Some(payload) = first_panic {
        resume_unwind(payload);
    }
    outputs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{PermutedExecutor, SequentialExecutor};

    #[test]
    fn ranges_cover_len_with_a_shorter_last_range() {
        let ranges: Vec<_> = slice_block_ranges(QUERY_BLOCK_SIZE * 2 + 5).collect();
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0], 0..QUERY_BLOCK_SIZE);
        assert_eq!(ranges[1], QUERY_BLOCK_SIZE..QUERY_BLOCK_SIZE * 2);
        assert_eq!(ranges[2], QUERY_BLOCK_SIZE * 2..QUERY_BLOCK_SIZE * 2 + 5);
    }

    #[test]
    fn zero_len_yields_no_ranges() {
        assert_eq!(slice_block_ranges(0).len(), 0);
    }

    #[test]
    fn run_blocks_preserves_order_regardless_of_executor() {
        let blocks: Vec<usize> = (0..10).collect();
        let sequential = run_blocks(&SequentialExecutor, blocks.clone(), |b| b * 2);
        let permuted = run_blocks(&PermutedExecutor::new(7), blocks, |b| b * 2);
        let expected: Vec<usize> = (0..10).map(|b| b * 2).collect();
        assert_eq!(sequential, expected);
        assert_eq!(permuted, expected);
    }

    #[test]
    #[should_panic(expected = "boom")]
    fn run_blocks_resumes_lowest_index_panic() {
        let blocks = vec![0usize, 1, 2, 3];
        run_blocks(&SequentialExecutor, blocks, |b| {
            if b == 2 {
                panic!("boom");
            }
            b
        });
    }
}
