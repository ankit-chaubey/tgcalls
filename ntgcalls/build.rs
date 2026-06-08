use std::{
    env,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo:rerun-if-env-changed=TGCALLS_LIB_DIR");
    println!("cargo:rerun-if-env-changed=TGCALLS_NTGCALLS_VERSION");
    println!("cargo:rerun-if-env-changed=TGCALLS_NTGCALLS_URL_PREFIX");

    if env::var("CARGO_FEATURE_BUNDLED").is_ok() {
        bundled();
    } else {
        panic!(
            "Only the `bundled` feature is supported for now. \
             Enable it in your Cargo.toml: ntgcalls = {{ features = [\"bundled\"] }}"
        );
    }
}

fn bundled() {
    use std::process::Command;

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_dir = out_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("failed to resolve target dir");

    let path: PathBuf;

    if let Ok(local_dir) = env::var("TGCALLS_LIB_DIR") {
        eprintln!("ntgcalls: using local NTgCalls from {}", local_dir);
        println!("cargo:rustc-env=NTGCALLS_BUNDLED_VERSION=local");
        path = PathBuf::from(local_dir);
    } else {
        let pinned = "2.2.1".to_string();

        let version = env::var("TGCALLS_NTGCALLS_VERSION").unwrap_or_else(|_| pinned.clone());

        if version != pinned {
            eprintln!(
                "ntgcalls: WARNING: TGCALLS_NTGCALLS_VERSION={version} overrides \
                 pinned version {pinned}.\n\
                 \n\
                 For development only. The safe wrapper was written against {pinned}. \
                 Mismatched versions can cause silent UB, wrong field values, or crashes. \
                 Upgrade by bumping the tgcalls crate, not this env var."
            );
        }

        println!("cargo:rustc-env=NTGCALLS_BUNDLED_VERSION={}", version);

        let base_url = env::var("TGCALLS_NTGCALLS_URL_PREFIX").unwrap_or_else(|_| {
            format!(
                "https://github.com/pytgcalls/ntgcalls/releases/download/v{}",
                version
            )
        });

        let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        let filename = artifact_name(&target_os, &target_arch);
        let url = format!("{}/{}", base_url, filename);

        eprintln!("ntgcalls: downloading {} ...", url);

        let status = Command::new("curl")
            .args(["-fLOk", "--max-time", "120", &url])
            .current_dir(&out_dir)
            .status()
            .expect("curl is required to download the bundled NTgCalls library");
        assert!(
            status.success(),
            "Failed to download NTgCalls from {}.\n\
             Set TGCALLS_LIB_DIR=<dir with libntgcalls.so> to use a local build.",
            url
        );

        let status = Command::new("unzip")
            .args(["-o", &filename])
            .current_dir(&out_dir)
            .status()
            .expect("unzip is required to unpack the bundled NTgCalls library");
        assert!(status.success(), "Failed to unpack {}", filename);

        path = out_dir.join("lib");
    }

    println!("cargo:rustc-link-search={}", path.display());

    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let lib_filename = match os.as_str() {
        "windows" => "ntgcalls.dll",
        "macos" => "libntgcalls.dylib",
        _ => "libntgcalls.so",
    };

    // Copy next to the binary so the dynamic linker finds it without LD_LIBRARY_PATH.
    let src = path.join(lib_filename);
    let dst = target_dir.join(lib_filename);
    if src.exists()
        && let Err(e) = std::fs::copy(&src, &dst)
    {
        eprintln!(
            "ntgcalls: warning: could not copy {} to {}: {}",
            src.display(),
            dst.display(),
            e
        );
    }

    println!("cargo:rustc-link-lib=ntgcalls");
}

fn artifact_name(os: &str, arch: &str) -> String {
    // Release assets: ntgcalls.{os}-{arch}-shared_libs.zip
    // ntgcalls uses "arm64" not "aarch64".
    let arch_str = match arch {
        "aarch64" => "arm64",
        other => other,
    };
    match (os, arch) {
        ("linux", "x86_64" | "aarch64") => {}
        ("windows", "x86_64") => {}
        ("macos", "aarch64") => {}
        ("macos", "x86_64") => panic!(
            "No prebuilt NTgCalls artifact for macos-x86_64 (Intel Mac).\n\
             NTgCalls only ships macos-arm64. Build from source or run under Rosetta.\n\
             See: https://github.com/pytgcalls/ntgcalls"
        ),
        _ => panic!(
            "No prebuilt NTgCalls artifact for {}-{}.\n\
             Build ntgcalls manually and set TGCALLS_LIB_DIR to the directory \
             containing libntgcalls.so.\n\
             See: https://github.com/pytgcalls/ntgcalls",
            os, arch
        ),
    }
    format!("ntgcalls.{}-{}-shared_libs.zip", os, arch_str)
}
