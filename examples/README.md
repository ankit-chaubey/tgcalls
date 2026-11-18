# Examples

Hey, welcome! If this is your first time here, start from the top. The examples are roughly ordered from simplest to most involved, so you can follow along naturally.

Before anything runs you need two things: an `api_id` and `api_hash` from [my.telegram.org](https://my.telegram.org). Log in there, go to API development tools, and create an app. Then export them once in your shell:

```
export API_ID=123456
export API_HASH=your_api_hash_here
```

On the first run you'll be asked for your phone number, Telegram sends you a login code, and the session is saved to `tgcalls.session` so you don't have to do it again.

A few things worth knowing up front:

- **Everything runs as a client (a userbot), on your own account.** Calls are an MTProto thing, so you need a real user session. Bot tokens can't join calls.
- **You need `ffmpeg` installed.** `Media::audio`, `Media::video` and `Media::av` decode your file through ffmpeg. Raw PCM (`Media::audio_raw`) is the one exception.
- **`chat_id` is the group or channel with the voice chat**, like `-1001234567890`. P2P examples take a `user_id` instead.
- The examples are meant for reading as much as running. Each one has a comment block at the top explaining what it does and why.

---

## The basics (start here)

### `simple_calls.rs`

The easiest way to play something in a voice chat. `Calls` handles the join, creates the voice chat if there isn't one, and works out whether your file has video. Plays the file, waits for you, leaves.

```
cargo run --example simple_calls -- -1001234567890 /path/to/song.mp3
```

If this works, everything else here will too. From here, [`call_controls`](#call_controlsrs) is the fun one, or [`group_audio_call`](#group_audio_callrs) if you want to see what `Calls` does for you under the hood.

### `group_audio_call.rs`

Same idea, but with `Call` directly instead of the manager. You join, stream, and leave yourself. Use this style when you only ever deal with one chat and want full control over each step.

```
cargo run --example group_audio_call -- -1001234567890 /path/to/song.mp3
```

---

## Group calls

All of these use `Call`, the single-chat handle. Pick the one that matches what you want to send.

### `group_video_call.rs`

Streams a video file as the camera. Width, height and fps are optional (defaults to 1280x720 at 30). The size is a bounding box: your video is fit inside it with its aspect ratio kept, never stretched.

```
cargo run --example group_video_call -- -1001234567890 /path/to/video.mp4 1280 720 30
```

### `group_av_call.rs`

Audio and video together, both taken from the same file. Under the hood that's two ffmpeg processes, one for each stream.

```
cargo run --example group_av_call -- -1001234567890 /path/to/video.mp4 1280 720 30
```

Use this one for normal movies and clips. Use [`group_video_call`](#group_video_callrs) when you want video without sound.

### `group_screen_share.rs`

Joins with audio, then starts a screen share on top of it and stops it again later. Screen share is a separate stream with its own connection, but you never touch that yourself, it's just `set_screen(Some(..))` and `set_screen(None)`.

```
cargo run --example group_screen_share -- -1001234567890 /path/to/song.mp3
```

---

## Controls (change things while the call is running)

This is the part most userbots actually need. Everything here works at any time during a call, in any order.

### `call_controls.rs`

Every control through `Calls`, driven from your terminal. Type a command and it happens in the call.

```
cargo run --example call_controls -- -1001234567890 /path/to/video.mp4
```

Commands: `mute`, `unmute`, `pause`, `resume`, `cam on`, `cam off`, `mic on`, `mic off`, `screen on`, `screen off`, `state`, `quit`.

Your mute, camera and screen state is pushed to Telegram right away, so other people see it change. Handy for trying things out before wiring them to your userbot's commands.

### `mute_resume_calls.rs`

The scripted version of the above: mute, unmute, change someone's volume, pause, resume, one after the other. Shows what the calls look like in code, which is what your command handlers would actually contain.

```
cargo run --example mute_resume_calls -- -1001234567890 /path/to/song.mp3 123456789
```

The last argument is the `user_id` whose volume gets changed.

### `call_bitrate.rs`

Plays audio at a bitrate you choose, then lets you type a new number to change it mid-call. Bitrate is in bits per second and **audio only**, ntgcalls has no video bitrate setting.

```
cargo run --example call_bitrate -- -1001234567890 /path/to/song.mp3 64000
```

A rough guide: 24000 to 32000 is fine for voice, 64000 is good for music, 128000 and up is high quality. Changing it swaps the microphone source, so the track restarts from the beginning.

> Needs ntgcalls `3.1.0-alpha01` or newer, which is what this version of tgcalls pins.

### `call_messages.rs`

Prints every message posted in the voice chat's live chat, and sends whatever you type.

```
cargo run --example call_messages -- -1001234567890 /path/to/song.mp3
```

> Messages are only delivered if `Calls` is registered as middleware on a `Dispatcher`. The example already does this. If you build your own userbot or client and never see `CallEvent::Message`, that's the first thing to check.

This is for regular voice chats. Conference calls are end-to-end encrypted and need ntgcalls' encrypt and decrypt functions, which aren't published yet, so messages there aren't supported for now.

---

## Managing many chats

### `queue_calls.rs`

Plays several files back to back in one chat. tgcalls has no queue type and doesn't need one: a queue is a list plus "when this ends, play the next", and `CallEvent::StreamEnded` is that signal. The example is the whole pattern, so you can copy it and change it.

```
cargo run --example queue_calls -- -1001234567890 one.mp3 two.mp3 three.mp3
```

### `graceful_shutdown.rs`

Plays in several chats at once, and on Ctrl+C leaves every one of them cleanly. Without this, killing the process makes Telegram wait for its own timeout before it notices you're gone. With it, you actually leave.

```
cargo run --example graceful_shutdown -- -1001234567890 a.mp3 -1009876543210 b.mp3
```

Pairs of `<chat_id> <file>`, as many as you like.

### `recording_calls.rs`

Records the incoming audio of a voice chat to a file for a number of seconds. If you aren't in the call yet, it joins silently first.

```
cargo run --example recording_calls -- -1001234567890 out.mp3 60
```

---

## P2P calls (one-to-one)

These ring a user directly, like a normal Telegram call, instead of joining a group voice chat. They take a `user_id`.

### `p2p_audio_call.rs`

Rings the user, and once they pick up, streams your audio file.

```
cargo run --example p2p_audio_call -- 123456789 /path/to/song.mp3
```

### `p2p_video_call.rs`

Same, with audio and video from one file.

```
cargo run --example p2p_video_call -- 123456789 /path/to/video.mp4 1280 720 30
```

---

## Conference calls

The newer end-to-end encrypted calls. You'll need a second account (or a friend) to ring, since the point is to have someone join.

All three examples wait until the other person has actually joined before streaming. That's on purpose: the shared encryption key only covers people currently in the call, so anything sent earlier couldn't be decrypted by them. The signal for "they're in" is `ConferenceEvent::ParticipantsChanged`.

You'll also see a verification emoji printed. Compare it with the other side, if it matches you're talking to who you think you are.

### `conference_calls.rs`

Starts a brand-new conference, rings one user, streams an audio file once they join. It uses `ConferenceCall` directly, so you can see the whole flow, including relaying chain block updates by hand so the encryption can move forward.

```
cargo run --example conference_calls -- -1001234567890 123456789 /path/to/song.mp3
```

### `conference_calls_managed.rs`

Same scenario, through the `ConferenceCalls` manager. The manager routes the chain blocks for you through middleware, so there's no relay loop to write. Much nicer once you have more than one conference going.

```
cargo run --example conference_calls_managed -- -1001234567890 123456789 /path/to/song.mp3
```

Start with [`conference_calls`](#conference_callsrs) to understand it, then use this one for real projects.

### `conference_video_calls.rs`

Rings the user as a **video** call, then streams a video file once they join. One thing to know: whether you show up as "video on" is decided when you join, so the example starts with a placeholder camera (it sends nothing) and swaps in the real video after they've joined. If you start with no media at all, the other side never renders your video.

```
cargo run --example conference_video_calls -- -1001234567890 123456789 /path/to/video.mp4 1280 720 30
```

---

## Which one: `Call`, `Calls`, or the conference types?

A short version so you don't have to dig:

**`Calls`** is the manager. One instance covers every chat, it handles joining for you, and it's what you want for a userbot. Most examples above use it.

**`Call`** is a single group call handle. More control, more code. Good when you only deal with one chat.

**`P2PCall`** is a one-to-one call with a user.

**`ConferenceCall`** and **`ConferenceCalls`** are the encrypted conference versions of a single call and the manager.

The controls (mute, pause, camera, screen, and so on) are named the same on all of them, so what you learn on one carries over to the rest.

---

## Quick reference

| Example | Type | Needs | What it does |
|---|---|---|---|
| `simple_calls` | `Calls` | `chat_id`, file | play a file in a voice chat |
| `group_audio_call` | `Call` | `chat_id`, file | join and stream audio |
| `group_video_call` | `Call` | `chat_id`, file | stream video only |
| `group_av_call` | `Call` | `chat_id`, file | stream audio and video from one file |
| `group_screen_share` | `Call` | `chat_id`, file | start and stop a screen share |
| `call_controls` | `Calls` | `chat_id`, file | mute, pause, camera, mic, screen from stdin |
| `mute_resume_calls` | `Calls` | `chat_id`, file, `user_id` | mute, volume, pause in code |
| `call_bitrate` | `Calls` | `chat_id`, file | set and change audio bitrate |
| `call_messages` | `Calls` | `chat_id`, file | read and send voice chat messages |
| `queue_calls` | `Calls` | `chat_id`, files | play files back to back |
| `graceful_shutdown` | `Calls` | `chat_id` + file pairs | leave every chat on Ctrl+C |
| `recording_calls` | `Calls` | `chat_id`, output, seconds | record incoming audio |
| `p2p_audio_call` | `P2PCall` | `user_id`, file | one-to-one audio call |
| `p2p_video_call` | `P2PCall` | `user_id`, file | one-to-one video call |
| `conference_calls` | `ConferenceCall` | `chat_id`, `user_id`, file | encrypted conference, audio |
| `conference_calls_managed` | `ConferenceCalls` | `chat_id`, `user_id`, file | same, through the manager |
| `conference_video_calls` | `ConferenceCall` | `chat_id`, `user_id`, file | encrypted conference, video |
