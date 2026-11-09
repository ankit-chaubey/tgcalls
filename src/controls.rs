//! One set of controls for every kind of call.
//!
//! [`Call`](crate::Call), [`P2PCall`](crate::P2PCall) and
//! [`ConferenceCall`](crate::ConferenceCall) have the same mid-call controls
//! under the same names, but not the same receivers (`Call` takes
//! `&mut self`, the others `&self`). [`CallControls`] puts them behind one
//! trait, so code that only needs to mute, switch video or share the screen
//! works with any of them:
//!
//! ```no_run
//! use tgcalls::{CallControls, Media, TgCallsError};
//!
//! // Works with a group call, a P2P call or a conference.
//! async fn start_presenting(call: &mut impl CallControls) -> Result<(), TgCallsError> {
//!     call.unmute().await?;
//!     call.set_screen(Some(Media::screen(1280, 720, 30))).await
//! }
//! ```
//!
//! # What each control means
//!
//! The controls follow ntgcalls' own model, one name per operation:
//!
//! | Slot | Sends / receives | Method |
//! |---|---|---|
//! | Microphone | the audio you send | `set_audio` |
//! | Camera | the video you send | `set_video` |
//! | Screen | a second, separate video track | `set_screen` |
//! | Speaker | where incoming audio goes (playback mode) | `set_sources(Playback, ..)` |
//!
//! `set_sources` replaces the whole set in one call; the three single-slot
//! setters change one slot and keep the rest. The slot names `microphone`
//! and `camera` still appear on `MediaDescription`, which belongs to
//! ntgcalls; `set_audio` fills `microphone` and `set_video` fills `camera`.
//!
//! The futures it returns are not `Send`: `ntgcalls::NTgCalls` isn't `Sync`
//! (see the top of `calls.rs`), so a `Call` or `P2PCall` has to stay on one
//! thread anyway. Await them where the call lives, or use [`Calls`](crate::Calls)
//! / [`ConferenceCalls`](crate::ConferenceCalls), which own a thread per chat
//! and are `Send`.
//!
//! The trait takes `&mut self` everywhere so `Call` fits. `Arc<ConferenceCall>`
//! and `Arc<P2PCall>` implement it too, for when you share one across tasks;
//! their own methods take `&self`, so you can also call those directly.
//!
//! [`Calls`](crate::Calls) and [`ConferenceCalls`](crate::ConferenceCalls)
//! have the same methods with a `chat_id` first, so they are not part of
//! this trait.

use std::future::Future;
use std::sync::Arc;

use ntgcalls::{
    AudioDescription, CallType, ConnectionMode, FrameData, MediaDescription, MediaState, SsrcGroup,
    StreamDevice, StreamMode, VideoDescription,
};

use crate::error::TgCallsError;
use crate::{Call, ConferenceCall, P2PCall};

/// Mid-call controls shared by every call type. Every method works at any
/// time while the call is up, in any order, and changing one slot keeps the
/// others (turning video on never drops the audio).
pub trait CallControls {
    /// Silences the mic and keeps its source.
    fn mute(&mut self) -> impl Future<Output = Result<(), TgCallsError>>;

    fn unmute(&mut self) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Pauses the outgoing stream (audio and video).
    fn pause(&mut self) -> impl Future<Output = Result<(), TgCallsError>>;

    fn resume(&mut self) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Sets, swaps (`Some`) or removes (`None`) the audio you send. To
    /// silence it while keeping the source, use [`CallControls::mute`].
    fn set_audio(
        &mut self,
        audio: Option<AudioDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Your video track on (`Some`), swapped, or off (`None`). Screen share
    /// is separate: see [`CallControls::set_screen`].
    fn set_video(
        &mut self,
        video: Option<VideoDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Screen share on (`Some`), swapped, or off (`None`). A separate track
    /// from [`CallControls::set_video`]; both can run at once.
    fn set_screen(
        &mut self,
        screen: Option<VideoDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Replaces everything being captured (`StreamMode::Capture`) or played
    /// back (`StreamMode::Playback`, e.g. a recording target) in one call.
    /// Anything missing from `media` is removed.
    fn set_sources(
        &mut self,
        mode: StreamMode,
        media: &MediaDescription,
    ) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Current mute / pause / video / screen flags.
    fn media_state(&mut self) -> impl Future<Output = Result<MediaState, TgCallsError>>;

    /// What is currently being captured (mic, camera, screen).
    fn current_media(&self) -> MediaDescription;

    /// Pushes one raw frame into a source configured with
    /// `MediaSource::External` (see `Media::external_video` and
    /// `Media::external_audio`).
    fn send_external_frame(
        &mut self,
        device: StreamDevice,
        frame: &[u8],
        frame_data: &FrameData,
    ) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Subscribes to a remote video stream. `user_id` is whose it is.
    fn add_incoming_video(
        &mut self,
        user_id: i64,
        endpoint: &str,
        ssrc_groups: &[SsrcGroup],
    ) -> impl Future<Output = Result<u32, TgCallsError>>;

    fn remove_incoming_video(
        &mut self,
        endpoint: &str,
    ) -> impl Future<Output = Result<bool, TgCallsError>>;

    /// Seconds of the given stream played so far.
    fn played_seconds(
        &mut self,
        mode: StreamMode,
    ) -> impl Future<Output = Result<u64, TgCallsError>>;

    fn call_type(&mut self) -> impl Future<Output = Result<CallType, TgCallsError>>;

    fn connection_mode(&mut self) -> impl Future<Output = Result<ConnectionMode, TgCallsError>>;

    /// CPU used by the native call stack, as a fraction of one core.
    fn cpu_usage(&mut self) -> impl Future<Output = Result<f64, TgCallsError>>;
}

impl CallControls for Call {
    fn mute(&mut self) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::mute(self)
    }
    fn unmute(&mut self) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::unmute(self)
    }
    fn pause(&mut self) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::pause(self)
    }
    fn resume(&mut self) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::resume(self)
    }
    fn set_audio(
        &mut self,
        audio: Option<AudioDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::set_audio(self, audio)
    }
    fn set_video(
        &mut self,
        video: Option<VideoDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::set_video(self, video)
    }
    fn set_screen(
        &mut self,
        screen: Option<VideoDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::set_screen(self, screen)
    }
    fn set_sources(
        &mut self,
        mode: StreamMode,
        media: &MediaDescription,
    ) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::set_sources(self, mode, media)
    }
    fn media_state(&mut self) -> impl Future<Output = Result<MediaState, TgCallsError>> {
        Call::media_state(self)
    }
    fn current_media(&self) -> MediaDescription {
        Call::current_media(self).clone()
    }
    fn send_external_frame(
        &mut self,
        device: StreamDevice,
        frame: &[u8],
        frame_data: &FrameData,
    ) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::send_external_frame(self, device, frame, frame_data)
    }
    fn add_incoming_video(
        &mut self,
        user_id: i64,
        endpoint: &str,
        ssrc_groups: &[SsrcGroup],
    ) -> impl Future<Output = Result<u32, TgCallsError>> {
        Call::add_incoming_video(self, user_id, endpoint, ssrc_groups)
    }
    fn remove_incoming_video(
        &mut self,
        endpoint: &str,
    ) -> impl Future<Output = Result<bool, TgCallsError>> {
        Call::remove_incoming_video(self, endpoint)
    }
    fn played_seconds(
        &mut self,
        mode: StreamMode,
    ) -> impl Future<Output = Result<u64, TgCallsError>> {
        Call::played_seconds(self, mode)
    }
    fn call_type(&mut self) -> impl Future<Output = Result<CallType, TgCallsError>> {
        Call::call_type(self)
    }
    fn connection_mode(&mut self) -> impl Future<Output = Result<ConnectionMode, TgCallsError>> {
        Call::connection_mode(self)
    }
    fn cpu_usage(&mut self) -> impl Future<Output = Result<f64, TgCallsError>> {
        Call::cpu_usage(self)
    }
}

/// Implements [`CallControls`] for a type whose own methods take `&self`.
/// `$get` turns `&Self` into `&$inner`.
macro_rules! shared_ref_controls {
    ($ty:ty, $inner:ty, $get:ident) => {
        impl CallControls for $ty {
            fn mute(&mut self) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::mute($get(&*self))
            }
            fn unmute(&mut self) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::unmute($get(&*self))
            }
            fn pause(&mut self) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::pause($get(&*self))
            }
            fn resume(&mut self) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::resume($get(&*self))
            }
            fn set_audio(
                &mut self,
                audio: Option<AudioDescription>,
            ) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::set_audio($get(&*self), audio)
            }
            fn set_video(
                &mut self,
                video: Option<VideoDescription>,
            ) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::set_video($get(&*self), video)
            }
            fn set_screen(
                &mut self,
                screen: Option<VideoDescription>,
            ) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::set_screen($get(&*self), screen)
            }
            fn set_sources(
                &mut self,
                mode: StreamMode,
                media: &MediaDescription,
            ) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::set_sources($get(&*self), mode, media)
            }
            fn media_state(&mut self) -> impl Future<Output = Result<MediaState, TgCallsError>> {
                <$inner>::media_state($get(&*self))
            }
            fn current_media(&self) -> MediaDescription {
                <$inner>::current_media($get(self))
            }
            fn send_external_frame(
                &mut self,
                device: StreamDevice,
                frame: &[u8],
                frame_data: &FrameData,
            ) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::send_external_frame($get(&*self), device, frame, frame_data)
            }
            fn add_incoming_video(
                &mut self,
                user_id: i64,
                endpoint: &str,
                ssrc_groups: &[SsrcGroup],
            ) -> impl Future<Output = Result<u32, TgCallsError>> {
                <$inner>::add_incoming_video($get(&*self), user_id, endpoint, ssrc_groups)
            }
            fn remove_incoming_video(
                &mut self,
                endpoint: &str,
            ) -> impl Future<Output = Result<bool, TgCallsError>> {
                <$inner>::remove_incoming_video($get(&*self), endpoint)
            }
            fn played_seconds(
                &mut self,
                mode: StreamMode,
            ) -> impl Future<Output = Result<u64, TgCallsError>> {
                <$inner>::played_seconds($get(&*self), mode)
            }
            fn call_type(&mut self) -> impl Future<Output = Result<CallType, TgCallsError>> {
                <$inner>::call_type($get(&*self))
            }
            fn connection_mode(
                &mut self,
            ) -> impl Future<Output = Result<ConnectionMode, TgCallsError>> {
                <$inner>::connection_mode($get(&*self))
            }
            fn cpu_usage(&mut self) -> impl Future<Output = Result<f64, TgCallsError>> {
                <$inner>::cpu_usage($get(&*self))
            }
        }
    };
}

fn same<T>(t: &T) -> &T {
    t
}

fn inside_arc<T>(t: &Arc<T>) -> &T {
    t
}

shared_ref_controls!(ConferenceCall, ConferenceCall, same);
shared_ref_controls!(P2PCall, P2PCall, same);
shared_ref_controls!(Arc<ConferenceCall>, ConferenceCall, inside_arc);
shared_ref_controls!(Arc<P2PCall>, P2PCall, inside_arc);
