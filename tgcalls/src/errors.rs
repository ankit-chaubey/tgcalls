use std::{error::Error, fmt::Display};

use ntgcalls::{
    NTG_ERROR_ASYNC_NOT_READY, NTG_ERROR_CONNECTION, NTG_ERROR_CONNECTION_NOT_FOUND,
    NTG_ERROR_CRYPTO, NTG_ERROR_FFMPEG, NTG_ERROR_FILE, NTG_ERROR_INVALID_PARAMS,
    NTG_ERROR_MEDIA_DEVICE, NTG_ERROR_NULL_POINTER, NTG_ERROR_PARSE_SDP, NTG_ERROR_PARSE_TRANSPORT,
    NTG_ERROR_RTC_CONNECTION_NEEDED, NTG_ERROR_RTMP_STREAMING_UNSUPPORTED, NTG_ERROR_SHELL,
    NTG_ERROR_SIGNALING, NTG_ERROR_SIGNALING_UNSUPPORTED, NTG_ERROR_TELEGRAM_SERVER,
    NTG_ERROR_TOO_SMALL, NTG_ERROR_UNKNOWN, NTG_ERROR_WEBRTC,
};

/// Result type for all [`TgCalls`] operations.
///
/// [`TgCalls`]: crate::TgCalls
pub type Result<T> = std::result::Result<T, CallError>;

/// An error from an ntgcalls operation.
///
/// Variants map 1:1 to ntgcalls C error codes except [`VersionMismatch`],
/// which comes from this crate's own startup check.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallError {
    // Startup
    /// The loaded `.so` version does not match what this crate was compiled
    /// against. Upgrade via the crate dependency. See `docs/upgrade.md`.
    VersionMismatch { compiled: String, loaded: String },

    // General
    /// Internal ntgcalls error with no further detail.
    Unknown,
    /// Null pointer passed to ntgcalls. This is a caller bug.
    NullPointer,
    /// Output buffer too small.
    TooSmall,
    /// Async operation polled before it completed.
    AsyncNotReady,

    // NTgCalls core
    /// No active call for this `chat_id` or `user_id`.
    /// Call [`TgCalls::create`] or [`TgCalls::create_p2p`] first.
    ///
    /// [`TgCalls::create`]: crate::TgCalls::create
    /// [`TgCalls::create_p2p`]: crate::TgCalls::create_p2p
    ConnectionNotFound,
    /// Cryptographic operation failed.
    Crypto,
    /// Signaling error.
    Signaling,
    /// Remote does not support the signaling protocol.
    SignalingUnsupported,
    /// Invalid parameters.
    InvalidParams,

    // Stream
    /// Media file could not be opened or read.
    File,
    /// FFmpeg error.
    FFmpeg,
    /// Shell command failed.
    Shell,
    /// Media device could not be accessed.
    MediaDevice,

    // WebRTC
    /// RTMP streaming not supported.
    RtmpStreamingUnsupported,
    /// Transport params JSON from Telegram could not be parsed.
    ParseTransport,
    /// WebRTC connection failed.
    Connection,
    /// Telegram server error during call setup.
    TelegramServer,
    /// Internal WebRTC error.
    WebRtc,
    /// SDP could not be parsed.
    ParseSdp,
    /// RTC connection required but not established yet.
    RtcConnectionNeeded,

    /// An ntgcalls error code not mapped in this crate version.
    /// Usually means the loaded ntgcalls is newer than what this crate was built against.
    Unrecognized(i32),
}

impl Error for CallError {}

impl Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionMismatch { compiled, loaded } => write!(
                f,
                "ntgcalls version mismatch: compiled against {compiled}, loaded {loaded}. \
                 Upgrade via the tgcalls crate. See docs/upgrade.md."
            ),
            Self::Unknown => f.write_str("unknown NTgCalls error"),
            Self::NullPointer => f.write_str("null pointer passed to NTgCalls"),
            Self::TooSmall => f.write_str("output buffer too small"),
            Self::AsyncNotReady => f.write_str("async operation not ready"),
            Self::ConnectionNotFound => f.write_str("no active call for this chat/user ID"),
            Self::Crypto => f.write_str("cryptographic operation failed"),
            Self::Signaling => f.write_str("signaling error"),
            Self::SignalingUnsupported => f.write_str("signaling not supported by remote"),
            Self::InvalidParams => f.write_str("invalid parameters"),
            Self::File => f.write_str("media file error"),
            Self::FFmpeg => f.write_str("ffmpeg error"),
            Self::Shell => f.write_str("shell command failed"),
            Self::MediaDevice => f.write_str("media device error"),
            Self::RtmpStreamingUnsupported => f.write_str("RTMP streaming not supported"),
            Self::ParseTransport => f.write_str("failed to parse transport params"),
            Self::Connection => f.write_str("WebRTC connection failed"),
            Self::TelegramServer => f.write_str("Telegram server error"),
            Self::WebRtc => f.write_str("WebRTC internal error"),
            Self::ParseSdp => f.write_str("failed to parse SDP"),
            Self::RtcConnectionNeeded => f.write_str("RTC connection required but not established"),
            Self::Unrecognized(code) => write!(f, "unrecognized NTgCalls error code: {code}"),
        }
    }
}

impl From<i32> for CallError {
    fn from(code: i32) -> Self {
        match code {
            NTG_ERROR_UNKNOWN => Self::Unknown,
            NTG_ERROR_NULL_POINTER => Self::NullPointer,
            NTG_ERROR_TOO_SMALL => Self::TooSmall,
            NTG_ERROR_ASYNC_NOT_READY => Self::AsyncNotReady,
            NTG_ERROR_CONNECTION_NOT_FOUND => Self::ConnectionNotFound,
            NTG_ERROR_CRYPTO => Self::Crypto,
            NTG_ERROR_SIGNALING => Self::Signaling,
            NTG_ERROR_SIGNALING_UNSUPPORTED => Self::SignalingUnsupported,
            NTG_ERROR_INVALID_PARAMS => Self::InvalidParams,
            NTG_ERROR_FILE => Self::File,
            NTG_ERROR_FFMPEG => Self::FFmpeg,
            NTG_ERROR_SHELL => Self::Shell,
            NTG_ERROR_MEDIA_DEVICE => Self::MediaDevice,
            NTG_ERROR_RTMP_STREAMING_UNSUPPORTED => Self::RtmpStreamingUnsupported,
            NTG_ERROR_PARSE_TRANSPORT => Self::ParseTransport,
            NTG_ERROR_CONNECTION => Self::Connection,
            NTG_ERROR_TELEGRAM_SERVER => Self::TelegramServer,
            NTG_ERROR_WEBRTC => Self::WebRtc,
            NTG_ERROR_PARSE_SDP => Self::ParseSdp,
            NTG_ERROR_RTC_CONNECTION_NEEDED => Self::RtcConnectionNeeded,
            _ => Self::Unrecognized(code),
        }
    }
}
