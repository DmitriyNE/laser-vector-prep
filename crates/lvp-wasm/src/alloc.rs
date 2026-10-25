//! The global allocator of the wasm build.
//!
//! std's wasm allocator guards its one dlmalloc heap with a spin lock: every allocation of
//! every thread queues on it, and with more threads than free cores the spinning starves
//! the thread holding the lock (ten threads evaluated a board panel five times slower than
//! two). Here small blocks are cached by size class in [`SLOTS`] slots and move to and from
//! the shared heap in batches, and the heap's lock sleeps instead of spinning.
//!
//! A thread picks a slot by the address of its stack, not by thread-local storage:
//! wasm-bindgen allocates a new thread's stack and TLS block with this allocator before
//! that thread's TLS exists. A slot is only ever *tried*; when another thread holds it the
//! call goes to the heap directly. Any slot may hold blocks freed by any thread, since all
//! blocks of a class have the same size and come from the same heap.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cache slots. Thread stacks are a megabyte or more apart, so threads mostly land in
/// different slots.
pub const SLOTS: usize = 32;
/// Largest cached block (bytes); larger ones always come from the heap.
const MAX_SMALL: usize = 2048;
/// Alignment every cached block has (dlmalloc's on wasm32; the class sizes are multiples).
const MIN_ALIGN: usize = 8;
/// Bytes one free list may hold before half of it goes back to the heap.
const LIST_BYTES: usize = 16 * 1024;
/// Size classes: steps of 8 bytes up to 64, then four steps per doubling up to 2 KiB.
const CLASSES: usize = 8 + 4 * 5;

const fn class_sizes() -> [usize; CLASSES] {
    let mut out = [0; CLASSES];
    let mut i = 0;
    while i < 8 {
        out[i] = 8 * (i + 1);
        i += 1;
    }
    let mut band = 64;
    while i < CLASSES {
        let mut k = 1;
        while k <= 4 {
            out[i] = band + band / 4 * k;
            i += 1;
            k += 1;
        }
        band *= 2;
    }
    out
}

const CLASS_SIZE: [usize; CLASSES] = class_sizes();

/// The class of a block of `size` bytes (1 ..= `MAX_SMALL`): the smallest that fits it.
fn class_of(size: usize) -> usize {
    if size <= 64 {
        return (size.max(1) - 1) / 8;
    }
    // size in (2^k, 2^(k+1)], k >= 6, split into four steps of 2^k / 4.
    let k = usize::BITS - 1 - (size - 1).leading_zeros();
    let band = 1usize << k;
    let step = band / 4;
    8 + 4 * (k as usize - 6) + (size - band).div_ceil(step) - 1
}

/// The class of a layout, if its blocks are cached.
fn cached_class(layout: Layout) -> Option<usize> {
    (layout.size() <= MAX_SMALL && layout.align() <= MIN_ALIGN).then(|| class_of(layout.size()))
}

/// The shared heap behind the caches; it does its own locking.
///
/// # Safety
/// As for [`GlobalAlloc`]; the batch methods move whole blocks of one size.
pub unsafe trait Heap: GlobalAlloc {
    /// Allocate `n` blocks of `size` bytes (aligned to `MIN_ALIGN`) onto `list`.
    unsafe fn alloc_batch(&self, size: usize, n: usize, list: &mut FreeList) {
        let layout = Layout::from_size_align(size, MIN_ALIGN).expect("class layout");
        for _ in 0..n {
            let p = unsafe { self.alloc(layout) };
            if p.is_null() {
                return;
            }
            unsafe { list.push(p) };
        }
    }

    /// Free every block of `list` (each `size` bytes).
    unsafe fn free_batch(&self, size: usize, mut list: FreeList) {
        let layout = Layout::from_size_align(size, MIN_ALIGN).expect("class layout");
        while let Some(p) = unsafe { list.pop() } {
            unsafe { self.dealloc(p, layout) };
        }
    }
}

/// An intrusive singly linked list of free blocks: each block stores the next pointer.
pub struct FreeList {
    head: *mut u8,
    len: usize,
}

impl FreeList {
    const fn new() -> FreeList {
        FreeList {
            head: ptr::null_mut(),
            len: 0,
        }
    }

    /// # Safety
    /// `p` is a free block of at least pointer size, owned by the list from now on.
    unsafe fn push(&mut self, p: *mut u8) {
        unsafe { (p as *mut *mut u8).write(self.head) };
        self.head = p;
        self.len += 1;
    }

    unsafe fn pop(&mut self) -> Option<*mut u8> {
        if self.head.is_null() {
            return None;
        }
        let p = self.head;
        self.head = unsafe { (p as *mut *mut u8).read() };
        self.len -= 1;
        Some(p)
    }

    /// The first `n` blocks as a list of their own.
    unsafe fn split_off(&mut self, n: usize) -> FreeList {
        let mut out = FreeList::new();
        for _ in 0..n {
            match unsafe { self.pop() } {
                Some(p) => unsafe { out.push(p) },
                None => break,
            }
        }
        out
    }
}

struct Slot {
    busy: AtomicBool,
    lists: UnsafeCell<[FreeList; CLASSES]>,
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            busy: AtomicBool::new(false),
            lists: UnsafeCell::new([const { FreeList::new() }; CLASSES]),
        }
    }

    /// The slot's lists, if no other thread is using them.
    fn try_lock(&self) -> Option<SlotGuard<'_>> {
        (!self.busy.swap(true, Ordering::Acquire)).then_some(SlotGuard(self))
    }
}

struct SlotGuard<'a>(&'a Slot);

impl SlotGuard<'_> {
    fn list(&mut self, class: usize) -> &mut FreeList {
        // Only the guard's holder touches the lists.
        unsafe { &mut (*self.0.lists.get())[class] }
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

/// Blocks a free list of `class` may hold.
fn limit(class: usize) -> usize {
    (LIST_BYTES / CLASS_SIZE[class]).max(8)
}

/// Small blocks cached per slot and class in front of a shared [`Heap`].
pub struct SlotAllocator<H> {
    heap: H,
    slots: [Slot; SLOTS],
}

// The slots are only touched by the holder of their flag; the heap locks itself.
unsafe impl<H: Sync> Sync for SlotAllocator<H> {}

impl<H: Heap> SlotAllocator<H> {
    pub const fn new(heap: H) -> Self {
        SlotAllocator {
            heap,
            slots: [const { Slot::new() }; SLOTS],
        }
    }

    /// The slot of the calling thread: by the address of its stack.
    fn slot(&self) -> &Slot {
        let marker = 0u8;
        let at = std::hint::black_box(ptr::addr_of!(marker)) as usize;
        &self.slots[(at >> 20) % SLOTS]
    }
}

unsafe impl<H: Heap> GlobalAlloc for SlotAllocator<H> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(class) = cached_class(layout) else {
            return unsafe { self.heap.alloc(layout) };
        };
        let size = CLASS_SIZE[class];
        if let Some(mut slot) = self.slot().try_lock() {
            let list = slot.list(class);
            if list.len == 0 {
                unsafe { self.heap.alloc_batch(size, limit(class) / 2, list) };
            }
            if let Some(p) = unsafe { list.pop() } {
                return p;
            }
        }
        unsafe {
            self.heap
                .alloc(Layout::from_size_align_unchecked(size, MIN_ALIGN))
        }
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let Some(class) = cached_class(layout) else {
            return unsafe { self.heap.dealloc(p, layout) };
        };
        let size = CLASS_SIZE[class];
        if let Some(mut slot) = self.slot().try_lock() {
            let list = slot.list(class);
            unsafe { list.push(p) };
            if list.len > limit(class) {
                let surplus = unsafe { list.split_off(limit(class) / 2) };
                drop(slot);
                unsafe { self.heap.free_batch(size, surplus) };
            }
            return;
        }
        unsafe {
            self.heap
                .dealloc(p, Layout::from_size_align_unchecked(size, MIN_ALIGN))
        }
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        match (cached_class(layout), cached_class(new_layout)) {
            (Some(a), Some(b)) if a == b => p,
            (None, None) => unsafe { self.heap.realloc(p, layout, new_size) },
            _ => {
                let q = unsafe { self.alloc(new_layout) };
                if !q.is_null() {
                    unsafe {
                        ptr::copy_nonoverlapping(p, q, layout.size().min(new_size));
                        self.dealloc(p, layout);
                    }
                }
                q
            }
        }
    }
}

/// The shared dlmalloc heap of the wasm module (the same allocator std uses), behind a lock
/// that waits with `memory.atomic.wait32` rather than spinning. Waiting is not allowed on a
/// browser's main thread, but this module only runs in workers (and in Node, which allows
/// it); a thread that never contends never waits.
#[cfg(target_arch = "wasm32")]
pub struct SharedHeap {
    /// 0 free, 1 held, 2 held with waiters.
    lock: std::sync::atomic::AtomicI32,
    heap: UnsafeCell<dlmalloc::Dlmalloc>,
}

#[cfg(target_arch = "wasm32")]
unsafe impl Sync for SharedHeap {}

/// Attempts to take the heap's lock before going to sleep on it.
#[cfg(target_arch = "wasm32")]
const HEAP_SPINS: u32 = 200;

#[cfg(target_arch = "wasm32")]
impl SharedHeap {
    pub const fn new() -> SharedHeap {
        SharedHeap {
            lock: std::sync::atomic::AtomicI32::new(0),
            heap: UnsafeCell::new(dlmalloc::Dlmalloc::new()),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut dlmalloc::Dlmalloc) -> R) -> R {
        use core::arch::wasm32::{memory_atomic_notify, memory_atomic_wait32};
        use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
        let acquired = || self.lock.compare_exchange(0, 1, Acquire, Relaxed).is_ok();
        // The heap is held for a few hundred instructions at a time: spin briefly before
        // sleeping, since waking a sleeper takes a trip through the scheduler.
        if !(0..HEAP_SPINS).any(|_| {
            let free = self.lock.load(Relaxed) == 0 && acquired();
            if !free {
                std::hint::spin_loop();
            }
            free
        }) {
            while self.lock.swap(2, Acquire) != 0 {
                unsafe { memory_atomic_wait32(self.lock.as_ptr(), 2, -1) };
            }
        }
        let r = f(unsafe { &mut *self.heap.get() });
        if self.lock.swap(0, Release) == 2 {
            unsafe { memory_atomic_notify(self.lock.as_ptr(), 1) };
        }
        r
    }
}

#[cfg(target_arch = "wasm32")]
unsafe impl GlobalAlloc for SharedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.with(|h| unsafe { h.malloc(layout.size(), layout.align()) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.with(|h| unsafe { h.calloc(layout.size(), layout.align()) })
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        self.with(|h| unsafe { h.free(p, layout.size(), layout.align()) })
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.with(|h| unsafe { h.realloc(p, layout.size(), layout.align(), new_size) })
    }
}

#[cfg(target_arch = "wasm32")]
unsafe impl Heap for SharedHeap {
    unsafe fn alloc_batch(&self, size: usize, n: usize, list: &mut FreeList) {
        self.with(|h| {
            for _ in 0..n {
                let p = unsafe { h.malloc(size, MIN_ALIGN) };
                if p.is_null() {
                    return;
                }
                unsafe { list.push(p) };
            }
        })
    }

    unsafe fn free_batch(&self, size: usize, mut list: FreeList) {
        self.with(|h| {
            while let Some(p) = unsafe { list.pop() } {
                unsafe { h.free(p, size, MIN_ALIGN) };
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::System;

    unsafe impl Heap for System {}

    #[test]
    fn classes_fit_and_grow() {
        for size in 1..=MAX_SMALL {
            let c = class_of(size);
            assert!(CLASS_SIZE[c] >= size, "size {size} class {c}");
            assert!(
                c == 0 || CLASS_SIZE[c - 1] < size,
                "size {size} not in its smallest class"
            );
            assert!(CLASS_SIZE[c].is_multiple_of(MIN_ALIGN));
        }
        assert_eq!(class_of(MAX_SMALL), CLASSES - 1);
        assert!(CLASS_SIZE.windows(2).all(|w| w[0] < w[1]));
        // Never more than a quarter wasted beyond 64 bytes.
        for size in 65..=MAX_SMALL {
            assert!(
                CLASS_SIZE[class_of(size)] * 4 <= size * 5 + 4 * 16,
                "size {size}"
            );
        }
    }

    /// Threads allocate, grow, shrink and free blocks of every size, many freed by another
    /// thread than the one that allocated them; every block keeps its contents.
    #[test]
    fn blocks_keep_their_contents_across_threads() {
        static A: SlotAllocator<System> = SlotAllocator::new(System);
        let (tx, rx) = std::sync::mpsc::channel::<Vec<(usize, usize, usize)>>();
        let rx = std::sync::Mutex::new(rx);
        std::thread::scope(|s| {
            for t in 0..8usize {
                let tx = tx.clone();
                let rx = &rx;
                s.spawn(move || {
                    let mut rng = 0x9e37_79b9_7f4a_7c15u64 ^ (t as u64 + 1);
                    let mut next = move || {
                        rng ^= rng << 13;
                        rng ^= rng >> 7;
                        rng ^= rng << 17;
                        rng
                    };
                    let fill = |p: *mut u8, size: usize, tag: usize| {
                        for i in 0..size {
                            unsafe { p.add(i).write((tag + i) as u8) };
                        }
                    };
                    let check = |p: *mut u8, size: usize, tag: usize| {
                        for i in 0..size {
                            assert_eq!(unsafe { p.add(i).read() }, (tag + i) as u8);
                        }
                    };
                    for round in 0..40 {
                        let mut live: Vec<(usize, usize, usize)> = Vec::new();
                        for k in 0..400 {
                            let size = 1 + (next() as usize) % 3000;
                            let align = [1, 2, 4, 8, 16][(next() as usize) % 5];
                            let layout = Layout::from_size_align(size, align).unwrap();
                            let p = unsafe { A.alloc(layout) };
                            assert!(!p.is_null() && (p as usize).is_multiple_of(align));
                            let tag = t * 31 + k;
                            fill(p, size, tag);
                            live.push((
                                p as usize,
                                size * 16
                                    + [1, 2, 4, 8, 16].iter().position(|a| *a == align).unwrap(),
                                tag,
                            ));
                        }
                        // Grow or shrink a third of them.
                        for e in live.iter_mut().step_by(3) {
                            let (size, align) = (e.1 / 16, [1, 2, 4, 8, 16][e.1 % 16]);
                            let new_size = 1 + (next() as usize) % 3000;
                            let layout = Layout::from_size_align(size, align).unwrap();
                            let q = unsafe { A.realloc(e.0 as *mut u8, layout, new_size) };
                            assert!(!q.is_null());
                            check(q, size.min(new_size), e.2);
                            fill(q, new_size, e.2);
                            *e = (q as usize, new_size * 16 + e.1 % 16, e.2);
                        }
                        // Hand half to another thread, free the rest here.
                        let mine = live.split_off(live.len() / 2);
                        tx.send(live).unwrap();
                        let theirs = if round % 2 == 0 {
                            rx.lock().unwrap().try_recv().ok()
                        } else {
                            None
                        };
                        for (p, sa, tag) in mine.into_iter().chain(theirs.into_iter().flatten()) {
                            let (size, align) = (sa / 16, [1, 2, 4, 8, 16][sa % 16]);
                            check(p as *mut u8, size, tag);
                            unsafe {
                                A.dealloc(
                                    p as *mut u8,
                                    Layout::from_size_align(size, align).unwrap(),
                                )
                            };
                        }
                    }
                });
            }
        });
        drop(tx);
        for batch in rx.into_inner().unwrap() {
            for (p, sa, _) in batch {
                let (size, align) = (sa / 16, [1, 2, 4, 8, 16][sa % 16]);
                unsafe { A.dealloc(p as *mut u8, Layout::from_size_align(size, align).unwrap()) };
            }
        }
    }
}
