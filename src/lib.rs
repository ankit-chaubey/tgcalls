mod call;
mod calls;
mod error;
mod media;
mod p2p;
mod signaling;

use std::time::Instant;

pub use call::{Call, CallEvent, CallState, ParticipantAction};
pub use calls::Calls;
pub use error::TgCallsError;
pub use media::{auto_media, Media};
pub use p2p::{P2PCall, P2PCallState, P2PEvent};

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
