use std::ffi::{CStr, CString};

use ntgcalls::{
    ntg_audio_description_struct, ntg_call_info_struct, ntg_media_description_struct,
    ntg_media_source_enum, ntg_video_description_struct,
};

use crate::{
    enums::{
        ConnectionKind, ConnectionState, MediaSegmentQuality, MediaSource, StreamDevice,
        StreamStatus,
    },
    utils::IntoCString,
};

/// Combined audio and video sources for a call.
///
/// Passed to [`TgCalls::set_stream_sources`]. Each field is optional. Set
/// only the streams you need. Unset fields leave that device inactive.
///
/// # Example
///
/// ```rust
/// use tgcalls::{MediaDescription, AudioDescription, MediaSource};
///
/// let desc = MediaDescription {
///     microphone: Some(AudioDescription::new(
///         MediaSource::File, "/tmp/audio.raw", 48000, 2, false,
///     )),
///     ..Default::default()
/// };
/// ```
///
/// [`TgCalls::set_stream_sources`]: crate::TgCalls::set_stream_sources
#[derive(Debug, Default, Clone)]
pub struct MediaDescription {
    /// Microphone / outgoing audio source.
    pub microphone: Option<AudioDescription>,
    /// Speaker / incoming audio playback source.
    pub speaker: Option<AudioDescription>,
    /// Camera / outgoing video source.
    pub camera: Option<VideoDescription>,
    /// Screen capture source.
    pub screen: Option<VideoDescription>,
}

/// Audio stream configuration.
///
/// Describes a single audio source. Used inside [`MediaDescription`].
#[derive(Debug, Clone)]
pub struct AudioDescription {
    /// Where the audio comes from.
    pub media_source: MediaSource,
    input: CString,
    /// Sample rate in Hz. Telegram group calls use 48000.
    pub sample_rate: u32,
    /// Number of channels. Telegram group calls use 2 (stereo).
    pub channel_count: u8,
    /// Keep the source open after the stream ends, for looping or live sources.
    pub keep_open: bool,
}

impl AudioDescription {
    /// Create a new audio description.
    ///
    /// - `media_source`: how ntgcalls reads the input.
    /// - `input`: file path, shell command, or device name depending on `media_source`.
    /// - `sample_rate`: Hz. Use 48000 for Telegram.
    /// - `channel_count`: channels. Use 2 for stereo.
    /// - `keep_open`: whether to keep the source open after EOF.
    pub fn new<S: IntoCString>(
        media_source: MediaSource,
        input: S,
        sample_rate: u32,
        channel_count: u8,
        keep_open: bool,
    ) -> Self {
        Self {
            media_source,
            input: input.into_c_string(),
            sample_rate,
            channel_count,
            keep_open,
        }
    }

    pub(crate) fn to_ffi(&self) -> ntg_audio_description_struct {
        ntg_audio_description_struct {
            mediaSource: self.media_source as ntg_media_source_enum,
            input: self.input.as_ptr() as *mut _,
            sampleRate: self.sample_rate,
            channelCount: self.channel_count,
            keepOpen: self.keep_open,
        }
    }
}

/// Video stream configuration.
///
/// Describes a single video source. Used inside [`MediaDescription`].
#[derive(Debug, Clone)]
pub struct VideoDescription {
    /// Where the video comes from.
    pub media_source: MediaSource,
    input: CString,
    /// Frame width in pixels.
    pub width: i16,
    /// Frame height in pixels.
    pub height: i16,
    /// Target frames per second.
    pub fps: u8,
    /// Keep the source open after the stream ends.
    pub keep_open: bool,
}

impl VideoDescription {
    /// Create a new video description.
    ///
    /// - `media_source`: how ntgcalls reads the input.
    /// - `input`: file path, shell command, or device name.
    /// - `width` / `height`: frame dimensions in pixels. Must be even.
    /// - `fps`: target frame rate.
    /// - `keep_open`: whether to keep the source open after EOF.
    pub fn new<S: IntoCString>(
        media_source: MediaSource,
        input: S,
        width: i16,
        height: i16,
        fps: u8,
        keep_open: bool,
    ) -> Self {
        Self {
            media_source,
            input: input.into_c_string(),
            width,
            height,
            fps,
            keep_open,
        }
    }

    pub(crate) fn to_ffi(&self) -> ntg_video_description_struct {
        ntg_video_description_struct {
            mediaSource: self.media_source as ntg_media_source_enum,
            input: self.input.as_ptr() as *mut _,
            width: self.width,
            height: self.height,
            fps: self.fps,
            keepOpen: self.keep_open,
        }
    }
}

pub(crate) struct FfiMediaDesc {
    pub microphone: Option<ntg_audio_description_struct>,
    pub speaker: Option<ntg_audio_description_struct>,
    pub camera: Option<ntg_video_description_struct>,
    pub screen: Option<ntg_video_description_struct>,
}

impl FfiMediaDesc {
    pub fn new(desc: &MediaDescription) -> Self {
        Self {
            microphone: desc.microphone.as_ref().map(AudioDescription::to_ffi),
            speaker: desc.speaker.as_ref().map(AudioDescription::to_ffi),
            camera: desc.camera.as_ref().map(VideoDescription::to_ffi),
            screen: desc.screen.as_ref().map(VideoDescription::to_ffi),
        }
    }

    pub fn as_ffi(&mut self) -> ntg_media_description_struct {
        ntg_media_description_struct {
            microphone: self
                .microphone
                .as_mut()
                .map_or(std::ptr::null_mut(), |a| a as *mut _),
            speaker: self
                .speaker
                .as_mut()
                .map_or(std::ptr::null_mut(), |a| a as *mut _),
            camera: self
                .camera
                .as_mut()
                .map_or(std::ptr::null_mut(), |v| v as *mut _),
            screen: self
                .screen
                .as_mut()
                .map_or(std::ptr::null_mut(), |v| v as *mut _),
        }
    }
}

/// Status of a single active call.
///
/// Returned as part of the `Vec<CallInfo>` from [`TgCalls::calls`].
///
/// [`TgCalls::calls`]: crate::TgCalls::calls
#[derive(Debug, Clone)]
pub struct CallInfo {
    /// The chat ID this call belongs to.
    pub chat_id: i64,
    /// Status of the outgoing (capture) stream.
    pub capture: StreamStatus,
    /// Status of the incoming (playback) stream.
    pub playback: StreamStatus,
}

impl From<ntg_call_info_struct> for CallInfo {
    fn from(s: ntg_call_info_struct) -> Self {
        Self {
            chat_id: s.chatId,
            capture: unsafe { std::mem::transmute::<i32, crate::enums::StreamStatus>(s.capture) },
            playback: unsafe { std::mem::transmute::<i32, crate::enums::StreamStatus>(s.playback) },
        }
    }
}

/// Mute and pause state of a call.
///
/// Returned by [`TgCalls::media_state`].
///
/// [`TgCalls::media_state`]: crate::TgCalls::media_state
#[derive(Debug, Clone)]
pub struct MediaState {
    /// Microphone is muted.
    pub muted: bool,
    /// Video (camera) is paused.
    pub video_paused: bool,
    /// Video (camera) is fully stopped.
    pub video_stopped: bool,
    /// Screen-share presentation is paused.
    pub presentation_paused: bool,
}

/// Connection kind and state snapshot.
///
/// Delivered via the `on_connection_change` callback.
#[derive(Debug, Clone, Copy)]
pub struct NetworkInfo {
    /// Whether this is a normal call or a presentation.
    pub kind: ConnectionKind,
    /// Current WebRTC connection state.
    pub state: ConnectionState,
}

/// A STUN or TURN server for a P2P call.
///
/// Passed to [`TgCalls::connect_p2p`] as a slice of servers obtained from
/// Telegram's `phone.requestCall` / `phone.acceptCall` response.
///
/// [`TgCalls::connect_p2p`]: crate::TgCalls::connect_p2p
#[derive(Debug, Clone)]
pub struct RtcServer {
    /// Server identifier from Telegram.
    pub id: u64,
    /// IPv4 address string.
    pub ipv4: String,
    /// IPv6 address string. May be empty if IPv6 is not available.
    pub ipv6: String,
    /// TURN username. Empty for STUN-only servers.
    pub username: String,
    /// TURN password. Empty for STUN-only servers.
    pub password: String,
    /// UDP port.
    pub port: u16,
    /// Whether this server supports TURN relay.
    pub turn: bool,
    /// Whether this server supports STUN binding.
    pub stun: bool,
    /// Whether TCP fallback is available.
    pub tcp: bool,
    /// Peer tag bytes for classic (non-WebRTC) connections.
    pub peer_tag: Vec<u8>,
}

pub(crate) struct FfiRtcServer {
    pub ffi: ntgcalls::ntg_rtc_server_struct,
    _ipv4: CString,
    _ipv6: CString,
    _username: CString,
    _password: CString,
    _peer_tag: Vec<u8>,
}

impl FfiRtcServer {
    pub fn new(s: &RtcServer) -> Self {
        let ipv4 = CString::new(s.ipv4.as_str()).unwrap_or_default();
        let ipv6 = CString::new(s.ipv6.as_str()).unwrap_or_default();
        let username = CString::new(s.username.as_str()).unwrap_or_default();
        let password = CString::new(s.password.as_str()).unwrap_or_default();
        let mut peer_tag = s.peer_tag.clone();
        let ffi = ntgcalls::ntg_rtc_server_struct {
            id: s.id,
            ipv4: ipv4.as_ptr() as *mut _,
            ipv6: ipv6.as_ptr() as *mut _,
            username: username.as_ptr() as *mut _,
            password: password.as_ptr() as *mut _,
            port: s.port,
            turn: s.turn,
            stun: s.stun,
            tcp: s.tcp,
            peerTag: if peer_tag.is_empty() {
                std::ptr::null_mut()
            } else {
                peer_tag.as_mut_ptr()
            },
            peerTagSize: peer_tag.len() as i32,
        };
        Self {
            ffi,
            _ipv4: ipv4,
            _ipv6: ipv6,
            _username: username,
            _password: password,
            _peer_tag: peer_tag,
        }
    }
}

/// Diffie-Hellman parameters from Telegram for P2P key exchange.
///
/// Obtained from `messages.getDhConfig` and passed to
/// [`TgCalls::init_exchange`].
///
/// [`TgCalls::init_exchange`]: crate::TgCalls::init_exchange
#[derive(Debug, Clone)]
pub struct DhConfig {
    /// The generator `g`.
    pub g: i32,
    /// The prime `p` as a big-endian byte slice.
    pub p: Vec<u8>,
    /// Random bytes from Telegram, used as additional entropy.
    pub random: Vec<u8>,
}

pub(crate) struct FfiDhConfig {
    pub ffi: ntgcalls::ntg_dh_config_struct,
    _p: Vec<u8>,
    _random: Vec<u8>,
}

impl FfiDhConfig {
    pub fn new(c: &DhConfig) -> Self {
        let p = c.p.clone();
        let random = c.random.clone();
        let ffi = ntgcalls::ntg_dh_config_struct {
            g: c.g,
            p: p.as_ptr(),
            sizeP: p.len() as i32,
            random: random.as_ptr(),
            sizeRandom: random.len() as i32,
        };
        Self {
            ffi,
            _p: p,
            _random: random,
        }
    }
}

/// Result of a completed DH key exchange.
///
/// Returned by [`TgCalls::exchange_keys`]. Pass `g_a_or_b` and
/// `key_fingerprint` to Telegram's `phone.confirmCall`.
///
/// [`TgCalls::exchange_keys`]: crate::TgCalls::exchange_keys
#[derive(Debug, Clone)]
pub struct AuthParams {
    /// The local `g_a` or `g_b` value to send to Telegram.
    pub g_a_or_b: Vec<u8>,
    /// Key fingerprint to send to Telegram for verification.
    pub key_fingerprint: i64,
}

impl AuthParams {
    pub(crate) fn from_ffi(raw: ntgcalls::ntg_auth_params_struct) -> Self {
        let g_a_or_b = if raw.g_a_or_b.is_null() || raw.sizeGAB <= 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(raw.g_a_or_b, raw.sizeGAB as usize).to_vec() }
        };
        Self {
            g_a_or_b,
            key_fingerprint: raw.key_fingerprint,
        }
    }
}

/// A group of SSRCs sharing the same RTP extension semantics.
///
/// Passed to [`TgCalls::add_incoming_video`] to subscribe to a participant's
/// video stream. Values come from Telegram's group call participant updates.
///
/// [`TgCalls::add_incoming_video`]: crate::TgCalls::add_incoming_video
#[derive(Debug, Clone)]
pub struct SsrcGroup {
    /// Semantics string, e.g. `"SIM"` or `"FID"`.
    pub semantics: String,
    /// The SSRCs in this group.
    pub ssrcs: Vec<u32>,
}

pub(crate) struct FfiSsrcGroup {
    pub ffi: ntgcalls::ntg_ssrc_group_struct,
    _semantics: CString,
    _ssrcs: Vec<u32>,
}

impl FfiSsrcGroup {
    pub fn new(g: &SsrcGroup) -> Self {
        let sem = CString::new(g.semantics.as_str()).unwrap_or_default();
        let mut ssrcs = g.ssrcs.clone();
        let ffi = ntgcalls::ntg_ssrc_group_struct {
            semantics: sem.as_ptr() as *mut _,
            ssrcs: if ssrcs.is_empty() {
                std::ptr::null_mut()
            } else {
                ssrcs.as_mut_ptr()
            },
            sizeSsrcs: ssrcs.len() as i32,
        };
        Self {
            ffi,
            _semantics: sem,
            _ssrcs: ssrcs,
        }
    }
}

/// Metadata for an external video frame.
///
/// Passed to [`TgCalls::send_external_frame`] alongside the raw pixel data.
///
/// [`TgCalls::send_external_frame`]: crate::TgCalls::send_external_frame
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameData {
    /// Capture timestamp in milliseconds (monotonic clock).
    pub absolute_capture_timestamp_ms: i64,
    /// Frame width in pixels.
    pub width: u16,
    /// Frame height in pixels.
    pub height: u16,
    /// Clockwise rotation in degrees (0, 90, 180, or 270).
    pub rotation: u16,
}

impl FrameData {
    pub(crate) fn to_ffi(self) -> ntgcalls::ntg_frame_data_struct {
        ntgcalls::ntg_frame_data_struct {
            absoluteCaptureTimestampMs: self.absolute_capture_timestamp_ms,
            width: self.width,
            height: self.height,
            rotation: self.rotation,
        }
    }
}

/// Information about a single audio or video device.
///
/// Part of [`MediaDevices`] returned by [`TgCalls::media_devices`].
///
/// [`TgCalls::media_devices`]: crate::TgCalls::media_devices
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Human-readable device name.
    pub name: String,
    /// Platform-specific metadata string (may be empty).
    pub metadata: String,
}

impl DeviceInfo {
    unsafe fn from_ffi(raw: &ntgcalls::ntg_device_info_struct) -> Self {
        let name = if raw.name.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(raw.name) }
                .to_string_lossy()
                .into_owned()
        };
        let metadata = if raw.metadata.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(raw.metadata) }
                .to_string_lossy()
                .into_owned()
        };
        Self { name, metadata }
    }
}

/// All available audio and video devices on the host.
///
/// Returned by [`TgCalls::media_devices`].
///
/// [`TgCalls::media_devices`]: crate::TgCalls::media_devices
#[derive(Debug, Clone)]
pub struct MediaDevices {
    /// Available microphones.
    pub microphones: Vec<DeviceInfo>,
    /// Available speakers / playback devices.
    pub speakers: Vec<DeviceInfo>,
    /// Available cameras.
    pub cameras: Vec<DeviceInfo>,
    /// Available screen capture sources.
    pub screens: Vec<DeviceInfo>,
}

impl MediaDevices {
    pub(crate) unsafe fn from_ffi(raw: ntgcalls::ntg_media_devices_struct) -> Self {
        fn read_devices(ptr: *mut ntgcalls::ntg_device_info_struct, size: i32) -> Vec<DeviceInfo> {
            if ptr.is_null() || size <= 0 {
                return Vec::new();
            }
            unsafe {
                std::slice::from_raw_parts(ptr, size as usize)
                    .iter()
                    .map(|d| DeviceInfo::from_ffi(d))
                    .collect()
            }
        }
        Self {
            microphones: read_devices(raw.microphone, raw.sizeMicrophone),
            speakers: read_devices(raw.speaker, raw.sizeSpeaker),
            cameras: read_devices(raw.camera, raw.sizeCamera),
            screens: read_devices(raw.screen, raw.sizeScreen),
        }
    }
}

/// A remote participant's active video source.
///
/// Delivered via the `on_remote_source_change` callback.
#[derive(Debug, Clone, Copy)]
pub struct RemoteSource {
    /// The SSRC of the remote source.
    pub ssrc: u32,
    /// Whether the source is active, paused, or idle.
    pub state: StreamStatus,
    /// Which device type this source represents.
    pub device: StreamDevice,
}

/// A request from ntgcalls for a broadcast segment.
///
/// Delivered via the `on_request_broadcast_part` callback. Fetch the
/// requested segment from Telegram and pass it back via
/// [`TgCalls::send_broadcast_part`].
///
/// [`TgCalls::send_broadcast_part`]: crate::TgCalls::send_broadcast_part
#[derive(Debug, Clone, Copy)]
pub struct SegmentPartRequest {
    /// Segment identifier (maps to Telegram's broadcast segment index).
    pub segment_id: i64,
    /// Part identifier within the segment.
    pub part_id: i32,
    /// Maximum byte length ntgcalls will accept for this part.
    pub limit: i32,
    /// Presentation timestamp for this segment in milliseconds.
    pub timestamp: i64,
    /// Whether this request is for a quality-change update rather than new data.
    pub quality_update: bool,
    /// Channel identifier within the broadcast.
    pub channel_id: i32,
    /// Requested quality level.
    pub quality: MediaSegmentQuality,
}
