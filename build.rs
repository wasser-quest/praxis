fn main() {
    // Linking the large test binary can exceed a small builder's RAM and get the
    // linker OOM-killed (see the test-environment note in
    // docs/PLUGINIZATION_HANDOFF.md). `PRAXIS_LOW_MEMORY_LINK=1` switches to the
    // BFD linker and trades link speed for a much lower peak footprint by
    // spilling its bookkeeping to temporary files. It is opt-in and only valid
    // with a GNU ld (BFD) installation present.
    if std::env::var_os("PRAXIS_LOW_MEMORY_LINK").is_some() {
        println!("cargo:rustc-link-arg=-fuse-ld=bfd");
        println!("cargo:rustc-link-arg=-Wl,--no-keep-memory");
        println!("cargo:rustc-link-arg=-Wl,--reduce-memory-overheads");
    }
}
