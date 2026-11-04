//! Opt-in, single-threaded release microbenchmarks; never connect to a user runtime.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

struct CountingAllocator;
static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ENABLED.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ENABLED.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(size as u64, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}

pub fn measure(name: &str, operations: usize, mut work: impl FnMut(usize)) {
    ALLOCATED.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
    let start = Instant::now();
    for index in 0..operations {
        work(index);
    }
    let elapsed = start.elapsed();
    ENABLED.store(false, Ordering::Relaxed);
    println!(
        "BENCH {}",
        serde_json::json!({
            "name": name, "operations": operations,
            "nsPerOp": elapsed.as_nanos() as f64 / operations as f64,
            "allocatedBytesPerOp": ALLOCATED.load(Ordering::Relaxed) as f64 / operations as f64,
        })
    );
}
