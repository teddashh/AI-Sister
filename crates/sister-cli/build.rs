fn main() {
    // Windows PE default main-thread stack is 1MB. `sister diagnose` in a
    // debug build crosses that while rendering an on-disk audit. Spawned
    // threads are not the thread that overflows.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }
}
