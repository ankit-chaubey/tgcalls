<h1 align="center">TgCalls</h1>

<p align="center">Telegram voice and video calls for Rust, powered by <a href="https://github.com/pytgcalls/ntgcalls">NTgCalls</a>.</p>

- [x] Group calls
- [x] P2P calls
- [x] Screen sharing
- [x] Broadcast reception

All the power of NTgCalls with a Rust-native API.

## Add to your project

```toml
[dependencies]
tgcalls = ">= 0.1.0"
```

Requires `curl` and `unzip` at build time to download the prebuilt NTgCalls
library. Works on Linux x86_64, Linux aarch64 (including Termux), Windows x86_64,
and macOS aarch64. See [`docs/build.md`](docs/build.md) for details.

## Usage

```rust
use tgcalls::{TgCalls, MediaDescription, AudioDescription, MediaSource, StreamMode};

let tg = TgCalls::try_new()?;

// get params JSON for phone.joinGroupCall
let params = tg.create(chat_id)?;

// after Telegram returns the transport JSON
tg.connect(chat_id, transport_json, false)?;

tg.set_stream_sources(chat_id, StreamMode::Capture, MediaDescription {
    microphone: Some(AudioDescription::new(
        MediaSource::File, "/path/to/audio.raw", 48000, 2, false,
    )),
    ..Default::default()
})?;

tg.stop(chat_id)?;
```

For P2P calls, see [`docs/p2p-flow.md`](docs/p2p-flow.md).

## Version compatibility

`TgCalls::try_new()` checks that the loaded NTgCalls shared library matches the
version this crate was built against. It returns an error if they don't match,
catching layout mismatches before they cause wrong behavior at runtime.

## Docs

Read these in order for the full picture:

1. [`docs/architecture.md`](docs/architecture.md) - crate layout and how the pieces fit together
2. [`docs/build.md`](docs/build.md) - platform requirements and env vars
3. [`docs/p2p-flow.md`](docs/p2p-flow.md) - P2P call flow step by step
4. [`docs/upgrade.md`](docs/upgrade.md) - how to upgrade ntgcalls

Run `cargo doc --open` for the full API reference.

## Contributing

Issues and PRs are welcome. If you're adding support for a new ntgcalls feature,
read `docs/upgrade.md` first. It covers how bindings are generated and what
needs to change together.

## License

MIT or Apache 2.0, at your option.
