# ntgcalls

Raw FFI bindings to the [NTgCalls](https://github.com/pytgcalls/ntgcalls) C API.

This crate is intended for internal use. Most users should use [`tgcalls`](../tgcalls) instead.

## Build

Requires `curl` and `unzip` on PATH. See `docs/build.md`.

## Environment variables

| Variable                      | Purpose                                              |
|-------------------------------|------------------------------------------------------|
| `TGCALLS_LIB_DIR`             | Path to a local `libntgcalls.so`. Skips the download. |
| `TGCALLS_NTGCALLS_VERSION`    | Override the download version. Development only. See warning below. |
| `TGCALLS_NTGCALLS_URL_PREFIX` | Override the base download URL.                      |

## Warning: do not override the version

`TGCALLS_NTGCALLS_VERSION` is for development only. The wrapper was written
against the pinned version in `build.rs`. Linking a different `.so` can cause
struct mismatches, wrong values, and crashes.

We publish in sync with ntgcalls. Upgrade via the crate.
`TgCalls::try_new()` returns `CallError::VersionMismatch` if versions do not match.
