fn main() {
    println!("cargo:rerun-if-env-changed=TGCALLS_LIB_DIR");
    println!("cargo:rerun-if-env-changed=TGCALLS_NTGCALLS_VERSION");

    if std::env::var("TGCALLS_LIB_DIR").is_ok() {
        println!("cargo:rustc-env=NTGCALLS_BUNDLED_VERSION=local");
    } else {
        let pinned = "2.2.1".to_string();

        let version = std::env::var("TGCALLS_NTGCALLS_VERSION").unwrap_or_else(|_| pinned.clone());

        if version != pinned {
            eprintln!(
                "tgcalls: WARNING: TGCALLS_NTGCALLS_VERSION={version} overrides \
                 pinned version {pinned}. For development only."
            );
        }

        println!("cargo:rustc-env=NTGCALLS_BUNDLED_VERSION={}", version);
    }
}
