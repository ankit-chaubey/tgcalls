//! One set of controls for every kind of call.
//!
//! [`Call`](crate::Call), [`P2PCall`](crate::P2PCall) and
//! [`ConferenceCall`](crate::ConferenceCall) all have the same mid-call
//! controls under the same names, but not the same receivers (`Call` takes
//! `&mut self`, the others `&self`). [`CallControls`] puts them behind one
//! trait, so code that only needs to mute, switch the camera or share the
//! screen works with any of them:
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
    AudioDescription, FrameData, MediaDescription, MediaState, StreamDevice, VideoDescription,
};

use crate::error::TgCallsError;
use crate::{Call, ConferenceCall, P2PCall};

/// Mid-call controls shared by every call type. Every method works at any
/// time while the call is up, in any order, and changing one source keeps
/// the others (turning the camera on never drops the mic).
pub trait CallControls {
    /// Silences the mic and keeps its source.
    fn mute(&mut self) -> impl Future<Output = Result<(), TgCallsError>>;

    fn unmute(&mut self) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Pauses the outgoing stream (audio and video).
    fn pause(&mut self) -> impl Future<Output = Result<(), TgCallsError>>;

    fn resume(&mut self) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Camera on (`Some`), swapped, or off (`None`).
    fn set_camera(
        &mut self,
        camera: Option<VideoDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Sets, swaps or removes the outgoing audio source. To silence the mic
    /// while keeping the source, use [`CallControls::mute`].
    fn set_microphone(
        &mut self,
        microphone: Option<AudioDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Screen share on (`Some`), swapped, or off (`None`).
    fn set_screen(
        &mut self,
        screen: Option<VideoDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>>;

    /// Current mute / pause / camera / screen flags.
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
    fn set_camera(
        &mut self,
        camera: Option<VideoDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::set_camera(self, camera)
    }
    fn set_microphone(
        &mut self,
        microphone: Option<AudioDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::set_microphone(self, microphone)
    }
    fn set_screen(
        &mut self,
        screen: Option<VideoDescription>,
    ) -> impl Future<Output = Result<(), TgCallsError>> {
        Call::set_screen(self, screen)
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
            fn set_camera(
                &mut self,
                camera: Option<VideoDescription>,
            ) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::set_camera($get(&*self), camera)
            }
            fn set_microphone(
                &mut self,
                microphone: Option<AudioDescription>,
            ) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::set_microphone($get(&*self), microphone)
            }
            fn set_screen(
                &mut self,
                screen: Option<VideoDescription>,
            ) -> impl Future<Output = Result<(), TgCallsError>> {
                <$inner>::set_screen($get(&*self), screen)
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
