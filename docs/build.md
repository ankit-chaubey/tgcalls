# Build

## Required tools

`curl` and `unzip` must be on PATH during `cargo build`. They download and
unpack the prebuilt ntgcalls library.

## Supported platforms

| Platform       | Artifact                                  |
|----------------|-------------------------------------------|
| Linux x86_64   | ntgcalls.linux-x86_64-shared_libs.zip    |
| Linux aarch64  | ntgcalls.linux-arm64-shared_libs.zip     |
| Windows x86_64 | ntgcalls.windows-x86_64-shared_libs.zip  |
| macOS aarch64  | ntgcalls.macos-arm64-shared_libs.zip     |

macOS x86_64 (Intel) has no prebuilt artifact. Build ntgcalls from source or
run under Rosetta with the arm64 binary.

- Supported platforms and release binaries are listed on the NTgCalls [releases page.](https://github.com/pytgcalls/ntgcalls/releases)

## Termux (Android aarch64)

```sh
pkg install curl unzip
cargo build
```

The `.so` is copied next to the binary so it is found without `LD_LIBRARY_PATH`.

If you encounter networking issues on Termux, consider running the project inside Ubuntu or Debian via proot-distro.

For interface detection related problems, see getifaddrs_shim.c in the project root. It provides a compatibility shim for applications that expect standard Linux network interfaces such as wlan0.

## Local ntgcalls build

Set `TGCALLS_LIB_DIR` to the directory containing `libntgcalls.so`. The
download is skipped. The runtime version check is also skipped since the
version is unknown at compile time.

```sh
TGCALLS_LIB_DIR=/path/to/ntgcalls/lib cargo build
```

## Environment variables

| Variable                     | Purpose                                                      |
|------------------------------|--------------------------------------------------------------|
| `TGCALLS_LIB_DIR`            | Path to a local `libntgcalls.so`. Skips the download.        |
| `TGCALLS_NTGCALLS_VERSION`   | Override the download version. Development only. See warning in `ntgcalls/README.md`. |
| `TGCALLS_NTGCALLS_URL_PREFIX`| Override the base download URL.                              |
