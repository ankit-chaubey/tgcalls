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
//! # What's not here
//!
//! - **Automatic reconnection** - if the network degrades, [`Call`] doesn't
//!   silently retry or fall back to a relay for you. [`Call::connection_mode`]
//!   tells you if you've dropped to relay mode so you can react; this
//!   crate won't do it for you. Worth noting: pytgcalls doesn't do this
//!   either, so this was never actually a parity gap - just a limit worth
//!   knowing about either way.

mod call;
mod calls;
mod e2e;
mod error;
mod media;
mod p2p;
mod signaling;

use std::time::Instant;

pub use call::{Call, CallEvent, CallState, ParticipantAction};
pub use calls::{CallStatus, Calls, Progress};
pub use e2e::{
    incoming_conference_call, migrate_from_p2p, ConferenceCall, ConferenceCalls, ConferenceEvent,
    ConferenceInvite, ConferenceState, ConferenceTarget,
};
pub use error::TgCallsError;
pub use media::{auto_media, auto_media_at, probe_duration, Media};
pub use p2p::{P2PCall, P2PCallState, P2PEvent};
pub use signaling::parse_conference_link;

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
