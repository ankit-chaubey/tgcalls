//! Telegram voice/video calls on top of `ntgcalls` and `ferogram`.
//!
//! Two layers, pick based on how much control you need:
//!
//! - [`Call`] - one chat, you own it. Manual join/leave, direct access to
//!   every ntgcalls operation (screen share, external frames, broadcast
//!   parts, raw participant updates). Use this when you're managing calls
//!   yourself or need something [`Calls`] doesn't expose.
//! - [`Calls`] - one manager, every chat your client is in. `play`, `pause`,
//!   `seek`, `record`, `status` - chat_id-keyed, auto-starts voice chats,
//!   auto-subscribes video, runs each call on its own thread so you never
//!   have to think about `ntgcalls`' threading constraints. Register it
//!   once with your `Dispatcher` and most bots never touch [`Call`] at all.
//!
//! [`Media`] builds the ffmpeg-backed (or device-backed) sources both
//! layers stream. [`P2PCall`] is the separate private (1:1) call flow.
//! [`ConferenceCall`] is Telegram's newer E2E-encrypted call type - a
//! genuinely different join/signaling flow (block chains, not participant
//! updates), documented in the `e2e` module.
//!
//! # Controls
//!
//! Every call type has the same controls, usable at any time while the
//! call is up and in any order. Your state is pushed to Telegram for you,
//! so other participants see mute, paused video, camera and screen share
//! as they change. [`CallControls`] puts them behind one trait, so code
//! that only needs the controls works with a [`Call`], a [`P2PCall`] or a
//! [`ConferenceCall`].
//!
//! | Control | Method |
//! |---|---|
//! | Mic off / on | `mute()` / `unmute()` |
//! | Pause / resume the stream | `pause()` / `resume()` |
//! | Audio you send: set, swap, remove | `set_audio(Some(..))` / `set_audio(None)` |
//! | Video you send: on, swap, off | `set_video(Some(..))` / `set_video(None)` |
//! | Screen share: on, swap, off | `set_screen(Some(..))` / `set_screen(None)` |
//! | Replace everything at once (send or playback) | `set_sources(mode, &media)` |
//! | Raw frames in | `send_external_frame(..)` |
//! | Remote video | `add_incoming_video(..)` / `remove_incoming_video(..)` |
//! | State and info | `media_state()`, `current_media()`, `played_seconds(mode)`, `call_type()`, `connection_mode()`, `cpu_usage()` |
//!
//! These are the same operations ntgcalls has, one name each: four slots
//! (microphone, speaker, camera, screen) in two modes (`Capture` sends,
//! `Playback` receives or records). See [`CallControls`] for what each slot
//! is.
//!
//! [`Calls`] and [`ConferenceCalls`] take a `chat_id` first. [`Calls`] also
//! has `play`, `play_media` and `seek` for queued playback with progress,
//! `record`, `status()` for a one-call snapshot and `chats()` for the
//! tracked chats. Changes to a single slot keep the others, so turning
//! video on never drops the audio.
//!
//! Use `mute()` to silence the mic and keep the source. `set_audio(None)`
//! removes the source entirely.
//!
//! For group calls, [`CallEvent::MediaStateChanged`] fires on every change,
//! including ones caused by the stream itself. Conferences have the
//! matching [`ConferenceEvent::MediaStateChanged`]. P2P state is sent to the
//! other side by ntgcalls directly.
//!
//! # What's not here
//!
//! - **Automatic reconnection** - if the network degrades, [`Call`] doesn't
//!   silently retry or fall back to a relay for you. [`Call::connection_mode`]
//!   tells you if you've dropped to relay mode so you can react to it
//!   yourself.

mod call;
mod calls;
mod controls;
mod e2e;
mod error;
mod media;
mod p2p;
mod signaling;

use std::time::Instant;

pub use call::{Call, CallEvent, CallState, ParticipantAction};
pub use calls::{CallStatus, Calls, Progress};
pub use controls::CallControls;
pub use e2e::{
    incoming_conference_call, migrate_from_p2p, ConferenceCall, ConferenceCalls, ConferenceEvent,
    ConferenceInvite, ConferenceState, ConferenceTarget,
};
pub use error::TgCallsError;
pub use media::{
    auto_media, auto_media_at, auto_media_at_probed, auto_media_probed, probe_duration, Media,
};
pub use p2p::{P2PCall, P2PCallState, P2PEvent};
pub use signaling::{parse_conference_link, send_p2p_signaling};

pub use ntgcalls::{
    AudioDescription, AuthParams, CallType, ConnectionMode, DeviceInfo, DhConfig, FrameData,
    MediaDescription, MediaDevices, MediaSegmentPartStatus, MediaSource, MediaState, NTgCalls,
    RTCServer, SsrcGroup, StreamDevice, StreamMode, StreamStatus, StreamType, VideoDescription,
};

/// Round-trip time to the native ntgcalls layer, in milliseconds. A basic
/// health check, not network latency.
pub fn ping_ms() -> Result<f64, TgCallsError> {
    let start = Instant::now();
    NTgCalls::ping()?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}
