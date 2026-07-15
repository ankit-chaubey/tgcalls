# tgcalls

Voice and video calls on Telegram, from Rust, using [ferogram](https://github.com/ankit-chaubey/ferogram) (MTProto) and the official [ntgcalls](https://github.com/pytgcalls/ntgcalls) Rust bindings.

Ferogram talks to Telegram. ntgcalls does the actual WebRTC calling work. This crate is the glue between the two, so joining a group call or ringing someone directly is one API instead of two libraries you have to wire together by hand every time.

If you're building a music bot, a voice-call assistant, or anything else that needs to join a Telegram call, this is meant to be quick building blocks for that. It's also just a decent starting point if you're curious how ferogram and ntgcalls fit together and want to see a working example instead of piecing it together from docs.

**This is not a finished library.** It covers the basic flow, join a call, stream a file, leave, plus a P2P calling path, but I've only run it against my minimal setup so far. There's no reconnect handling, no volume or seek controls, no queueing, none of that yet. Think of this as a working starting point, not something you drop into production and forget about.

## What you need

- Rust, stable
- ffmpeg on your PATH. It decodes whatever file you give it into the raw PCM/YUV that ntgcalls actually wants
- A C++ toolchain, since ntgcalls' native core is C++ on top of WebRTC:
  ```
  apt install build-essential zlib1g-dev
  ```
  Skip this and you'll hit `ld: cannot find -lstdc++` or `-lz` at link time. Ask me how I know.
- A Telegram API ID and hash from my.telegram.org
- If you're on Termux/Android, see the section at the bottom before you get your hopes up

## Getting it running

```bash
git clone https://github.com/ankit-chaubey/tgcalls.git
cd tgcalls
export API_ID=123456
export API_HASH=your_api_hash_here
```

First run asks for your phone number and login code (and 2FA if you have it on), then saves a session file so you don't have to do that again.

## Joining a group call and playing something

```bash
cargo run --example group_audio_call -- -1001234567890 /path/to/song.mp3
```

Pass the chat id the way ferogram gives it to you elsewhere (the full `-100xxxxxxxxxx` form); the library strips the prefix internally before handing it to ntgcalls.

What actually happens when you call `join`:

1. Look up the group's active call through ferogram
2. Ask ntgcalls to create a call session and hand back connection params
3. Send those params to Telegram (`phone.joinGroupCall`), get transport info back
4. Give that transport info to ntgcalls and wait for the connection to come up
5. Point ntgcalls at an ffmpeg command that decodes your file into raw audio and starts streaming it

In code, that's just:

```rust
let mut call = Call::new(client, chat_id);
call.join(Media::audio("/path/to/song.mp3")).await?;
// do whatever while it's playing
call.leave().await?;
```

`Media::audio` / `video` / `av` build the ffmpeg command for you. If your file is already raw, headerless PCM or YUV, `Media::audio_raw` skips ffmpeg entirely, but unless you specifically need that, just use `Media::audio`.

## Calling someone directly

Group calls and direct (P2P) calls are separate Telegram APIs with a different setup flow, so this is a separate type:

```bash
cargo run --example p2p_audio_call -- <user_id> /path/to/song.mp3
```

```rust
let mut call = P2PCall::new(client, user_id);
let (servers, versions) = call.request(false, &mut stream).await?;
let (mut sig_out, mut conn) = call.connect(&servers, &versions, true).await?;
call.set_media(StreamMode::Capture, &media).await?;
call.run_signaling(&mut sig_out, &mut conn, &mut stream).await?;
```

P2P calls do a Diffie-Hellman key exchange before anything else, that's what keeps direct calls end to end encrypted, and signaling data has to keep flowing between you and Telegram for the whole time the call is connecting. That's why `run_signaling` is a loop you drive rather than a single call you await once. More moving parts than group calls, and more places for it to hang if the order of operations is wrong.

## API, the short version

Everything you actually need is exported from the crate root.

**`Call`**, for group calls
```rust
let mut call = Call::new(client, chat_id);
call.join(Media::audio(path)).await?;
call.pause().await?;
call.resume().await?;
call.mute().await?;
call.leave().await?;
```

**`P2PCall`**, for direct calls, see the section above for the full flow.

**`Media`**, for building what to stream
```rust
Media::audio(path)                       // any format ffmpeg understands
Media::video(path, width, height, fps)
Media::av(audio_path, video_path, width, height, fps)
Media::audio_raw(path)                   // already raw PCM, skips ffmpeg
Media::screen(width, height, fps)        // for join_presentation / screen share
```

**`TgCallsError`**, one error type for the whole crate, wraps both ferogram and ntgcalls errors plus a few state errors like `NotJoined` or `AlreadyJoined`.

That's the everyday surface. There's more in there too, subscribing to a specific participant's video, pushing raw frames yourself instead of going through ffmpeg, broadcast segment sending, but those are edge cases most bots won't touch. Read `src/call.rs` and `src/p2p.rs` if you need them, they're not long files.

## The other examples

- `group_video_call`, `group_av_call` - same idea as audio, with video or both
- `group_screen_share` - joins a group call and starts a screen share stream on top of it
- `p2p_video_call` - direct call with video

They're all short, read them, that's honestly the fastest way to understand the flow.

## Running on Termux

If you're inside proot-distro Ubuntu on Termux rather than bare Android, this is just a normal Linux aarch64 build, nothing special.

If you're targeting actual Android (bionic, not proot), you're a bit on your own right now. The official ntgcalls-sys build script doesn't know how to fetch Android binaries yet, so you'd need to point `NTGCALLS_LIB_DIR` at a manually downloaded Android build yourself.

Also, Android doesn't expose network interfaces the way WebRTC expects them (no `wlan0` visible through `getifaddrs`), so you'll likely need a compatibility shim. There's a starting point in `getifaddrs_shim.c`, but check ntgcalls' own docs for the details of what it's working around.

## Why this exists

pytgcalls' ntgcalls project ships official Rust bindings now, which do the real WebRTC work: ICE, DTLS, the actual audio/video pipeline. This crate exists so that the whole path, from asking Telegram for a call to actually streaming a file into it, is one coherent thing to call into instead of stitching two libraries together every time you want to build something with calls.

## License

Dual licensed under MIT or Apache 2.0, whichever works better for you. See `LICENSE-MIT` and `LICENSE-APACHE`.
