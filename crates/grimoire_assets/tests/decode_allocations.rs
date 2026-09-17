//! Contract §2 rule 9 for the generated pack manifest decoder (Plan 0002 WP8.4): a short manifest
//! that claims a large entry count fails before the decoder reserves memory for the paths. Lives in
//! its own test binary because it installs a global allocator that records the largest single
//! allocation of the current thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use grimoire_assets::{MAX_ENTRIES, PackManifestBody, PackManifestV1Error};

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
// `the_recorder_observes_a_vector_of_paths` checks that the wrapper is installed and records.
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

const PATHS_CAPACITY_BYTES: usize = MAX_ENTRIES as usize * std::mem::size_of::<String>();

#[test]
fn the_recorder_observes_a_vector_of_paths() {
    let (paths, largest) =
        largest_allocation_during(|| Vec::<String>::with_capacity(MAX_ENTRIES as usize).capacity());
    assert!(paths >= MAX_ENTRIES as usize);
    assert!(largest >= PATHS_CAPACITY_BYTES, "largest = {largest}");
}

#[test]
fn a_short_manifest_claiming_the_maximum_entry_count_allocates_no_path_vector() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u32.to_le_bytes()); // manifest_version
    bytes.extend_from_slice(&0u16.to_le_bytes()); // compiler: empty
    bytes.extend_from_slice(&0u16.to_le_bytes()); // compiler_version: empty
    bytes.extend_from_slice(&MAX_ENTRIES.to_le_bytes()); // entry_count, no paths follow
    bytes.extend_from_slice(&[0u8; 4]); // 4 bytes follow, far fewer than 65 536 paths need

    let (result, largest) = largest_allocation_during(|| PackManifestBody::decode(&bytes));
    assert!(matches!(
        result,
        Err(PackManifestV1Error::UnexpectedEnd { .. })
    ));
    assert!(
        largest < PATHS_CAPACITY_BYTES,
        "decode allocated {largest} bytes, a vector of {MAX_ENTRIES} paths takes {PATHS_CAPACITY_BYTES}"
    );
}
