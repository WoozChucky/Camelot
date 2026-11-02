//! A peer cannot make the decoder reserve much more memory than the frame it sent: a
//! list count is checked against the bytes left, but an element can be many times
//! bigger in memory than on the wire, so the count must not size the reservation.
//!
//! This binary counts allocations with its own global allocator, so it holds a single
//! test: other tests running in parallel would disturb the count.

use avalon_proto::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static LARGEST: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LARGEST.fetch_max(new_size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// A maximum-size frame of type `ty`: `head`, then a list count that claims every
/// byte left, then `fill` up to the end.
fn forged(ty: u16, head: &[u8], fill: u8) -> Vec<u8> {
    let payload_len = MAX_FRAME_LEN as usize - 2;
    let count = (payload_len - head.len() - 4) as u32;
    let mut payload = head.to_vec();
    payload.extend(count.to_le_bytes());
    payload.resize(payload_len, fill);
    Frame { ty, payload }.to_bytes()
}

/// Payload of a minimal `VmSummary` with an empty device list, as the head of a
/// `VmInfo`.
fn summary_head() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(1u32.to_le_bytes()); // id
    v.extend(1u32.to_le_bytes()); // name "a"
    v.push(b'a');
    v.push(0); // state created
    v.extend(4096u64.to_le_bytes()); // ram_size
    v.push(0); // clock_hz none
    v.extend(1u32.to_le_bytes()); // nominal_hz
    v.extend(0u32.to_le_bytes()); // no devices
    v
}

#[test]
fn forged_list_counts_do_not_reserve_more_than_the_frame() {
    let mut create_head = Vec::new();
    create_head.extend(1u32.to_le_bytes()); // name "a"
    create_head.push(b'a');
    create_head.extend(4096u64.to_le_bytes()); // ram_size
    create_head.push(0); // clock_hz none
    create_head.extend(1u32.to_le_bytes()); // nominal_hz

    // 0xff filler makes the first element fail (a huge string length or a bad tag),
    // except for VmInfo's device entries and a gateway's leases, which are plain
    // integers and addresses: those decode for real until the payload runs out, and
    // the vector grows with them.
    let mut switch_head = Vec::new();
    switch_head.extend(1u32.to_le_bytes()); // one switch
    switch_head.extend(1u32.to_le_bytes()); // name "a"
    switch_head.push(b'a');
    switch_head.push(0); // no capture
    let mut nat_head = switch_head.clone();
    nat_head.push(1); // a gateway
    let mut leases_head = nat_head.clone();
    leases_head.extend(0u32.to_le_bytes()); // no forwarded ports
    switch_head.push(0); // no gateway
    let mut create_switch_head = Vec::new();
    create_switch_head.extend(1u32.to_le_bytes()); // name "a"
    create_switch_head.push(b'a');
    create_switch_head.push(1); // a gateway

    let cases = [
        ("VmList", forged(ty::VM_LIST, &[], 0xff)),
        ("DiskList", forged(ty::DISK_LIST, &[], 0xff)),
        ("SwitchList", forged(ty::SWITCH_LIST, &[], 0xff)),
        ("SwitchList.ports", forged(ty::SWITCH_LIST, &switch_head, 0xff)),
        ("SwitchList.hostfwd", forged(ty::SWITCH_LIST, &nat_head, 0xff)),
        ("SwitchList.leases", forged(ty::SWITCH_LIST, &leases_head, 0xff)),
        (
            "CreateSwitch.hostfwd",
            forged(ty::CREATE_SWITCH, &create_switch_head, 0xff),
        ),
        ("CreateVm.devices", forged(ty::CREATE_VM, &create_head, 0xff)),
        ("VmInfo.devices", forged(ty::VM_INFO, &summary_head(), 0xff)),
    ];
    for (what, frame) in &cases {
        LARGEST.store(0, Ordering::Relaxed);
        let result = decode(frame);
        let largest = LARGEST.load(Ordering::Relaxed);
        assert!(result.is_err(), "{what}: {result:?}");
        // Twice the frame leaves room for a vector of real elements doubling past
        // the payload size; a reservation sized by the count is 16 to 88 times it.
        assert!(
            largest <= 2 * MAX_FRAME_LEN as usize,
            "{what}: decoding a {} byte frame made a {largest} byte allocation",
            frame.len()
        );
    }
}
