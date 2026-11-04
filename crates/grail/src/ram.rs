//! A machine's RAM, backed lazily (spec section 6, "RAM"): the whole size is
//! reserved in the host's address space when the machine is made, and host memory
//! is committed a piece at a time, the first time the guest (or a device's DMA, or
//! the loader) touches it. A 1 TiB machine costs the host what its guest uses, as
//! with QEMU.
//!
//! [`Ram`] is a byte slice to everyone else (`Deref<Target = [u8]>`): the bus, the
//! devices' DMA and the tests index it as they did the `Vec<u8>` it replaces. What
//! happens on a first touch is the host's business:
//!
//! - **Linux:** an anonymous private mapping with `MAP_NORESERVE`. The kernel already
//!   works this way: the mapping is valid from the start, a page reads as zeros until
//!   it is written, and a host page is found for it on first touch. Nothing more to do.
//! - **Windows:** memory that is only reserved (`VirtualAlloc` with `MEM_RESERVE`)
//!   faults when touched, and committing all of it up front would charge the whole
//!   size against the system's commit limit, which a large machine can pass. So the
//!   reservation is registered, and a vectored exception handler, installed once,
//!   catches the access violation of a first touch inside a registered region,
//!   commits the [`COMMIT_CHUNK`] around it (zero-filled, as committed memory always
//!   is) and lets the instruction run again: the same demand paging, done by hand.
//!   Any other access violation (a null pointer, a stack overflow's guard page) is
//!   passed on to the next handler as if this one were not there.
//! - **Elsewhere:** a plain zeroed allocation, committed at once.
//!
//! One rule follows from the Windows side: the first touch of a page must come from
//! this process's own code. When the host's kernel writes into a buffer for a system
//! call (a file read straight into RAM, for a disk's DMA), a page it finds only
//! reserved makes the call fail rather than fault. So whatever hands a piece of RAM to
//! host I/O calls [`touch`] on it first.
//!
//! Every function of the Windows side's FFI is declared by hand (CLAUDE.md: std only).

use std::ops::{Deref, DerefMut};

/// How much the Windows side commits on a first touch: 64 KiB, the granularity of
/// Windows' own allocations, so a guest that sweeps memory takes one fault per 16
/// pages.
pub const COMMIT_CHUNK: usize = 64 * 1024;

/// RAM of `len` bytes, all zero until written.
pub struct Ram {
    ptr: *mut u8,
    len: usize,
    #[cfg(windows)]
    region: &'static windows::Region,
    #[cfg(not(any(windows, target_os = "linux")))]
    vec: Vec<u8>,
}

// The memory is owned by this value and reached only through it (`&` and `&mut`
// borrows of the slice): it moves between threads as a `Vec<u8>` would.
unsafe impl Send for Ram {}
unsafe impl Sync for Ram {}

impl Ram {
    /// Reserves `len` bytes (not zero). An error when the host cannot reserve that much
    /// address space: the size is refused when the machine is made, not when the guest
    /// first runs into it.
    pub fn new(len: usize) -> Result<Ram, String> {
        assert!(len > 0, "RAM of 0 bytes");
        let refused = |why: String| format!("cannot reserve {len:#x} bytes of RAM on this host: {why}");
        #[cfg(target_os = "linux")]
        {
            let ptr = linux::reserve(len).map_err(refused)?;
            Ok(Ram { ptr, len })
        }
        #[cfg(windows)]
        {
            let (ptr, region) = windows::reserve(len).map_err(refused)?;
            Ok(Ram { ptr, len, region })
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let mut vec = Vec::new();
            vec.try_reserve_exact(len).map_err(|e| refused(e.to_string()))?;
            vec.resize(len, 0);
            let ptr = vec.as_mut_ptr();
            Ok(Ram { ptr, len, vec })
        }
    }
}

/// Makes every page of `buf`, a piece of RAM, backed before host I/O uses it (see the
/// module's doc): reads a byte of each page, which commits it if it was not. On Linux
/// the kernel backs a page of the mapping whoever touches it first, so this costs a
/// read per page and changes nothing.
pub fn touch(buf: &[u8]) {
    const PAGE: usize = 4096;
    let start = buf.as_ptr() as usize;
    let mut at = 0;
    while at < buf.len() {
        // A volatile read, so that the compiler keeps it.
        unsafe { std::ptr::read_volatile(buf.as_ptr().add(at)) };
        // To the next page's first byte.
        at += PAGE - (start + at) % PAGE;
    }
}

impl Deref for Ram {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // The whole range is reserved for as long as `self` lives, and every byte of
        // it reads as written (or 0): on Windows the first touch of a piece commits it
        // before the access completes.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl DerefMut for Ram {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for Ram {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        linux::release(self.ptr, self.len);
        #[cfg(windows)]
        windows::release(self.ptr, self.region);
    }
}

impl std::fmt::Debug for Ram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the bytes: RAM can be a terabyte.
        write!(f, "Ram({:#x} bytes)", self.len)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::c_void;

    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const MAP_PRIVATE: i32 = 0x02;
    const MAP_ANONYMOUS: i32 = 0x20;
    /// Don't count the mapping against the overcommit limit: a host page is found
    /// only when one is touched.
    const MAP_NORESERVE: i32 = 0x4000;
    const MAP_FAILED: *mut c_void = !0 as *mut c_void;

    extern "C" {
        fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
    }

    pub fn reserve(len: usize) -> Result<*mut u8, String> {
        let p = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE,
                -1,
                0,
            )
        };
        if p == MAP_FAILED {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(p.cast())
    }

    pub fn release(ptr: *mut u8, len: usize) {
        unsafe { munmap(ptr.cast(), len) };
    }
}

#[cfg(windows)]
mod windows {
    use super::COMMIT_CHUNK;
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};
    use std::sync::Once;

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_READWRITE: u32 = 0x04;
    const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
    const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        record: *mut ExceptionRecord,
        address: *mut c_void,
        parameters: u32,
        /// For an access violation: [0] is 0 for a read, 1 a write, 8 an execution;
        /// [1] the address that faulted.
        information: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        record: *mut ExceptionRecord,
        context: *mut c_void,
    }

    type Handler = unsafe extern "system" fn(*mut ExceptionPointers) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
        fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> i32;
        fn AddVectoredExceptionHandler(first: u32, handler: Handler) -> *mut c_void;
    }

    /// One reserved RAM, `base` and `len`, in a slot of a list the handler reads
    /// without a lock: it runs in the middle of whatever faulted, on any thread, so it
    /// must not wait or allocate. Slots are never freed, only reused.
    ///
    /// `state` says what the slot is. Only a `LIVE` slot's `base` and `len` are a pair,
    /// and nobody but its owner (the `Ram`, when it is dropped) takes a live slot back:
    /// looking for a free slot claims `FREE` ones only, so a live region never drops out
    /// of the handler's sight, as it would if every slot passed were locked to look at.
    pub struct Region {
        base: AtomicUsize,
        len: AtomicUsize,
        state: AtomicU8,
        next: AtomicPtr<Region>,
    }

    const FREE: u8 = 0;
    /// Being set up or taken down: not a region the handler may use.
    const CLAIMED: u8 = 1;
    const LIVE: u8 = 2;

    static REGIONS: AtomicPtr<Region> = AtomicPtr::new(null_mut());
    static INSTALL: Once = Once::new();

    /// A free slot, claimed, or a new one pushed on the list.
    fn take_slot() -> &'static Region {
        let mut at = REGIONS.load(Ordering::Acquire);
        while let Some(r) = unsafe { at.as_ref() } {
            if r.state
                .compare_exchange(FREE, CLAIMED, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return r;
            }
            at = r.next.load(Ordering::Acquire);
        }
        let r: &'static Region = Box::leak(Box::new(Region {
            base: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
            state: AtomicU8::new(CLAIMED),
            next: AtomicPtr::new(null_mut()),
        }));
        let mut head = REGIONS.load(Ordering::Acquire);
        loop {
            r.next.store(head, Ordering::Release);
            match REGIONS.compare_exchange(head, r as *const _ as *mut _, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return r,
                Err(h) => head = h,
            }
        }
    }

    pub fn reserve(len: usize) -> Result<(*mut u8, &'static Region), String> {
        INSTALL.call_once(|| unsafe {
            // First in line, so a first touch is settled before anyone else's handler
            // (or a debugger's first-chance report) sees a fault that isn't one.
            if AddVectoredExceptionHandler(1, first_touch).is_null() {
                panic!("cannot install the RAM's exception handler");
            }
        });
        let p = unsafe { VirtualAlloc(null_mut(), len, MEM_RESERVE, PAGE_NOACCESS) };
        if p.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let r = take_slot();
        r.base.store(p as usize, Ordering::Relaxed);
        r.len.store(len, Ordering::Relaxed);
        // Publishes base and len: the handler reads them only after seeing LIVE.
        r.state.store(LIVE, Ordering::Release);
        Ok((p.cast(), r))
    }

    pub fn release(ptr: *mut u8, r: &'static Region) {
        // Out of the handler's sight first, so it commits nothing here any more. The
        // owner is the only one to take a live slot back, and nothing touches its RAM
        // while it is dropped.
        r.state.store(CLAIMED, Ordering::Release);
        unsafe { VirtualFree(ptr.cast(), 0, MEM_RELEASE) };
        r.len.store(0, Ordering::Relaxed);
        r.base.store(0, Ordering::Relaxed);
        r.state.store(FREE, Ordering::Release);
    }

    /// The region `addr` is in, as (base, len).
    fn region_of(addr: usize) -> Option<(usize, usize)> {
        let mut at = REGIONS.load(Ordering::Acquire);
        while let Some(r) = unsafe { at.as_ref() } {
            if r.state.load(Ordering::Acquire) == LIVE {
                let (base, len) = (r.base.load(Ordering::Relaxed), r.len.load(Ordering::Relaxed));
                if addr >= base && addr - base < len {
                    return Some((base, len));
                }
            }
            at = r.next.load(Ordering::Acquire);
        }
        None
    }

    /// The vectored exception handler: a first touch of reserved RAM commits the
    /// chunk around it and runs the access again.
    unsafe extern "system" fn first_touch(info: *mut ExceptionPointers) -> i32 {
        let record = unsafe { &*(*info).record };
        if record.code != EXCEPTION_ACCESS_VIOLATION || record.parameters < 2 {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let addr = record.information[1];
        let Some((base, len)) = region_of(addr) else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let start = base + (addr - base) / COMMIT_CHUNK * COMMIT_CHUNK;
        let size = COMMIT_CHUNK.min(base + len - start);
        let p = unsafe { VirtualAlloc(start as *mut c_void, size, MEM_COMMIT, PAGE_READWRITE) };
        if p.is_null() {
            // The host is out of memory: let the fault take its normal course.
            return EXCEPTION_CONTINUE_SEARCH;
        }
        EXCEPTION_CONTINUE_EXECUTION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_zero_and_keeps_what_is_written() {
        let mut ram = Ram::new(3 * COMMIT_CHUNK + 4096).unwrap();
        assert_eq!(ram.len(), 3 * COMMIT_CHUNK + 4096);
        assert_eq!(ram[0], 0);
        assert_eq!(ram[ram.len() - 1], 0);
        // Across a chunk boundary, and the last piece, which is shorter than a chunk.
        ram[COMMIT_CHUNK - 2..COMMIT_CHUNK + 2].copy_from_slice(&[1, 2, 3, 4]);
        let last = ram.len() - 1;
        ram[last] = 9;
        assert_eq!(&ram[COMMIT_CHUNK - 2..COMMIT_CHUNK + 2], &[1, 2, 3, 4]);
        assert_eq!(ram[last], 9);
        assert!(ram[2 * COMMIT_CHUNK..3 * COMMIT_CHUNK].iter().all(|&b| b == 0));
        assert_eq!(format!("{ram:?}"), format!("Ram({:#x} bytes)", ram.len()));
    }

    #[test]
    fn a_terabyte_costs_what_is_touched() {
        // 1 TiB reserved, a few bytes far apart touched: only their pieces are backed.
        let mut ram = Ram::new(1 << 40).unwrap();
        for at in [0, 1 << 20, 1 << 33, (1 << 40) - 1] {
            ram[at] = 0x5a;
            assert_eq!(ram[at], 0x5a);
        }
        assert_eq!(ram[1 << 39], 0);
    }

    #[test]
    fn many_at_once_from_many_threads() {
        // Machines are made and dropped on many threads at once (Avalon, the tests):
        // each RAM's first touches find their own region.
        std::thread::scope(|s| {
            for t in 0..8u8 {
                s.spawn(move || {
                    for round in 0..20u8 {
                        let mut ram = Ram::new(4 * COMMIT_CHUNK).unwrap();
                        for chunk in 0..4 {
                            ram[chunk * COMMIT_CHUNK + 7] = t ^ round;
                        }
                        for chunk in 0..4 {
                            assert_eq!(ram[chunk * COMMIT_CHUNK + 7], t ^ round);
                        }
                    }
                });
            }
        });
    }

    #[test]
    fn host_io_into_untouched_ram_works_once_touched() {
        // A file read straight into RAM that nothing has touched yet: on Windows the
        // kernel's write into a page that is only reserved fails the call, which
        // `touch` prevents. Across a chunk boundary, from an odd offset.
        let path = std::env::temp_dir().join(format!("grail-ram-{}.bin", std::process::id()));
        let data: Vec<u8> = (0..3 * 4096 + 100).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let mut ram = Ram::new(4 * COMMIT_CHUNK).unwrap();
        let at = COMMIT_CHUNK - 4096 - 3;
        let buf = &mut ram[at..at + data.len()];
        touch(buf);
        use std::io::Read;
        std::fs::File::open(&path).unwrap().read_exact(buf).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&ram[at..at + data.len()], &data[..]);
        // And out again, from RAM to a file.
        let path = std::env::temp_dir().join(format!("grail-ram-out-{}.bin", std::process::id()));
        let fresh = Ram::new(2 * COMMIT_CHUNK).unwrap();
        touch(&fresh[COMMIT_CHUNK..]);
        std::fs::write(&path, &fresh[COMMIT_CHUNK..]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![0; COMMIT_CHUNK]);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn first_touches_while_others_come_and_go() {
        // Some threads make and drop RAMs all the time (looking for free slots, and
        // freeing theirs), while others take first touches of RAM that stays: each
        // touch must find its region whatever the others are doing to the list. (A
        // version that locked each slot it looked at hid live regions from the handler
        // for a moment, and a touch then crashed the process.)
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let mut ram = Ram::new(COMMIT_CHUNK).unwrap();
                        ram[0] = 1;
                    }
                });
            }
            let touchers: Vec<_> = (0..4)
                .map(|t| {
                    s.spawn(move || {
                        let mut ram = Ram::new(512 * COMMIT_CHUNK).unwrap();
                        for chunk in 0..512 {
                            ram[chunk * COMMIT_CHUNK + t] = chunk as u8;
                        }
                        (0..512).all(|chunk| ram[chunk * COMMIT_CHUNK + t] == chunk as u8)
                    })
                })
                .collect();
            for t in touchers {
                assert!(t.join().unwrap());
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }

    #[test]
    fn a_size_the_host_cannot_reserve_is_refused() {
        // More address space than any 64-bit host has for a process.
        let e = Ram::new(usize::MAX & !0xfff).unwrap_err();
        assert!(e.starts_with("cannot reserve "), "{e}");
    }
}
