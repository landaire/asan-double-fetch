#![cfg_attr(feature = "no_std", no_std)]
#![cfg_attr(feature = "no_std", feature(alloc, allocator_api))]

mod memory_tracking;
mod span;

#[cfg(feature = "no_std")]
use alloc::sync::Arc;
use memory_tracking::MemoryTracker;
use once_cell::sync::OnceCell;
use rand::Rng;
use span::Span;
use span::SpanRelation;
use std::ffi::c_void;
use std::os::raw::c_int;
#[cfg(not(feature = "no_std"))]
use std::sync::Arc;

#[cfg(feature = "no_std")]
type Lock<T> = kernel::sync::Mutex<T>;
#[cfg(not(feature = "no_std"))]
type Lock<T> = std::sync::RwLock<T>;
type Address = usize;

type ThreadSafeMemoryTracker = Arc<Lock<MemoryTracker>>;

/// Global list of memory regions being tracked
static TRACKED_MEMORY_REGIONS: OnceCell<Lock<Vec<(crate::span::Span, ThreadSafeMemoryTracker)>>> =
    OnceCell::new();

/// Global list of pending memory regions that were created with `shmget()`
static SHMGET_IDS: OnceCell<std::sync::Mutex<Vec<(c_int, usize)>>> = OnceCell::new();

#[no_mangle]
pub extern "C" fn asan_remember_shm_id(id: c_int, size: usize) {
    println!("(runtime) got shm with id {:#x} and len {:#x}", id, size);
    let ids = SHMGET_IDS.get().expect("SHMGET_IDS not initialized");
    let mut ids = ids.lock().unwrap();
    ids.push((id, size));
}

#[no_mangle]
pub extern "C" fn asan_register_shmat(id: c_int, addr: *mut c_void) {
    println!("(runtime) got shmat with id {:#x} and addr {:p}", id, addr);
    let ids = SHMGET_IDS.get().expect("SHMGET_IDS not initialized");
    let mut ids = ids.lock().unwrap();
    if let Some(idx) = ids.iter().position(|(list_id, size)| *list_id == id) {
        println!("(runtime) found match for shmat");

        let (_, size) = ids.remove(idx);
        __asan_watch_shared_memory_region(addr as Address, size);
    }
}

#[no_mangle]
pub extern "C" fn __asan_shared_memory_region_init() {
    #[cfg(not(feature = "no_std"))]
    let global = Default::default();
    #[cfg(feature = "no_std")]
    let global = {
        let mut mutex = Lock::new(Default::default());
        kernel::mutex_init!(Pin::new(&mut mutex), "asan_tracked_memory_regions");
    };

    TRACKED_MEMORY_REGIONS
        .set(global)
        .expect("failed to init shared memory region global");

    SHMGET_IDS
        .set(Default::default())
        .expect("failed to SHMGET_IDS");

    println!("(runtime) shared_mem runtime initialized");
}

/// Creates a new memory tracker for the given address + its size
#[no_mangle]
pub extern "C" fn __asan_watch_shared_memory_region(addr: Address, len: usize) {
    println!(
        "(runtime) watching memory region at {:#X}, len={:#X}",
        addr, len
    );

    let span = Span::with_len(addr, len);
    let mem_regions = TRACKED_MEMORY_REGIONS
        .get()
        .expect("tracked memory regions is not initialized");

    #[cfg(not(feature = "no_std"))]
    let mut mem_regions = mem_regions.write().unwrap();
    #[cfg(feature = "linux_kasan")]
    let mut mem_regions = mem_regions.lock();

    mem_regions.push((span, Default::default()))
}

/// Destroys the memory tracker corresponding to the given address + its size
#[no_mangle]
pub extern "C" fn __asan_unwatch_shared_memory_region(addr: Address) {
    let target_span = Span::with_len(addr, 1);
    let mem_regions = TRACKED_MEMORY_REGIONS
        .get()
        .expect("tracked memory regions is not initialized");

    #[cfg(not(feature = "no_std"))]
    let mut mem_regions = mem_regions.write().unwrap();
    #[cfg(feature = "linux_kasan")]
    let mut mem_regions = mem_regions.lock();

    if let Some(idx) = mem_regions
        .iter()
        .position(|(va_range, _tracker)| target_span.relation(&va_range) != SpanRelation::None)
    {
        mem_regions.remove(idx);
    }
}

#[no_mangle]
pub extern "C" fn __asan_double_fetch_check(addr: Address, len: usize, is_write: bool) -> bool {
    let memory_tracker = get_memory_tracker(addr, len);
    if memory_tracker.is_none() {
        return false;
    }

    println!(
        "(runtime) fetch check addr: {:#X}, len: {:#X}, is_write: {:?}",
        addr, len, is_write
    );

    let memory_tracker = memory_tracker.unwrap();
    #[cfg(feature = "no_std")]
    let memory_tracker = memory_tracker.lock();

    if !is_write {
        #[cfg(not(feature = "no_std"))]
        let memory_tracker = memory_tracker.read().unwrap();

        if memory_tracker.check(addr, len).is_err() {
            // this is a double-fetch
            println!("(runtime) double-fetch detected!");
            let data: &mut [u8] =
                unsafe { std::slice::from_raw_parts_mut(std::mem::transmute(addr), len) };
            if len <= 16 {
                println!("(runtime) existing bytes: {:X?}", data);
            }

            let mut rng = rand::thread_rng();
            if rng.gen() {
                data.iter_mut().for_each(|b| *b = rng.gen());
                if len <= 16 {
                    println!("(runtime) new bytes: {:X?}", data);
                }
            }
            return false;
        }
    }

    #[cfg(not(feature = "no_std"))]
    let mut memory_tracker = memory_tracker.write().unwrap();
    memory_tracker.track_access(addr, len);

    false
}

fn get_memory_tracker(addr: Address, len: usize) -> Option<Arc<Lock<MemoryTracker>>> {
    let target_span = Span::with_len(addr, len);
    let mem_regions = TRACKED_MEMORY_REGIONS
        .get()
        .expect("tracked memory regions is not initialized");

    #[cfg(not(feature = "no_std"))]
    let mem_regions = mem_regions.write().unwrap();
    #[cfg(feature = "linux_kasan")]
    let mem_regions = mem_regions.lock();

    mem_regions.iter().find_map(|(va_range, tracker)| {
        if target_span.relation(&va_range) == SpanRelation::None {
            None
        } else {
            Some(Arc::clone(tracker))
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
