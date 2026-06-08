//! Safe Rust bindings to [NTgCalls](https://github.com/pytgcalls/ntgcalls).
//!
//! Covers group calls, P2P calls, screen sharing, external frames, and
//! broadcast (channel stream) reception.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use tgcalls::{TgCalls, MediaDescription, AudioDescription, MediaSource, StreamMode};
//!
//! let tg = TgCalls::try_new()?;
//! let params = tg.create(chat_id)?;
//! // pass params to phone.joinGroupCall, get transport JSON back
//! tg.connect(chat_id, transport_json, false)?;
//!
//! let desc = MediaDescription {
//!     microphone: Some(AudioDescription::new(
//!         MediaSource::File, "/path/to/audio.raw", 48000, 2, false,
//!     )),
//!     ..Default::default()
//! };
//! tg.set_stream_sources(chat_id, StreamMode::Capture, desc)?;
//! tg.stop(chat_id)?;
//! ```
//!
//! # Version compatibility
//!
//! [`TgCalls::try_new`] returns an error if the loaded `.so` version does not
//! match the compiled version. See `docs/upgrade.md`.

use std::{
    ffi::CStr,
    mem::MaybeUninit,
    os::raw::{c_char, c_int, c_void},
    sync::Arc,
};

use ntgcalls::{
    ntg_add_incoming_video, ntg_async_struct, ntg_auth_params_struct, ntg_calls, ntg_connect,
    ntg_connect_p2p, ntg_cpu_usage, ntg_create, ntg_create_p2p, ntg_destroy, ntg_enable_g_lib_loop,
    ntg_exchange_keys, ntg_frame_struct, ntg_get_connection_mode, ntg_get_media_devices,
    ntg_get_protocol, ntg_get_state, ntg_get_version, ntg_init, ntg_init_exchange,
    ntg_init_presentation, ntg_media_devices_struct, ntg_media_state_struct,
    ntg_media_state_struct as NtgMediaState, ntg_mute, ntg_network_info_struct,
    ntg_on_connection_change, ntg_on_stream_end, ntg_on_upgrade, ntg_pause, ntg_protocol_struct,
    ntg_remote_source_struct, ntg_remove_incoming_video, ntg_resume,
    ntg_segment_part_request_struct, ntg_send_broadcast_part, ntg_send_broadcast_timestamp,
    ntg_send_external_frame, ntg_send_signaling_data, ntg_set_stream_sources, ntg_skip_exchange,
    ntg_stop, ntg_stop_presentation, ntg_stream_device_enum, ntg_stream_mode_enum,
    ntg_stream_type_enum, ntg_time, ntg_unmute, uintptr_t,
};

pub mod enums;
pub mod errors;
mod logger;
pub mod structures;
pub mod utils;

pub use enums::{
    ConnectionKind, ConnectionMode, ConnectionState, MediaSegmentQuality, MediaSegmentStatus,
    MediaSource, StreamDevice, StreamMode, StreamStatus, StreamType,
};
pub use errors::{CallError, Result};
pub use structures::{
    AudioDescription, AuthParams, CallInfo, DeviceInfo, DhConfig, FrameData, MediaDescription,
    MediaDevices, MediaState, NetworkInfo, RemoteSource, RtcServer, SegmentPartRequest, SsrcGroup,
    VideoDescription,
};
use structures::{FfiDhConfig, FfiMediaDesc, FfiRtcServer, FfiSsrcGroup};
pub use utils::IntoCString;

// Callback type aliases.

/// Called when a stream ends (EOF, error, or explicit stop).
pub type StreamCallback = Option<
    unsafe extern "C" fn(uintptr_t, i64, ntg_stream_type_enum, ntg_stream_device_enum, *mut c_void),
>;

/// Called on stream quality upgrade.
pub type UpgradeCallback = Option<unsafe extern "C" fn(uintptr_t, i64, NtgMediaState, *mut c_void)>;

/// Called on WebRTC connection state change.
pub type ConnectionCallback =
    Option<unsafe extern "C" fn(uintptr_t, i64, ntg_network_info_struct, *mut c_void)>;

/// Called when ntgcalls produces outgoing signaling bytes.
/// Forward them to the remote peer via `phone.sendSignalingData`.
pub type SignalingCallback =
    Option<unsafe extern "C" fn(uintptr_t, i64, *mut u8, c_int, *mut c_void)>;

/// Called when incoming video frames arrive.
pub type FrameCallback = Option<
    unsafe extern "C" fn(
        uintptr_t,
        i64,
        ntg_stream_mode_enum,
        ntg_stream_device_enum,
        *mut ntg_frame_struct,
        u64,
        *mut c_void,
    ),
>;

/// Called when a remote participant's source changes.
pub type RemoteSourceCallback =
    Option<unsafe extern "C" fn(uintptr_t, i64, ntg_remote_source_struct, *mut c_void)>;

/// Called when ntgcalls needs the current broadcast timestamp.
/// Respond with [`TgCalls::send_broadcast_timestamp`].
pub type BroadcastTimestampCallback = Option<unsafe extern "C" fn(uintptr_t, i64, *mut c_void)>;

/// Called when ntgcalls needs a broadcast segment part.
/// Respond with [`TgCalls::send_broadcast_part`].
pub type BroadcastPartCallback =
    Option<unsafe extern "C" fn(uintptr_t, i64, ntg_segment_part_request_struct, *mut c_void)>;

struct AsyncCtx {
    tx: std::sync::mpsc::SyncSender<()>,
    error_code: c_int,
    _error_message: *mut c_char,
}

unsafe impl Send for AsyncCtx {}

unsafe extern "C" fn async_promise(user_data: *mut c_void) {
    let ctx = unsafe { &*(user_data as *mut AsyncCtx) };
    let _ = ctx.tx.send(());
}

fn call_async<F>(f: F) -> Result<()>
where
    F: FnOnce(ntg_async_struct) -> c_int,
{
    let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
    let ctx = Box::into_raw(Box::new(AsyncCtx {
        tx,
        error_code: 0,
        _error_message: std::ptr::null_mut(),
    }));

    let future = ntg_async_struct {
        userData: ctx as *mut _,
        errorCode: unsafe { &mut (*ctx).error_code as *mut _ },
        errorMessage: unsafe { &mut (*ctx)._error_message as *mut _ },
        promise: Some(async_promise),
    };

    let dispatch_rc = f(future);
    if dispatch_rc < 0 {
        unsafe { drop(Box::from_raw(ctx)) };
        return Err(CallError::from(dispatch_rc));
    }

    rx.recv().expect("NTgCalls async promise never called");

    let error_code = unsafe { (*ctx).error_code };
    unsafe { drop(Box::from_raw(ctx)) };

    if error_code < 0 {
        Err(CallError::from(error_code))
    } else {
        Ok(())
    }
}

struct Inner(uintptr_t);

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = unsafe { ntg_destroy(self.0) };
    }
}

/// A handle to an NTgCalls instance.
///
/// Cheap to clone. All clones share the same underlying instance via `Arc`.
/// The instance is destroyed when the last clone is dropped.
///
/// Create with [`TgCalls::try_new`].
#[derive(Clone)]
pub struct TgCalls {
    inner: Arc<Inner>,
}

impl TgCalls {
    /// Create a new NTgCalls instance.
    ///
    /// Registers the ntgcalls logger (once, globally) and checks that the
    /// runtime `.so` version matches the pinned version.
    ///
    /// # Errors
    ///
    /// Returns [`CallError::VersionMismatch`] if the versions don't match.
    /// Upgrade via the crate version, not `TGCALLS_NTGCALLS_VERSION`.
    pub fn try_new() -> Result<Self> {
        logger::init();

        #[cfg(feature = "bundled")]
        {
            const BUNDLED: &str = env!("NTGCALLS_BUNDLED_VERSION");
            if BUNDLED != "local" {
                let runtime = Self::version_raw();
                if runtime != BUNDLED {
                    return Err(CallError::VersionMismatch {
                        compiled: BUNDLED.to_string(),
                        loaded: runtime,
                    });
                }
            }
        }

        Ok(Self {
            inner: Arc::new(Inner(unsafe { ntg_init() })),
        })
    }

    // Calls ntg_get_version before ntg_init so no instance is needed.
    fn version_raw() -> String {
        let mut buf: *mut c_char = std::ptr::null_mut();
        unsafe {
            ntg_get_version(&mut buf);
            if buf.is_null() {
                return String::new();
            }
            CStr::from_ptr(buf).to_string_lossy().into_owned()
        }
    }

    /// Returns the ntgcalls version string from the loaded shared library.
    pub fn version() -> String {
        Self::version_raw()
    }

    /// Returns the WebRTC protocol descriptor for this ntgcalls build.
    ///
    /// Pass the result to `phone.joinGroupCall` to advertise supported layers.
    pub fn protocol() -> Result<Protocol> {
        let mut proto: MaybeUninit<ntg_protocol_struct> = MaybeUninit::uninit();
        let rc = unsafe { ntg_get_protocol(proto.as_mut_ptr()) };
        if rc < 0 {
            return Err(CallError::from(rc));
        }
        let p = unsafe { proto.assume_init() };
        let versions = unsafe {
            (0..p.libraryVersionsSize as usize)
                .map(|i| {
                    CStr::from_ptr(*p.libraryVersions.add(i))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        };
        Ok(Protocol {
            min_layer: p.minLayer,
            max_layer: p.maxLayer,
            udp_p2p: p.udpP2P,
            udp_reflector: p.udpReflector,
            library_versions: versions,
        })
    }

    /// Enable or disable GLib main loop integration.
    ///
    /// Needed on some Linux desktop environments for device enumeration.
    pub fn enable_g_lib_loop(enable: bool) -> Result<()> {
        let rc = unsafe { ntg_enable_g_lib_loop(enable) };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }

    /// List all available audio and video devices on the host.
    pub fn media_devices() -> Result<MediaDevices> {
        let mut raw: MaybeUninit<ntg_media_devices_struct> = MaybeUninit::uninit();
        let rc = unsafe { ntg_get_media_devices(raw.as_mut_ptr()) };
        if rc < 0 {
            return Err(CallError::from(rc));
        }
        Ok(unsafe { MediaDevices::from_ffi(raw.assume_init()) })
    }
}

impl TgCalls {
    /// Prepare to join a group call for `chat_id`.
    ///
    /// Returns params JSON for `phone.joinGroupCall`. Call [`connect`] after
    /// Telegram returns the transport JSON.
    ///
    /// [`connect`]: TgCalls::connect
    pub fn create(&self, chat_id: i64) -> Result<String> {
        let mut buf: *mut c_char = std::ptr::null_mut();
        let buf_ptr = &mut buf as *mut *mut c_char;
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_create(ptr, chat_id, buf_ptr, future) })?;
        if buf.is_null() {
            return Err(CallError::InvalidParams);
        }
        Ok(unsafe { CStr::from_ptr(buf).to_string_lossy().into_owned() })
    }

    /// Connect to a group call using Telegram's transport JSON.
    ///
    /// Set `is_presentation` to `true` for a screen-share connection.
    pub fn connect<S: IntoCString>(
        &self,
        chat_id: i64,
        params: S,
        is_presentation: bool,
    ) -> Result<()> {
        let c_params = params.into_c_string();
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_connect(
                ptr,
                chat_id,
                c_params.as_ptr() as *mut _,
                is_presentation,
                future,
            )
        })
    }

    /// Set audio/video stream sources for an active call.
    pub fn set_stream_sources(
        &self,
        chat_id: i64,
        stream_mode: StreamMode,
        desc: MediaDescription,
    ) -> Result<()> {
        let mut ffi = FfiMediaDesc::new(&desc);
        let ffi_desc = ffi.as_ffi();
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_set_stream_sources(ptr, chat_id, stream_mode as i32, ffi_desc, future)
        })
    }

    /// Stop streaming and leave the call for `chat_id`.
    pub fn stop(&self, chat_id: i64) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_stop(ptr, chat_id, future) })
    }

    /// Pause the active stream for `chat_id`.
    pub fn pause(&self, chat_id: i64) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_pause(ptr, chat_id, future) })
    }

    /// Resume a paused stream for `chat_id`.
    pub fn resume(&self, chat_id: i64) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_resume(ptr, chat_id, future) })
    }

    /// Mute the microphone for `chat_id`.
    pub fn mute(&self, chat_id: i64) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_mute(ptr, chat_id, future) })
    }

    /// Unmute the microphone for `chat_id`.
    pub fn unmute(&self, chat_id: i64) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_unmute(ptr, chat_id, future) })
    }

    /// Returns milliseconds played for the given stream mode.
    pub fn played_time(&self, chat_id: i64, stream_mode: StreamMode) -> Result<i64> {
        let mut time: i64 = 0;
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_time(ptr, chat_id, stream_mode as i32, &mut time, future)
        })?;
        Ok(time)
    }

    /// Returns the current mute/pause/stop state for `chat_id`.
    pub fn media_state(&self, chat_id: i64) -> Result<MediaState> {
        let mut buf: MaybeUninit<ntg_media_state_struct> = MaybeUninit::uninit();
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_get_state(ptr, chat_id, buf.as_mut_ptr(), future) })?;
        let s = unsafe { buf.assume_init() };
        Ok(MediaState {
            muted: s.muted,
            video_paused: s.videoPaused,
            video_stopped: s.videoStopped,
            presentation_paused: s.presentationPaused,
        })
    }

    /// Returns the transport mode for `chat_id`.
    pub fn connection_mode(&self, chat_id: i64) -> Result<ConnectionMode> {
        let mut mode: i32 = 0;
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_get_connection_mode(ptr, chat_id, &mut mode, future) })?;
        Ok(unsafe { std::mem::transmute::<i32, enums::ConnectionMode>(mode) })
    }

    /// Returns info about all active calls managed by this instance.
    pub fn calls(&self) -> Result<Vec<CallInfo>> {
        let mut buf: *mut ntgcalls::ntg_call_info_struct = std::ptr::null_mut();
        let mut size: c_int = 0;
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_calls(ptr, &mut buf, &mut size, future) })?;
        let result = unsafe {
            std::slice::from_raw_parts(buf, size as usize)
                .iter()
                .map(|s| CallInfo::from(*s))
                .collect()
        };
        Ok(result)
    }

    /// Returns the current CPU usage of ntgcalls as a percentage (0.0 to 100.0).
    pub fn cpu_usage(&self) -> Result<f64> {
        let mut usage: f64 = 0.0;
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_cpu_usage(ptr, &mut usage, future) })?;
        Ok(usage)
    }

    /// Prepare a screen-share presentation for `chat_id`.
    ///
    /// Returns JSON for `phone.joinGroupCallPresentation`. Requires an active
    /// group call on the same `chat_id` first. Call [`connect`] with
    /// `is_presentation = true` after Telegram returns the transport JSON.
    ///
    /// [`connect`]: TgCalls::connect
    pub fn init_presentation(&self, chat_id: i64) -> Result<String> {
        let mut buf: *mut c_char = std::ptr::null_mut();
        let buf_ptr = &mut buf as *mut *mut c_char;
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_init_presentation(ptr, chat_id, buf_ptr, future) })?;
        if buf.is_null() {
            return Err(CallError::InvalidParams);
        }
        Ok(unsafe { CStr::from_ptr(buf).to_string_lossy().into_owned() })
    }

    /// Stop the screen-share presentation for `chat_id`.
    pub fn stop_presentation(&self, chat_id: i64) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_stop_presentation(ptr, chat_id, future) })
    }

    /// Subscribe to a remote participant's video stream.
    ///
    /// Returns the assigned local SSRC for this video track.
    pub fn add_incoming_video(
        &self,
        chat_id: i64,
        endpoint: impl IntoCString,
        ssrc_groups: &[SsrcGroup],
    ) -> Result<u32> {
        let endpoint_cs = endpoint.into_c_string();
        let mut ffi_groups: Vec<FfiSsrcGroup> = ssrc_groups.iter().map(FfiSsrcGroup::new).collect();
        let mut ffi_ptrs: Vec<ntgcalls::ntg_ssrc_group_struct> =
            ffi_groups.iter_mut().map(|g| g.ffi.clone()).collect();
        let mut out_ssrc: u32 = 0;
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_add_incoming_video(
                ptr,
                chat_id,
                endpoint_cs.as_ptr() as *mut _,
                if ffi_ptrs.is_empty() {
                    std::ptr::null_mut()
                } else {
                    ffi_ptrs.as_mut_ptr()
                },
                ffi_ptrs.len() as c_int,
                &mut out_ssrc,
                future,
            )
        })?;
        Ok(out_ssrc)
    }

    /// Unsubscribe from a remote participant's video stream.
    pub fn remove_incoming_video(&self, chat_id: i64, endpoint: impl IntoCString) -> Result<()> {
        let endpoint_cs = endpoint.into_c_string();
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_remove_incoming_video(ptr, chat_id, endpoint_cs.as_ptr() as *mut _, future)
        })
    }

    /// Push a raw frame when using [`MediaSource::External`].
    ///
    /// For video devices (`Camera`, `Screen`): raw YUV420P bytes.
    /// For audio devices (`Microphone`, `Speaker`): raw s16le PCM bytes.
    pub fn send_external_frame(
        &self,
        chat_id: i64,
        device: StreamDevice,
        frame: &mut [u8],
        frame_data: FrameData,
    ) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_send_external_frame(
                ptr,
                chat_id,
                device as i32,
                frame.as_mut_ptr(),
                frame.len() as c_int,
                frame_data.to_ffi(),
                future,
            )
        })
    }

    /// Send the current broadcast timestamp. Called in response to [`BroadcastTimestampCallback`].
    pub fn send_broadcast_timestamp(&self, chat_id: i64, timestamp: i64) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_send_broadcast_timestamp(ptr, chat_id, timestamp, future)
        })
    }

    /// Deliver a broadcast segment part. Called in response to [`BroadcastPartCallback`].
    pub fn send_broadcast_part(
        &self,
        chat_id: i64,
        segment_id: i64,
        part_id: i32,
        status: MediaSegmentStatus,
        quality_update: bool,
        frame: &[u8],
    ) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_send_broadcast_part(
                ptr,
                chat_id,
                segment_id,
                part_id,
                status as i32,
                quality_update,
                frame.as_ptr(),
                frame.len() as c_int,
                future,
            )
        })
    }

    /// Prepare a P2P call with `user_id`.
    ///
    /// Call before [`init_exchange`] or [`skip_exchange`].
    ///
    /// [`init_exchange`]: TgCalls::init_exchange
    /// [`skip_exchange`]: TgCalls::skip_exchange
    pub fn create_p2p(&self, user_id: i64) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe { ntg_create_p2p(ptr, user_id, future) })
    }

    /// Connect a P2P call to STUN/TURN servers.
    ///
    /// Wire up [`on_signaling_data`] and [`on_connection_change`] before
    /// calling this. See `docs/p2p-flow.md`.
    ///
    /// [`on_signaling_data`]: TgCalls::on_signaling_data
    /// [`on_connection_change`]: TgCalls::on_connection_change
    pub fn connect_p2p(
        &self,
        user_id: i64,
        servers: &[RtcServer],
        versions: &[String],
        p2p_allowed: bool,
    ) -> Result<()> {
        let ffi_servers: Vec<FfiRtcServer> = servers.iter().map(FfiRtcServer::new).collect();
        let mut ffi_server_ptrs: Vec<ntgcalls::ntg_rtc_server_struct> =
            ffi_servers.iter().map(|s| s.ffi.clone()).collect();
        let cstrings: Vec<std::ffi::CString> = versions
            .iter()
            .map(|v| std::ffi::CString::new(v.as_str()).unwrap_or_default())
            .collect();
        let mut ver_ptrs: Vec<*mut c_char> =
            cstrings.iter().map(|cs| cs.as_ptr() as *mut _).collect();
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_connect_p2p(
                ptr,
                user_id,
                if ffi_server_ptrs.is_empty() {
                    std::ptr::null_mut()
                } else {
                    ffi_server_ptrs.as_mut_ptr()
                },
                ffi_server_ptrs.len() as c_int,
                if ver_ptrs.is_empty() {
                    std::ptr::null_mut()
                } else {
                    ver_ptrs.as_mut_ptr()
                },
                ver_ptrs.len() as c_int,
                p2p_allowed,
                future,
            )
        })
    }

    /// Forward incoming signaling bytes from Telegram to ntgcalls.
    ///
    /// Call when `updatePhoneCallSignalingData` arrives.
    pub fn send_signaling_data(&self, user_id: i64, data: &mut [u8]) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_send_signaling_data(ptr, user_id, data.as_mut_ptr(), data.len() as c_int, future)
        })
    }

    /// Start DH key exchange for a P2P call.
    ///
    /// Pass an empty slice for `g_a_hash` when calling (outgoing); the return
    /// value is `SHA256(g_a)` to send in `phone.requestCall`.
    /// Pass the received `g_a_hash` when accepting (incoming); the return
    /// value is `g_b` to send in `phone.acceptCall`.
    /// See `docs/p2p-flow.md`.
    pub fn init_exchange(
        &self,
        user_id: i64,
        dh_config: &DhConfig,
        g_a_hash: &[u8],
    ) -> Result<Vec<u8>> {
        let mut ffi_dh = FfiDhConfig::new(dh_config);
        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_size: c_int = 0;
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_init_exchange(
                ptr,
                user_id,
                &mut ffi_dh.ffi,
                g_a_hash.as_ptr(),
                g_a_hash.len() as c_int,
                &mut out_buf,
                &mut out_size,
                future,
            )
        })?;
        if out_buf.is_null() || out_size <= 0 {
            return Ok(Vec::new());
        }
        Ok(unsafe { std::slice::from_raw_parts(out_buf, out_size as usize).to_vec() })
    }

    /// Complete DH key exchange.
    ///
    /// Returns [`AuthParams`] to pass to `phone.confirmCall`.
    pub fn exchange_keys(
        &self,
        user_id: i64,
        g_a_or_b: &[u8],
        fingerprint: i64,
    ) -> Result<AuthParams> {
        let mut raw: MaybeUninit<ntg_auth_params_struct> = MaybeUninit::uninit();
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_exchange_keys(
                ptr,
                user_id,
                g_a_or_b.as_ptr(),
                g_a_or_b.len() as c_int,
                fingerprint,
                raw.as_mut_ptr(),
                future,
            )
        })?;
        Ok(AuthParams::from_ffi(unsafe { raw.assume_init() }))
    }

    /// Skip DH exchange and use a pre-shared encryption key.
    ///
    /// `is_outgoing`: `true` if this side initiated the call.
    pub fn skip_exchange(
        &self,
        user_id: i64,
        encryption_key: &[u8],
        is_outgoing: bool,
    ) -> Result<()> {
        let ptr = self.inner.0;
        call_async(|future| unsafe {
            ntg_skip_exchange(
                ptr,
                user_id,
                encryption_key.as_ptr(),
                encryption_key.len() as c_int,
                is_outgoing,
                future,
            )
        })
    }

    /// # Safety
    /// `user_data` must outlive this instance and be safe to access from ntgcalls' threads.
    pub unsafe fn on_stream_end(
        &self,
        callback: StreamCallback,
        user_data: *mut c_void,
    ) -> Result<()> {
        let rc = unsafe { ntg_on_stream_end(self.inner.0, callback, user_data) };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }

    /// # Safety
    /// `user_data` must outlive this instance and be safe to access from ntgcalls' threads.
    pub unsafe fn on_upgrade(
        &self,
        callback: UpgradeCallback,
        user_data: *mut c_void,
    ) -> Result<()> {
        let rc = unsafe { ntg_on_upgrade(self.inner.0, callback, user_data) };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }

    /// [`ConnectionState::Connected`] fires after ICE and DTLS are both complete,
    /// not just after channel negotiation.
    ///
    /// # Safety
    /// `user_data` must outlive this instance and be safe to access from ntgcalls' threads.
    pub unsafe fn on_connection_change(
        &self,
        callback: ConnectionCallback,
        user_data: *mut c_void,
    ) -> Result<()> {
        let rc = unsafe { ntg_on_connection_change(self.inner.0, callback, user_data) };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }

    /// Forward these promptly. Delays stall ICE negotiation.
    ///
    /// # Safety
    /// `user_data` must outlive this instance and be safe to access from ntgcalls' threads.
    pub unsafe fn on_signaling_data(
        &self,
        callback: SignalingCallback,
        user_data: *mut c_void,
    ) -> Result<()> {
        let rc = unsafe { ntgcalls::ntg_on_signaling_data(self.inner.0, callback, user_data) };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }

    /// # Safety
    /// `user_data` must outlive this instance and be safe to access from ntgcalls' threads.
    pub unsafe fn on_frames(&self, callback: FrameCallback, user_data: *mut c_void) -> Result<()> {
        let rc = unsafe { ntgcalls::ntg_on_frames(self.inner.0, callback, user_data) };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }

    /// # Safety
    /// `user_data` must outlive this instance and be safe to access from ntgcalls' threads.
    pub unsafe fn on_remote_source_change(
        &self,
        callback: RemoteSourceCallback,
        user_data: *mut c_void,
    ) -> Result<()> {
        let rc =
            unsafe { ntgcalls::ntg_on_remote_source_change(self.inner.0, callback, user_data) };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }

    /// # Safety
    /// `user_data` must outlive this instance and be safe to access from ntgcalls' threads.
    pub unsafe fn on_request_broadcast_timestamp(
        &self,
        callback: BroadcastTimestampCallback,
        user_data: *mut c_void,
    ) -> Result<()> {
        let rc = unsafe {
            ntgcalls::ntg_on_request_broadcast_timestamp(self.inner.0, callback, user_data)
        };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }

    /// # Safety
    /// `user_data` must outlive this instance and be safe to access from ntgcalls' threads.
    pub unsafe fn on_request_broadcast_part(
        &self,
        callback: BroadcastPartCallback,
        user_data: *mut c_void,
    ) -> Result<()> {
        let rc =
            unsafe { ntgcalls::ntg_on_request_broadcast_part(self.inner.0, callback, user_data) };
        if rc < 0 {
            Err(CallError::from(rc))
        } else {
            Ok(())
        }
    }
}

/// WebRTC protocol descriptor for this ntgcalls build.
///
/// Returned by [`TgCalls::protocol`].
#[derive(Debug, Clone)]
pub struct Protocol {
    /// Minimum supported layer.
    pub min_layer: i32,
    /// Maximum supported layer.
    pub max_layer: i32,
    /// Whether UDP P2P is supported.
    pub udp_p2p: bool,
    /// Whether UDP via a reflector is supported.
    pub udp_reflector: bool,
    /// Supported library version strings.
    pub library_versions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!TgCalls::version().is_empty());
    }
}
