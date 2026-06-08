# Architecture

## Crate layout

```
ntgcalls   raw FFI bindings to the ntgcalls C API
tgcalls       safe Rust wrapper over ntgcalls
```

## How it fits together

ntgcalls is a C shared library distributed as prebuilt binaries. `ntgcalls`
downloads and links it at build time and exposes the raw C types via `bindings.rs`.

`tgcalls` wraps that into safe Rust: owned types instead of raw pointers, a
proper error enum instead of integer codes, and blocking calls instead of raw
C async callbacks.

## Why bindings.rs is committed

bindgen is not run at build time. `bindings.rs` and `ntgcalls.h` are committed
and pinned to the version hardcoded in `ntgcalls/build.rs`. Builds are
reproducible without the bindgen toolchain. The trade-off is a manual step on
each ntgcalls upgrade. See `docs/upgrade.md`.

## Version check

`tgcalls/build.rs` bakes the pinned version into the binary.
`TgCalls::try_new()` calls `ntg_get_version()` at startup and returns
`CallError::VersionMismatch` if it does not match. This catches struct layout
mismatches before they cause wrong behavior at runtime.
