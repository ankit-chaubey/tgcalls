<h1 align="center">TgCalls</h1>

<p align="center">Telegram voice and video calls for Rust, powered by <a href="https://github.com/pytgcalls/ntgcalls">NTgCalls</a>.</p>

- [x] Group calls
- [x] P2P calls
- [x] Screen sharing
- [x] Broadcast reception

All the power of NTgCalls with a Rust-native API.

## Crates

Two crates: `ntgcalls` (raw FFI bindings) and `tgcalls` (safe Rust wrapper on top).

Most users only need to touch `tgcalls`. Use `ntgcalls` only if you need a deeper level and know what you are doing.

## Getting started

The build script downloads the right prebuilt `libntgcalls` automatically. Requires `curl` and `unzip` on PATH. See [docs/build.md](docs/build.md) for platforms, env vars, and Termux notes.

## Contributing

Issues and PRs are welcome. If you are adding support for a new ntgcalls feature, read [docs/upgrade.md](docs/upgrade.md) first.

## License

MIT or Apache 2.0, at your option.
