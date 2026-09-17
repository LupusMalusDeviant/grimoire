//! Contract §2 rule 9 for the generated payload decoders (Plan 0002 WP8.4): a short message that
//! claims a large element count fails before the decoder reserves memory for those elements. Lives
//! in its own test binary because it installs a global allocator that records the largest single
//! allocation of the current thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use grimoire_debug::{ProtocolError, Stats, StatsCounter, StatsScope};

thread_local! {
    // Per thread, so allocations of the test harness on other threads are not recorded.
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn record_allocation(size: usize) {
    // `try_with` because the allocator may run while thread-locals are being torn down.
    let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
}

/// Largest single allocation (in bytes) of the current thread while `work` runs.
fn largest_allocation_during<T>(work: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.with(|largest| largest.set(0));
    let result = work();
    (result, LARGEST.with(Cell::get))
}

/// Forwards to [`System`] and records the largest allocation of the current thread.
struct RecordingAllocator;

// SAFETY: every method forwards its arguments unchanged to `System`, which upholds the
// `GlobalAlloc` contract. Recording only touches a const-initialised thread-local `Cell` without a
// destructor, which never allocates and therefore cannot recurse into the allocator.
// `the_recorder_observes_a_vector_of_scopes` checks that the wrapper is installed and records.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for RecordingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        // SAFETY: the caller's guarantees for `layout` are passed through unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        // SAFETY: the caller's guarantees for `layout` are passed through unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation(new_size);
        // SAFETY: `ptr` was allocated by `System` through this wrapper with `layout`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `System` through this wrapper with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: RecordingAllocator = RecordingAllocator;

/// `Stats` bytes up to (not including) `scopes`: 56 bytes of zeroed frame values.
fn stats_prefix() -> Vec<u8> {
    vec![0u8; 8 + 8 + 4 + 4 + 8 + 4 + 8 + 4 + 8]
}

const SCOPES_CAPACITY_BYTES: usize = 64 * std::mem::size_of::<StatsScope>();
const COUNTERS_CAPACITY_BYTES: usize = 64 * std::mem::size_of::<StatsCounter>();

#[test]
fn the_recorder_observes_a_vector_of_scopes() {
    let (scopes, largest) =
        largest_allocation_during(|| Vec::<StatsScope>::with_capacity(64).capacity());
    assert!(scopes >= 64);
    assert!(largest >= SCOPES_CAPACITY_BYTES, "largest = {largest}");
}

#[test]
fn a_short_stats_message_claiming_64_scopes_allocates_no_scope_vector() {
    let mut bytes = stats_prefix();
    bytes.extend_from_slice(&64u32.to_le_bytes()); // scopes: count 64, no elements follow

    let (result, largest) = largest_allocation_during(|| Stats::decode(&bytes));
    assert!(matches!(result, Err(ProtocolError::UnexpectedEnd { .. })));
    assert!(
        largest < SCOPES_CAPACITY_BYTES,
        "decode allocated {largest} bytes, a vector of 64 scopes takes {SCOPES_CAPACITY_BYTES}"
    );
}

#[test]
fn a_short_stats_message_claiming_64_counters_allocates_no_counter_vector() {
    let mut bytes = stats_prefix();
    bytes.extend_from_slice(&0u32.to_le_bytes()); // scopes: none
    bytes.extend_from_slice(&64u32.to_le_bytes()); // counters: count 64, 12 bytes follow
    bytes.extend_from_slice(&[0u8; 12]);

    let (result, largest) = largest_allocation_during(|| Stats::decode(&bytes));
    assert!(matches!(result, Err(ProtocolError::UnexpectedEnd { .. })));
    assert!(
        largest < COUNTERS_CAPACITY_BYTES,
        "decode allocated {largest} bytes, a vector of 64 counters takes {COUNTERS_CAPACITY_BYTES}"
    );
}
