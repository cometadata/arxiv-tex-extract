//! Runtime memory-management helpers.
//!
//! Jemalloc's `dirty_decay_ms` / `muzzy_decay_ms` timers alone can't keep up
//! with sustained batch throughput: freed pages sit in per-arena caches and
//! RSS grows monotonically across thousands of papers. Explicitly calling
//! `arena.<i>.purge` at coarse checkpoints (e.g. after each outer tar)
//! returns those pages to the OS.

/// Purge all jemalloc arenas, returning freed pages to the OS.
///
/// Intended to be called at coarse batch boundaries — not per-paper —
/// since purging contends with allocator fast-paths. A best-effort call:
/// any mallctl error is swallowed and the function returns cleanly.
#[cfg(not(target_env = "msvc"))]
pub fn purge_jemalloc_arenas() {
    use tikv_jemalloc_ctl::{arenas, raw};
    let Ok(narenas) = arenas::narenas::read() else { return };
    for i in 0..narenas {
        let name = format!("arena.{}.purge\0", i);
        unsafe {
            let _ = raw::write(name.as_bytes(), ());
        }
    }
}

#[cfg(target_env = "msvc")]
pub fn purge_jemalloc_arenas() {}
