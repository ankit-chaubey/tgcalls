// Hand-written bindings for ntgcalls v2.2.1 C API.
// Source of truth: ntgcalls/include/ntgcalls.h (v2.2.1)

use std::os::raw::{c_char, c_double, c_int};

// Error codes

pub const NTG_ERROR_CONNECTION_NOT_FOUND: i32 = -101;
pub const NTG_ERROR_CRYPTO: i32 = -102;
pub const NTG_ERROR_SIGNALING: i32 = -104;
pub const NTG_ERROR_SIGNALING_UNSUPPORTED: i32 = -105;
pub const NTG_ERROR_INVALID_PARAMS: i32 = -106;

pub const NTG_ERROR_FILE: i32 = -200;
pub const NTG_ERROR_FFMPEG: i32 = -202;
pub const NTG_ERROR_SHELL: i32 = -203;
pub const NTG_ERROR_MEDIA_DEVICE: i32 = -204;

pub const NTG_ERROR_RTMP_STREAMING_UNSUPPORTED: i32 = -300;
pub const NTG_ERROR_PARSE_TRANSPORT: i32 = -301;
pub const NTG_ERROR_CONNECTION: i32 = -302;
pub const NTG_ERROR_TELEGRAM_SERVER: i32 = -303;
pub const NTG_ERROR_WEBRTC: i32 = -304;
pub const NTG_ERROR_PARSE_SDP: i32 = -305;
pub const NTG_ERROR_RTC_CONNECTION_NEEDED: i32 = -306;

pub const NTG_ERROR_UNKNOWN: i32 = -1;
pub const NTG_ERROR_NULL_POINTER: i32 = -2;
pub const NTG_ERROR_TOO_SMALL: i32 = -3;
pub const NTG_ERROR_ASYNC_NOT_READY: i32 = -4;

// Enums

pub type ntg_error_code_enum = c_int;
pub type ntg_media_source_enum = c_int;

pub const NTG_FILE: ntg_media_source_enum = 1 << 0;
pub const NTG_SHELL: ntg_media_source_enum = 1 << 1;
pub const NTG_FFMPEG: ntg_media_source_enum = 1 << 2;
pub const NTG_DEVICE: ntg_media_source_enum = 1 << 3;
pub const NTG_DESKTOP: ntg_media_source_enum = 1 << 4;
pub const NTG_EXTERNAL: ntg_media_source_enum = 1 << 5;

pub type ntg_stream_device_enum = c_int;
pub const NTG_STREAM_MICROPHONE: ntg_stream_device_enum = 0;
pub const NTG_STREAM_SPEAKER: ntg_stream_device_enum = 1;
pub const NTG_STREAM_CAMERA: ntg_stream_device_enum = 2;
pub const NTG_STREAM_SCREEN: ntg_stream_device_enum = 3;

pub type ntg_stream_mode_enum = c_int;
pub const NTG_STREAM_CAPTURE: ntg_stream_mode_enum = 0;
pub const NTG_STREAM_PLAYBACK: ntg_stream_mode_enum = 1;

pub type ntg_stream_type_enum = c_int;
pub const NTG_STREAM_AUDIO: ntg_stream_type_enum = 0;
pub const NTG_STREAM_VIDEO: ntg_stream_type_enum = 1;

pub type ntg_stream_status_enum = c_int;
pub const NTG_ACTIVE: ntg_stream_status_enum = 0;
pub const NTG_PAUSED: ntg_stream_status_enum = 1;
pub const NTG_IDLING: ntg_stream_status_enum = 2;

pub type ntg_connection_state_enum = c_int;
pub const NTG_STATE_CONNECTING: ntg_connection_state_enum = 0;
pub const NTG_STATE_CONNECTED: ntg_connection_state_enum = 1;
pub const NTG_STATE_TIMEOUT: ntg_connection_state_enum = 2;
pub const NTG_STATE_FAILED: ntg_connection_state_enum = 3;
pub const NTG_STATE_CLOSED: ntg_connection_state_enum = 4;

pub type ntg_connection_kind_enum = c_int;
pub const NTG_KIND_NORMAL: ntg_connection_kind_enum = 0;
pub const NTG_KIND_PRESENTATION: ntg_connection_kind_enum = 1;

pub type ntg_media_segment_quality_enum = c_int;
pub const NTG_MEDIA_SEGMENT_QUALITY_NONE: ntg_media_segment_quality_enum = 0;
pub const NTG_MEDIA_SEGMENT_QUALITY_THUMBNAIL: ntg_media_segment_quality_enum = 1;
pub const NTG_MEDIA_SEGMENT_QUALITY_MEDIUM: ntg_media_segment_quality_enum = 2;
pub const NTG_MEDIA_SEGMENT_QUALITY_FULL: ntg_media_segment_quality_enum = 3;

pub type ntg_media_segment_status_enum = c_int;
pub const NTG_MEDIA_SEGMENT_NOT_READY: ntg_media_segment_status_enum = 0;
pub const NTG_MEDIA_SEGMENT_RESYNC_NEEDED: ntg_media_segment_status_enum = 1;
pub const NTG_MEDIA_SEGMENT_SUCCESS: ntg_media_segment_status_enum = 2;

pub type ntg_connection_mode_enum = c_int;
pub const NTG_CONNECTION_MODE_NONE: ntg_connection_mode_enum = 0;
pub const NTG_CONNECTION_MODE_RTC: ntg_connection_mode_enum = 1;
pub const NTG_CONNECTION_MODE_STREAM: ntg_connection_mode_enum = 2;
pub const NTG_CONNECTION_MODE_RTMP: ntg_connection_mode_enum = 3;

pub type ntg_log_level_enum = c_int;
pub const NTG_LOG_DEBUG: ntg_log_level_enum = 1 << 0;
pub const NTG_LOG_INFO: ntg_log_level_enum = 1 << 1;
pub const NTG_LOG_WARNING: ntg_log_level_enum = 1 << 2;
pub const NTG_LOG_ERROR: ntg_log_level_enum = 1 << 3;
pub const NTG_LOG_UNKNOWN: ntg_log_level_enum = -1;

pub type ntg_log_source_enum = c_int;
pub const NTG_LOG_WEBRTC: ntg_log_source_enum = 1 << 0;
pub const NTG_LOG_SELF: ntg_log_source_enum = 1 << 1;

// Structs

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ntg_network_info_struct {
    pub kind: ntg_connection_kind_enum,
    pub state: ntg_connection_state_enum,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_audio_description_struct {
    pub mediaSource: ntg_media_source_enum,
    pub input: *mut c_char,
    pub sampleRate: u32,
    pub channelCount: u8,
    pub keepOpen: bool,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_video_description_struct {
    pub mediaSource: ntg_media_source_enum,
    pub input: *mut c_char,
    pub width: i16,
    pub height: i16,
    pub fps: u8,
    pub keepOpen: bool,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_auth_params_struct {
    pub g_a_or_b: *mut u8,
    pub sizeGAB: c_int,
    pub key_fingerprint: i64,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_media_description_struct {
    pub microphone: *mut ntg_audio_description_struct,
    pub speaker: *mut ntg_audio_description_struct,
    pub camera: *mut ntg_video_description_struct,
    pub screen: *mut ntg_video_description_struct,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ntg_call_info_struct {
    pub chatId: i64,
    pub capture: ntg_stream_status_enum,
    pub playback: ntg_stream_status_enum,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ntg_media_state_struct {
    pub muted: bool,
    pub videoPaused: bool,
    pub videoStopped: bool,
    pub presentationPaused: bool,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_rtc_server_struct {
    pub id: u64,
    pub ipv4: *mut c_char,
    pub ipv6: *mut c_char,
    pub username: *mut c_char,
    pub password: *mut c_char,
    pub port: u16,
    pub turn: bool,
    pub stun: bool,
    pub tcp: bool,
    pub peerTag: *mut u8,
    pub peerTagSize: c_int,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_protocol_struct {
    pub minLayer: i32,
    pub maxLayer: i32,
    pub udpP2P: bool,
    pub udpReflector: bool,
    pub libraryVersions: *mut *mut c_char,
    pub libraryVersionsSize: c_int,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_dh_config_struct {
    pub g: i32,
    pub p: *const u8,
    pub sizeP: c_int,
    pub random: *const u8,
    pub sizeRandom: c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ntg_frame_data_struct {
    pub absoluteCaptureTimestampMs: i64,
    pub width: u16,
    pub height: u16,
    pub rotation: u16,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ntg_remote_source_struct {
    pub ssrc: u32,
    pub state: ntg_stream_status_enum,
    pub device: ntg_stream_device_enum,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_ssrc_group_struct {
    pub semantics: *mut c_char,
    pub ssrcs: *mut u32,
    pub sizeSsrcs: c_int,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_device_info_struct {
    pub name: *mut c_char,
    pub metadata: *mut c_char,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_media_devices_struct {
    pub microphone: *mut ntg_device_info_struct,
    pub sizeMicrophone: c_int,
    pub speaker: *mut ntg_device_info_struct,
    pub sizeSpeaker: c_int,
    pub camera: *mut ntg_device_info_struct,
    pub sizeCamera: c_int,
    pub screen: *mut ntg_device_info_struct,
    pub sizeScreen: c_int,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_frame_struct {
    pub ssrc: i64,
    pub data: *mut u8,
    pub sizeData: c_int,
    pub frameData: ntg_frame_data_struct,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ntg_segment_part_request_struct {
    pub segmentId: i64,
    pub partId: i32,
    pub limit: i32,
    pub timestamp: i64,
    pub qualityUpdate: bool,
    pub channelId: i32,
    pub quality: ntg_media_segment_quality_enum,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ntg_log_message_struct {
    pub level: ntg_log_level_enum,
    pub source: ntg_log_source_enum,
    pub file: *mut c_char,
    pub line: u32,
    pub message: *mut c_char,
}

// Async callback machinery

pub type ntg_async_callback = Option<unsafe extern "C" fn(*mut std::os::raw::c_void)>;

#[repr(C)]
pub struct ntg_async_struct {
    pub userData: *mut std::os::raw::c_void,
    pub errorCode: *mut c_int,
    pub errorMessage: *mut *mut c_char,
    pub promise: ntg_async_callback,
}

// Event callbacks

pub type ntg_stream_callback = Option<
    unsafe extern "C" fn(
        uintptr_t,
        i64,
        ntg_stream_type_enum,
        ntg_stream_device_enum,
        *mut std::os::raw::c_void,
    ),
>;

pub type ntg_upgrade_callback = Option<
    unsafe extern "C" fn(
        uintptr_t,
        i64,
        ntg_media_state_struct,
        *mut std::os::raw::c_void,
    ),
>;

pub type ntg_connection_callback = Option<
    unsafe extern "C" fn(
        uintptr_t,
        i64,
        ntg_network_info_struct,
        *mut std::os::raw::c_void,
    ),
>;

pub type ntg_signaling_callback = Option<
    unsafe extern "C" fn(
        uintptr_t,
        i64,
        *mut u8,
        c_int,
        *mut std::os::raw::c_void,
    ),
>;

pub type ntg_frame_callback = Option<
    unsafe extern "C" fn(
        uintptr_t,
        i64,
        ntg_stream_mode_enum,
        ntg_stream_device_enum,
        *mut ntg_frame_struct,
        u64,
        *mut std::os::raw::c_void,
    ),
>;

pub type ntg_remote_source_callback = Option<
    unsafe extern "C" fn(
        uintptr_t,
        i64,
        ntg_remote_source_struct,
        *mut std::os::raw::c_void,
    ),
>;

pub type ntg_broadcast_timestamp_callback = Option<
    unsafe extern "C" fn(uintptr_t, i64, *mut std::os::raw::c_void),
>;

pub type ntg_broadcast_part_callback = Option<
    unsafe extern "C" fn(
        uintptr_t,
        i64,
        ntg_segment_part_request_struct,
        *mut std::os::raw::c_void,
    ),
>;

pub type ntg_log_message_callback = Option<unsafe extern "C" fn(ntg_log_message_struct)>;

// uintptr_t alias for clarity
pub type uintptr_t = usize;

// Extern functions

unsafe extern "C" {
    pub fn ntg_register_logger(callback: ntg_log_message_callback);

    #[must_use]
    pub fn ntg_init() -> uintptr_t;

    pub fn ntg_destroy(ptr: uintptr_t) -> c_int;

    // Group call flow
    pub fn ntg_create(
        ptr: uintptr_t,
        chatID: i64,
        buffer: *mut *mut c_char,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_connect(
        ptr: uintptr_t,
        chatID: i64,
        params: *mut c_char,
        isPresentation: bool,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_get_protocol(buffer: *mut ntg_protocol_struct) -> c_int;

    pub fn ntg_set_stream_sources(
        ptr: uintptr_t,
        chatID: i64,
        streamMode: ntg_stream_mode_enum,
        desc: ntg_media_description_struct,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_pause(ptr: uintptr_t, chatID: i64, future: ntg_async_struct) -> c_int;

    pub fn ntg_resume(ptr: uintptr_t, chatID: i64, future: ntg_async_struct) -> c_int;

    pub fn ntg_mute(ptr: uintptr_t, chatID: i64, future: ntg_async_struct) -> c_int;

    pub fn ntg_unmute(ptr: uintptr_t, chatID: i64, future: ntg_async_struct) -> c_int;

    pub fn ntg_stop(ptr: uintptr_t, chatID: i64, future: ntg_async_struct) -> c_int;

    pub fn ntg_time(
        ptr: uintptr_t,
        chatID: i64,
        streamMode: ntg_stream_mode_enum,
        time: *mut i64,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_get_state(
        ptr: uintptr_t,
        chatID: i64,
        mediaState: *mut ntg_media_state_struct,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_get_connection_mode(
        ptr: uintptr_t,
        chatID: i64,
        mode: *mut ntg_connection_mode_enum,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_calls(
        ptr: uintptr_t,
        buffer: *mut *mut ntg_call_info_struct,
        size: *mut c_int,
        future: ntg_async_struct,
    ) -> c_int;

    // P2P call flow
    pub fn ntg_create_p2p(
        ptr: uintptr_t,
        userId: i64,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_init_exchange(
        ptr: uintptr_t,
        userId: i64,
        dhConfig: *mut ntg_dh_config_struct,
        g_a_hash: *const u8,
        sizeGAHash: c_int,
        buffer: *mut *mut u8,
        size: *mut c_int,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_exchange_keys(
        ptr: uintptr_t,
        userId: i64,
        g_a_or_b: *const u8,
        sizeGAB: c_int,
        fingerprint: i64,
        buffer: *mut ntg_auth_params_struct,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_skip_exchange(
        ptr: uintptr_t,
        userId: i64,
        encryptionKey: *const u8,
        size: c_int,
        isOutgoing: bool,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_connect_p2p(
        ptr: uintptr_t,
        userId: i64,
        servers: *mut ntg_rtc_server_struct,
        serversSize: c_int,
        versions: *mut *mut c_char,
        versionsSize: c_int,
        p2pAllowed: bool,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_send_signaling_data(
        ptr: uintptr_t,
        userId: i64,
        buffer: *mut u8,
        size: c_int,
        future: ntg_async_struct,
    ) -> c_int;

    // Presentation
    pub fn ntg_init_presentation(
        ptr: uintptr_t,
        chatId: i64,
        buffer: *mut *mut c_char,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_stop_presentation(
        ptr: uintptr_t,
        chatId: i64,
        future: ntg_async_struct,
    ) -> c_int;

    // Incoming video
    pub fn ntg_add_incoming_video(
        ptr: uintptr_t,
        chatId: i64,
        endpoint: *mut c_char,
        ssrcGroups: *mut ntg_ssrc_group_struct,
        size: c_int,
        buffer: *mut u32,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_remove_incoming_video(
        ptr: uintptr_t,
        chatId: i64,
        endpoint: *mut c_char,
        future: ntg_async_struct,
    ) -> c_int;

    // External frame / broadcast
    pub fn ntg_send_external_frame(
        ptr: uintptr_t,
        chatID: i64,
        device: ntg_stream_device_enum,
        frame: *mut u8,
        frameSize: c_int,
        frameData: ntg_frame_data_struct,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_send_broadcast_timestamp(
        ptr: uintptr_t,
        chatId: i64,
        timestamp: i64,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_send_broadcast_part(
        ptr: uintptr_t,
        chatId: i64,
        segmentId: i64,
        partId: i32,
        status: ntg_media_segment_status_enum,
        qualityUpdate: bool,
        frame: *const u8,
        frameSize: c_int,
        future: ntg_async_struct,
    ) -> c_int;

    // Device enumeration
    pub fn ntg_get_media_devices(buffer: *mut ntg_media_devices_struct) -> c_int;

    // Misc
    pub fn ntg_get_version(buffer: *mut *mut c_char) -> c_int;

    pub fn ntg_cpu_usage(
        ptr: uintptr_t,
        buffer: *mut c_double,
        future: ntg_async_struct,
    ) -> c_int;

    pub fn ntg_enable_g_lib_loop(enable: bool) -> c_int;

    // Event hooks
    pub fn ntg_on_stream_end(
        ptr: uintptr_t,
        callback: ntg_stream_callback,
        userData: *mut std::os::raw::c_void,
    ) -> c_int;

    pub fn ntg_on_upgrade(
        ptr: uintptr_t,
        callback: ntg_upgrade_callback,
        userData: *mut std::os::raw::c_void,
    ) -> c_int;

    pub fn ntg_on_connection_change(
        ptr: uintptr_t,
        callback: ntg_connection_callback,
        userData: *mut std::os::raw::c_void,
    ) -> c_int;

    pub fn ntg_on_signaling_data(
        ptr: uintptr_t,
        callback: ntg_signaling_callback,
        userData: *mut std::os::raw::c_void,
    ) -> c_int;

    pub fn ntg_on_frames(
        ptr: uintptr_t,
        callback: ntg_frame_callback,
        userData: *mut std::os::raw::c_void,
    ) -> c_int;

    pub fn ntg_on_remote_source_change(
        ptr: uintptr_t,
        callback: ntg_remote_source_callback,
        userData: *mut std::os::raw::c_void,
    ) -> c_int;

    pub fn ntg_on_request_broadcast_timestamp(
        ptr: uintptr_t,
        callback: ntg_broadcast_timestamp_callback,
        userData: *mut std::os::raw::c_void,
    ) -> c_int;

    pub fn ntg_on_request_broadcast_part(
        ptr: uintptr_t,
        callback: ntg_broadcast_part_callback,
        userData: *mut std::os::raw::c_void,
    ) -> c_int;
}
