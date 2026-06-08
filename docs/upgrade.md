# Upgrading ntgcalls

We publish tgcalls in sync with every ntgcalls release. For users, upgrading
means bumping the crate dependency and nothing else.

For maintainers cutting a new release, these things must change together.

## Steps

1. Bump the pinned version string in `ntgcalls/build.rs` and `tgcalls/build.rs`:

   ```rust
   let pinned = "X.Y.Z".to_string();
   ```

2. Replace `ntgcalls/lib/include/ntgcalls.h` with the header from the new
   ntgcalls release.

3. Regenerate `bindings.rs`:

   ```sh
   bindgen ntgcalls/lib/include/ntgcalls.h \
     --use-core \
     --ctypes-prefix std::os::raw \
     --allowlist-function "ntg_.*" \
     --allowlist-type "ntg_.*" \
     --allowlist-var "NTG_.*" \
     -o ntgcalls/src/bindings.rs
   ```

   On Termux: `pkg install bindgen`

4. Build and fix compile errors:

   ```sh
   cargo build -p tgcalls
   ```

   Errors will be in `structures.rs`, `enums.rs`, `errors.rs`, or `lib.rs`
   depending on what changed in ntgcalls.

5. Read the ntgcalls changelog for semantic changes. Compile errors catch
   shape changes but not meaning changes.

6. Bump the workspace version in `Cargo.toml` and publish.

## What not to do

Do not use `TGCALLS_NTGCALLS_VERSION`. It downloads a different `.so` without
updating `bindings.rs`. `TgCalls::try_new()` will return `CallError::VersionMismatch`.
