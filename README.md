<img src="https://raw.githubusercontent.com/ankit-chaubey/tgcalls/main/.github/images/tgcalls.png" alt="tgcalls logo" />

<p align="center">
    <b>An elegant Rust client for Telegram voice and video calls.</b>
    <br>
    <sub>Powered by <a href="https://github.com/pytgcalls/ntgcalls">ntgcalls</a> and <a href="https://github.com/ankit-chaubey/ferogram">ferogram</a>.</sub>
    <br><br>
    <a href="https://github.com/ankit-chaubey/tgcalls/tree/main/examples">
        Examples
    </a>
    •
    <a href="https://docs.rs/tgcalls">
        Documentation
    </a>
    •
    <a href="https://crates.io/crates/tgcalls">
        Crates.io
    </a>
    •
    <a href="https://github.com/ankit-chaubey/ferogram">
        Ferogram
    </a>
</p>

# tgcalls [![Crates.io](https://img.shields.io/crates/v/tgcalls.svg?logo=rust&logoColor=%23959DA5&label=crates.io&labelColor=%23282f37&color=%23e5710a)](https://crates.io/crates/tgcalls) [![Downloads](https://img.shields.io/crates/d/tgcalls?logoColor=%23959DA5&labelColor=%23282f37&color=%2328A745)](https://crates.io/crates/tgcalls) [![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg?labelColor=%23282f37)](#license)

tgcalls brings real voice and video calling to Telegram bots and clients written in Rust. It sits between two libraries that don't know about each other: [ferogram](https://github.com/ankit-chaubey/ferogram) speaks MTProto to Telegram, and [ntgcalls](https://github.com/pytgcalls/ntgcalls) does the actual WebRTC work of getting audio and video across the wire. tgcalls wires them together so joining a call, or starting one, is a single clean call instead of two libraries you have to coordinate by hand.

If you're building a music bot, a voice assistant, or anything that needs to be present in a Telegram call, this is the fastest way there.

**A note on where this stands:** the core flow works, join a call, stream into it, leave, and there's a full P2P calling path too, but it's still early. No reconnect handling, no seek or volume controls, no queueing yet. Treat it as a solid foundation to build on rather than something finished.

## Getting started

```bash
git clone https://github.com/ankit-chaubey/tgcalls.git
cd tgcalls
export API_ID=123456
export API_HASH=your_api_hash_here
```

You'll need ffmpeg on your PATH (it decodes whatever media you give it into the raw audio/video ntgcalls wants) and a C++ toolchain, since ntgcalls' native core is built on WebRTC:

```bash
apt install build-essential zlib1g-dev
```

First run will ask for your phone number and login code, then save a session so you only do that once.

<p align="center">
    <img src="https://raw.githubusercontent.com/ankit-chaubey/tgcalls/main/.github/images/banner.png" alt="tgcalls banner" />
</p>

## Let's stream

Joining a group call and playing a file into it is three lines:

```rust
let mut call = Call::new(client, chat_id);
call.join(Media::audio("/path/to/song.mp3")).await?;
call.leave().await?;
```

Behind that `join`, tgcalls looks up the group's active call through ferogram, asks ntgcalls to open a session, exchanges transport info with Telegram, and once the connection is up, points ntgcalls at ffmpeg to start streaming your file. All of that happens for you.

```bash
cargo run --example group_audio_call -- -1001234567890 /path/to/song.mp3
```

Direct, one-on-one calls work the same way in spirit, ring someone, exchange keys, stream, though the setup underneath is a little more involved since P2P calls are end-to-end encrypted and need a live signaling loop while connecting. The `p2p_audio_call` example walks through it end to end.

Video and screen share follow the same pattern, `Media::video`, `Media::av`, and `Media::screen` build the right ffmpeg pipeline for you. The `examples/` folder has a short, working file for each of these, that's genuinely the fastest way to see how it all fits together.

## License

Dual licensed under MIT or Apache 2.0, whichever works better for you. See `LICENSE-MIT` and `LICENSE-APACHE`.
