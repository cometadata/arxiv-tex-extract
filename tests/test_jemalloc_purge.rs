#[cfg(not(target_env = "msvc"))]
#[test]
fn purge_arenas_does_not_panic() {
    // Smoke test: the purge function must be callable and return cleanly
    // under the real jemalloc allocator used by the binary.
    latex_extract::memory::purge_jemalloc_arenas();
}
