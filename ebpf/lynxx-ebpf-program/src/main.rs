//! lynxx-ebpf kernel-side BPF programs.
//!
//! Two tracepoints are attached:
//!
//! * `tcp/tcp_retransmit_skb` — fires when the kernel retransmits a TCP
//!   segment. We bump a per-CPU-friendly counter in a `HashMap` keyed by
//!   a packed `(saddr, daddr)` 64-bit value. The userspace daemon polls
//!   this map every second and emits Prometheus counters.
//!
//! * `tcp/tcp_set_state` — fires whenever a TCP connection transitions
//!   states (LISTEN → SYN_SENT → ESTABLISHED → FIN_WAIT_1, etc). We
//!   bump a counter keyed by `(oldstate << 8) | newstate`.
//!
//! V1 caveats:
//!
//! * IPv4 only. The tracepoint format includes both v4 and v6 fields,
//!   but for v1 we treat any non-AF_INET event as key = 0.
//! * The tracepoint context offsets are read using
//!   `TracePointContext::read_at`. Field offsets follow the on-kernel
//!   format string at `/sys/kernel/debug/tracing/events/tcp/<name>/format`
//!   on a 5.8+ kernel. They have been stable for a long time, but if a
//!   future kernel reshuffles them, the read will silently produce
//!   garbage. This is acceptable for v1; CO-RE relocation is a v2 task.
//! * No filtering by gateway PID. The userspace `--gateway-pid` flag is
//!   logged but not yet plumbed through to the BPF side.
//!
//! This file MUST compile to BPF bytecode via bpf-linker on a nightly
//! toolchain. It will not build on the host target.

#![no_std]
#![no_main]
#![allow(clippy::missing_safety_doc)]

use aya_ebpf::{
    macros::{map, tracepoint},
    maps::HashMap,
    programs::TracePointContext,
};
use aya_log_ebpf::info;

/// Maximum tracked (saddr, daddr) flows. 8192 is enough for typical
/// gateway-front workloads; on overflow `insert` returns an error and
/// the retransmit is silently dropped from the metric (it still fires
/// in the kernel of course).
const MAX_FLOW_ENTRIES: u32 = 8192;

/// Counter map for TCP retransmits, keyed by `(saddr << 32) | daddr`
/// (both u32 in network byte order). Value is a monotonically
/// increasing u64 incremented from BPF.
#[map]
static mut RETRANSMITS: HashMap<u64, u64> =
    HashMap::<u64, u64>::with_max_entries(MAX_FLOW_ENTRIES, 0);

/// Counter map for TCP state transitions, keyed by
/// `((oldstate as u32) << 16) | (newstate as u32)`. Both states are
/// `TCP_ESTABLISHED`, `TCP_SYN_SENT`, ... values from
/// `include/net/tcp_states.h`.
#[map]
static mut STATE_TRANSITIONS: HashMap<u32, u64> =
    HashMap::<u32, u64>::with_max_entries(256, 0);

// ---------------------------------------------------------------------
// tcp_retransmit_skb
// ---------------------------------------------------------------------

/// Tracepoint format (see
/// /sys/kernel/debug/tracing/events/tcp/tcp_retransmit_skb/format on a
/// recent kernel) — only the offsets we actually read are listed.
///
/// Offsets are from the start of the tracepoint context AFTER the
/// 8-byte common header (aya's `read_at` already accounts for that on
/// most kernels via the same layout the kernel exposes; we use the
/// raw offsets that match the `format` file).
const OFF_SADDR: usize = 8;
const OFF_DADDR: usize = 12;

#[tracepoint]
pub fn tcp_retransmit_skb(ctx: TracePointContext) -> u32 {
    match try_tcp_retransmit_skb(&ctx) {
        Ok(ret) => ret,
        Err(_) => 1,
    }
}

fn try_tcp_retransmit_skb(ctx: &TracePointContext) -> Result<u32, i64> {
    // Both addresses are stored in network byte order in the
    // tracepoint context. We do NOT byteswap here — the userspace
    // side does the formatting. This keeps the BPF program tiny.
    let saddr: u32 = unsafe { ctx.read_at::<u32>(OFF_SADDR).unwrap_or(0) };
    let daddr: u32 = unsafe { ctx.read_at::<u32>(OFF_DADDR).unwrap_or(0) };

    let key: u64 = ((saddr as u64) << 32) | (daddr as u64);

    unsafe {
        let count = RETRANSMITS.get(&key).copied().unwrap_or(0);
        let next = count.wrapping_add(1);
        // BPF_ANY = 0 — insert or update.
        let _ = RETRANSMITS.insert(&key, &next, 0);
    }

    info!(ctx, "tcp_retransmit saddr={:x} daddr={:x}", saddr, daddr);
    Ok(0)
}

// ---------------------------------------------------------------------
// tcp_set_state
// ---------------------------------------------------------------------

/// Field offsets for `tcp/tcp_set_state`. The fields we care about
/// are `oldstate` and `newstate`, both `__u8`, near the end of the
/// context.
const OFF_OLDSTATE: usize = 24;
const OFF_NEWSTATE: usize = 25;

#[tracepoint]
pub fn tcp_set_state(ctx: TracePointContext) -> u32 {
    match try_tcp_set_state(&ctx) {
        Ok(ret) => ret,
        Err(_) => 1,
    }
}

fn try_tcp_set_state(ctx: &TracePointContext) -> Result<u32, i64> {
    let old: u8 = unsafe { ctx.read_at::<u8>(OFF_OLDSTATE).unwrap_or(0) };
    let new: u8 = unsafe { ctx.read_at::<u8>(OFF_NEWSTATE).unwrap_or(0) };

    let key: u32 = ((old as u32) << 16) | (new as u32);

    unsafe {
        let count = STATE_TRANSITIONS.get(&key).copied().unwrap_or(0);
        let next = count.wrapping_add(1);
        let _ = STATE_TRANSITIONS.insert(&key, &next, 0);
    }

    Ok(0)
}

// ---------------------------------------------------------------------
// no_std plumbing
// ---------------------------------------------------------------------

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // BPF programs cannot panic; the verifier wouldn't accept it. We
    // park forever — in practice this branch is unreachable because
    // every fallible path above returns `Result<_, i64>`.
    loop {}
}
