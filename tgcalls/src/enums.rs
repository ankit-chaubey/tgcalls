/// Where a media stream's input comes from.
///
/// Passed to [`AudioDescription::new`] and [`VideoDescription::new`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum MediaSource {
    /// Read raw PCM / YUV frames from a file path.
    File = 1 << 0,
    /// Spawn a shell command and read its stdout as raw frames.
    Shell = 1 << 1,
    /// Reserved for future FFmpeg-based decoding. Not yet implemented;
    /// using this source returns [`CallError::FFmpeg`] at runtime.
    ///
    /// [`CallError::FFmpeg`]: crate::errors::CallError::FFmpeg
    FFmpeg = 1 << 2,
    /// Capture from a physical audio/video device.
    Device = 1 << 3,
    /// Desktop/screen capture.
    Desktop = 1 << 4,
    /// Frames pushed manually via [`TgCalls::send_external_frame`].
    External = 1 << 5,
}

/// Which device a stream targets.
///
/// Used in [`TgCalls::send_external_frame`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum StreamDevice {
    /// Microphone input.
    Microphone = 0,
    /// Playback output.
    Speaker = 1,
    /// Camera input.
    Camera = 2,
    /// Screen capture.
    Screen = 3,
}

/// Direction of a stream operation.
///
/// Passed to [`TgCalls::set_stream_sources`] and [`TgCalls::played_time`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum StreamMode {
    /// Outgoing: capturing and sending.
    Capture = 0,
    /// Incoming: receiving and playing back.
    Playback = 1,
}

/// Whether a stream carries audio or video.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum StreamType {
    /// Audio stream.
    Audio = 0,
    /// Video stream.
    Video = 1,
}

/// Playback/capture status of a stream.
///
/// Returned as part of [`CallInfo`] from [`TgCalls::calls`].
///
/// [`CallInfo`]: crate::structures::CallInfo
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum StreamStatus {
    /// Stream is running.
    Active = 0,
    /// Stream is paused.
    Paused = 1,
    /// Stream is idle.
    Idling = 2,
}

/// WebRTC connection lifecycle state.
///
/// Delivered via the `on_connection_change` callback.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ConnectionState {
    /// ICE gathering or DTLS handshake in progress.
    Connecting = 0,
    /// ICE and DTLS complete. Media is flowing.
    Connected = 1,
    /// Connection timed out waiting for ICE consent.
    Timeout = 2,
    /// ICE or DTLS error.
    Failed = 3,
    /// Connection closed cleanly.
    Closed = 4,
}

/// Whether a call is a regular call or a screen-share presentation.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ConnectionKind {
    /// Standard group call or P2P call.
    Normal = 0,
    /// Screen-share presentation alongside a group call.
    Presentation = 1,
}

/// Transport mode ntgcalls is using for a call.
///
/// Returned by [`TgCalls::connection_mode`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ConnectionMode {
    /// No transport active.
    None = 0,
    /// WebRTC RTC transport.
    Rtc = 1,
    /// Broadcast stream transport.
    Stream = 2,
    /// RTMP transport.
    Rtmp = 3,
}

/// Availability of a broadcast segment.
///
/// Passed to [`TgCalls::send_broadcast_part`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum MediaSegmentStatus {
    /// Segment not yet available.
    NotReady = 0,
    /// Resync required. Seek back to a keyframe.
    ResyncNeeded = 1,
    /// Segment data is ready.
    Success = 2,
}

/// Quality level of a broadcast segment.
///
/// Delivered via the `on_request_broadcast_part` callback.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum MediaSegmentQuality {
    /// No video. Audio only.
    None = 0,
    /// Low-resolution thumbnail.
    Thumbnail = 1,
    /// Medium resolution.
    Medium = 2,
    /// Full resolution.
    Full = 3,
}
