//! Allocation budget for the isolated script worker, inactive in the desktop host.
//!
//! This bounds cumulative Rust allocation requests (not total process RSS). Existing
//! startup mappings and native-library allocations are outside this limit. No
//! script is evaluated unless the host has installed this allocator wrapper.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

pub const LIMIT: usize = 512 * 1024 * 1024;
pub(crate) const EXIT_BUDGET: i32 = 86;
// 0: wrapper not observed; 1: installed/inactive; 2: worker budget active.
static STATE: AtomicU8 = AtomicU8::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);

/// Install as the host binary's global allocator. Outside worker mode it forwards
/// directly to System with one relaxed flag check per allocation request.
pub struct WorkerAllocator;

fn reserve(bytes: usize) {
    let state = STATE.load(Ordering::Relaxed);
    if state != 2 {
        if state == 0 {
            let _ = STATE.compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed);
        }
        return;
    }
    if bytes > LIMIT || REQUESTED.fetch_add(bytes, Ordering::Relaxed) > LIMIT - bytes {
        // Do not allocate, unwind or run cleanup handlers inside an allocator.
        #[cfg(unix)]
        unsafe {
            unsafe extern "C" {
                fn _exit(code: std::ffi::c_int) -> !;
            }
            _exit(EXIT_BUDGET);
        }
        #[cfg(windows)]
        unsafe {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                #[link_name = "GetCurrentProcess"]
                fn current_process() -> *mut std::ffi::c_void;
                #[link_name = "TerminateProcess"]
                fn terminate_process(process: *mut std::ffi::c_void, code: u32) -> i32;
            }
            // Avoid DLL-detach callbacks/allocations while terminating from an
            // allocator. Self-termination does not return on success.
            terminate_process(current_process(), EXIT_BUDGET as u32);
            std::process::abort();
        }
        #[cfg(not(any(unix, windows)))]
        std::process::abort();
    }
}

// SAFETY: all allocation/deallocation operations retain System's layout contract.
// The only additional effect is process termination when the worker budget expires.
unsafe impl GlobalAlloc for WorkerAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        reserve(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        reserve(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        reserve(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

pub(crate) fn activate() -> bool {
    // Normal binary/test-harness startup already allocated through the wrapper.
    // Do not rely on a probe allocation which an optimizer could eliminate.
    matches!(STATE.swap(2, Ordering::Relaxed), 1 | 2)
}

#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: WorkerAllocator = WorkerAllocator;
