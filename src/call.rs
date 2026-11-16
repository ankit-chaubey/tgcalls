use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferogram::tl;
use ntgcalls::{
    AudioDescription, CallType, ConnectionInfo, ConnectionMode, ConnectionState, FrameData,
    MediaDescription, MediaSegmentPartStatus, MediaState, NTgCalls, SsrcGroup, StreamDevice,
    StreamMode, StreamType, VideoDescription,
};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::{error::TgCallsError, signaling, signaling::MediaStateSync};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Idle,
    Joining,
    Joined,
    Leaving,
}

/// Filled by `join()` right before `connect()`, drained by the
/// `on_connection_change` callback once the state leaves `Connecting`.
type ConnectSlot = Arc<Mutex<Option<oneshot::Sender<Result<(), ntgcalls::Error>>>>>;

const JOIN_MAX_RETRIES: u32 = 3;
const JOIN_RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantAction {
    Joined,
    Left,
    Updated,
}

/// Everything a client might want to react to about this call, beyond the
/// return values of `Call`'s own methods.
#[derive(Debug, Clone)]
pub enum CallEvent {
    /// A stream (or a recording target) reached EOF.
    StreamEnded(StreamType, StreamDevice),
    /// A participant joined, left, or changed state (mute, video, etc).
    ParticipantUpdate {
        user_id: i64,
        action: ParticipantAction,
        is_self: bool,
    },
    /// You were removed from (or otherwise left) the call without calling
    /// `leave()` yourself. `Call`'s state is already reset to `Idle` by
    /// the time this fires.
    Left,
    /// The voice chat was ended for everyone.
    Ended,
    /// Your own mute / pause / camera / screen state changed, whether from
    /// a method on this `Call` or from the stream itself. Already pushed to
    /// Telegram by the time this fires.
    MediaStateChanged(MediaState),
}

type EventHandler = Arc<dyn Fn(CallEvent) + Send + Sync>;

fn participant_action(p: &tl::types::GroupCallParticipant) -> ParticipantAction {
    if p.just_joined {
        ParticipantAction::Joined
    } else if p.left {
        ParticipantAction::Left
    } else {
        ParticipantAction::Updated
    }
}

#[derive(Default, Clone)]
struct VideoSubs {
    camera_endpoint: Option<String>,
    presentation_endpoint: Option<String>,
}

#[derive(Clone, Copy)]
enum VideoKind {
    Camera,
    Presentation,
}

pub struct Call {
    client: ferogram::Client,
    chat_id: i64,
    ntg: NTgCalls,
    connect_slot: ConnectSlot,
    state: CallState,
    active_call: Option<ferogram::tl::enums::InputGroupCall>,
    presentation_active: bool,
    /// What is currently being captured. ntgcalls replaces the whole
    /// capture set on every `set_sources`, so single-source changes
    /// (`set_video`, `set_audio`, `set_screen`) start from this.
    media: MediaDescription,
    /// Sends our mute / camera / screen state to Telegram. Fresh on every
    /// join, so a rejoin always announces its real state.
    sync: MediaStateSync,
    video_subs: Arc<Mutex<HashMap<i64, VideoSubs>>>,
    event_handler: Option<EventHandler>,
}

pub(crate) fn no_media() -> MediaDescription {
    MediaDescription {
        microphone: None,
        speaker: None,
        camera: None,
        screen: None,
    }
}

/// Whether a join() failure looks transient and worth retrying: flood
/// waits, timeouts, and Telegram's internal (-500) errors. Anything else
/// (bad chat, no active call, auth issues) fails fast instead.
fn is_transient(err: &TgCallsError) -> bool {
    if !matches!(err, TgCallsError::Ferogram(_) | TgCallsError::NtgCalls(_)) {
        return false;
    }
    let msg = err.to_string().to_uppercase();
    msg.contains("FLOOD_WAIT")
        || msg.contains("TIMEOUT")
        || msg.contains("-500")
        || msg.contains("INTERNAL")
        || msg.contains("CONNECTION")
}

pub(crate) fn peer_user_id(peer: &tl::enums::Peer) -> Option<i64> {
    match peer {
        tl::enums::Peer::User(u) => Some(u.user_id),
        _ => None,
    }
}

/// Pulls `(endpoint, ssrc_groups)` out of a participant's camera or
/// presentation field, or `None` if absent/paused.
fn extract_video(
    v: &Option<tl::enums::GroupCallParticipantVideo>,
) -> Option<(String, Vec<SsrcGroup>)> {
    let tl::enums::GroupCallParticipantVideo::GroupCallParticipantVideo(v) = v.as_ref()?;
    if v.paused {
        return None;
    }
    Some((v.endpoint.clone(), to_ssrc_groups(&v.source_groups)))
}

fn to_ssrc_groups(groups: &[tl::enums::GroupCallParticipantVideoSourceGroup]) -> Vec<SsrcGroup> {
    groups
        .iter()
        .map(|g| {
            let tl::enums::GroupCallParticipantVideoSourceGroup::GroupCallParticipantVideoSourceGroup(g) = g;
            SsrcGroup {
                semantics: g.semantics.clone(),
                ssrcs: g.sources.iter().map(|&s| s as u32).collect(),
            }
        })
        .collect()
}

// Telegram supergroup IDs come in as -100xxxxxxxxxx; strip the -100 prefix
// to get the raw channel_id that ntgcalls expects.
pub(crate) fn ntg_chat_id(chat_id: i64) -> i64 {
    if chat_id < 0 {
        let abs = -chat_id;
        if abs > 1_000_000_000_000 {
            abs - 1_000_000_000_000
        } else {
            abs
        }
    } else {
        chat_id
    }
}

impl Call {
    pub fn new(client: ferogram::Client, chat_id: i64) -> Self {
        let mut ntg = NTgCalls::new();
        let connect_slot: ConnectSlot = Arc::new(Mutex::new(None));

        {
            let connect_slot = connect_slot.clone();
            ntg.on_connection_change(move |_chat_id, info: ConnectionInfo| {
                let mut slot = connect_slot.lock().unwrap();
                match info.state {
                    ConnectionState::Connected => {
                        debug!(target: "tgcalls", "connection established");
                        if let Some(tx) = slot.take() {
                            let _ = tx.send(Ok(()));
                        }
                    }
                    ConnectionState::Failed
                    | ConnectionState::Timeout
                    | ConnectionState::Closed => {
                        warn!(target: "tgcalls", "connection ended: {:?}", info.state);
                        if let Some(tx) = slot.take() {
                            let _ = tx.send(Err(ntgcalls::Error {
                                kind: ntgcalls::ErrorKind::Connection,
                                message: format!("connection ended: {:?}", info.state),
                            }));
                        }
                    }
                    ConnectionState::Connecting => {
                        debug!(target: "tgcalls", "connecting...");
                    }
                }
            });
        }

        Self {
            client,
            chat_id,
            ntg,
            connect_slot,
            state: CallState::Idle,
            active_call: None,
            presentation_active: false,
            media: no_media(),
            sync: MediaStateSync::default(),
            video_subs: Arc::new(Mutex::new(HashMap::new())),
            event_handler: None,
        }
    }

    /// Registers a handler for everything in [`CallEvent`] - stream end,
    /// participant changes, unexpected removal, the call ending. Call
    /// before `join()`.
    ///
    /// Must be called from inside a Tokio runtime (it is - every caller
    /// runs from inside `rt.block_on` or `#[tokio::main]`). ntgcalls fires
    /// `on_stream_end` (and every other `on_*` callback) synchronously on
    /// its own native WebRTC thread, never on a Tokio thread, so we can't
    /// just call `handler` in place: if `handler` does anything that needs
    /// a runtime (`tokio::spawn`, `.await`, a timer, ...) it aborts with
    /// "there is no reactor running". We capture this thread's `Handle`
    /// now and `spawn` onto it from the callback instead, so `handler`
    /// always runs as a proper task on the runtime that owns this `Call`.
    pub fn on_event(&mut self, handler: impl Fn(CallEvent) + Send + Sync + 'static) {
        let handler: EventHandler = Arc::new(handler);
        let stream_end_handler = handler.clone();
        let rt_handle = tokio::runtime::Handle::current();
        self.ntg
            .on_stream_end(move |_chat_id, stream_type, device| {
                let stream_end_handler = stream_end_handler.clone();
                let event = CallEvent::StreamEnded(stream_type, device);
                // Hop back onto the Tokio runtime before running user code.
                rt_handle.spawn(async move {
                    stream_end_handler(event);
                });
            });
        self.event_handler = Some(handler);
    }

    /// Registers a callback for raw decoded frames: `Capture` is your own
    /// outgoing audio / video as it is about to be sent, `Playback` is
    /// someone else's after decoding. Opt-in because ntgcalls only
    /// delivers frames once a callback is set. Fires on ntgcalls' native
    /// thread at media rate, so keep it cheap and hand the frames to a
    /// channel. Call before `join()`. Same shape as
    /// [`crate::P2PCall::on_frames`].
    pub fn on_frames(
        &mut self,
        callback: impl Fn(i64, StreamMode, StreamDevice, Vec<ntgcalls::Frame>) + Send + Sync + 'static,
    ) {
        self.ntg.on_frames(callback);
    }

    /// Registers a callback for when a remote participant's video source
    /// appears or disappears. See the note on [`Call::on_frames`].
    pub fn on_remote_source_change(
        &mut self,
        callback: impl Fn(i64, ntgcalls::RemoteSource) + Send + Sync + 'static,
    ) {
        self.ntg.on_remote_source_change(callback);
    }

    fn emit(&self, event: CallEvent) {
        if let Some(handler) = &self.event_handler {
            handler(event);
        }
    }

    pub async fn join(&mut self, media: MediaDescription) -> Result<(), TgCallsError> {
        self.join_inner(media, false).await
    }

    /// Like `join`, but starts a new voice chat first if none is active
    /// yet, instead of failing with `NoActiveGroupCall`.
    pub async fn create_and_join(&mut self, media: MediaDescription) -> Result<(), TgCallsError> {
        self.join_inner(media, true).await
    }

    async fn join_inner(
        &mut self,
        media: MediaDescription,
        auto_start: bool,
    ) -> Result<(), TgCallsError> {
        if self.state != CallState::Idle {
            return Err(TgCallsError::AlreadyJoined);
        }

        let mut attempt = 0;
        loop {
            match self.try_join(media.clone(), auto_start).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt < JOIN_MAX_RETRIES && is_transient(&e) => {
                    attempt += 1;
                    let delay = JOIN_RETRY_BASE_DELAY * 2u32.pow(attempt - 1);
                    warn!(
                        "tgcalls: join attempt {} failed ({}), retrying in {:?}",
                        attempt, e, delay
                    );
                    self.state = CallState::Idle;
                    self.active_call = None;
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    self.state = CallState::Idle;
                    return Err(e);
                }
            }
        }
    }

    async fn try_join(
        &mut self,
        media: MediaDescription,
        auto_start: bool,
    ) -> Result<(), TgCallsError> {
        self.state = CallState::Joining;

        let call = if auto_start {
            signaling::resolve_or_create_call(&self.client, self.chat_id).await?
        } else {
            signaling::resolve_call(&self.client, self.chat_id).await?
        };
        let ntg_id = ntg_chat_id(self.chat_id);

        let params_json = self.ntg.create_call(ntg_id).await?;
        debug!(
            "tgcalls: create params ({} bytes): {}",
            params_json.len(),
            params_json
        );

        let (tx, rx) = oneshot::channel();
        *self.connect_slot.lock().unwrap() = Some(tx);

        let transport_json = signaling::join_call(&self.client, call.clone(), &params_json).await?;
        debug!(
            "tgcalls: transport received ({} bytes): {}",
            transport_json.len(),
            transport_json
        );

        self.ntg.connect(ntg_id, &transport_json, false).await?;

        // connect() only dispatches the SDP; Connected arrives via the callback.
        rx.await.expect("connection callback never fired")?;

        self.sync = MediaStateSync::default();
        self.start_state_sync(call.clone());
        self.ntg
            .set_stream_sources(ntg_id, StreamMode::Capture, &media)
            .await?;
        self.media = media;

        // join_call() announces "unmuted, video on". Send the real state
        // once so an audio-only join doesn't show up as a video participant.
        if let Err(e) = self
            .sync
            .refresh(&self.client, call.clone(), &self.ntg, ntg_id)
            .await
        {
            warn!("tgcalls: initial media state sync failed: {}", e);
        }

        self.active_call = Some(call);
        self.state = CallState::Joined;
        info!("tgcalls: joined chat {}", self.chat_id);
        debug!("tgcalls: stream sources configured");
        Ok(())
    }

    pub async fn leave(&mut self) -> Result<(), TgCallsError> {
        if self.state != CallState::Joined {
            return Err(TgCallsError::NotJoined);
        }
        self.state = CallState::Leaving;

        if self.presentation_active {
            if let Err(e) = self.stop_presentation().await {
                warn!("tgcalls: stop_presentation error (ignored): {}", e);
            }
        }

        if let Err(e) = self.ntg.stop(ntg_chat_id(self.chat_id)).await {
            warn!("tgcalls: ntg stop error (ignored): {}", e);
        }

        let leave_result = match self.active_call.take() {
            Some(call) => signaling::leave_call(&self.client, call, 0).await,
            None => Ok(()),
        };

        // Reset unconditionally, even on a signaling failure: the media
        // stream is already stopped above either way, and getting stuck in
        // `Leaving` forever - unable to join() or leave() again - would be
        // strictly worse than surfacing this error while still recovering.
        self.video_subs.lock().unwrap().clear();
        self.media = no_media();
        self.state = CallState::Idle;

        if let Err(e) = leave_result {
            warn!(
                "tgcalls: leaveGroupCall signaling failed (state still reset, safe to retry): {}",
                e
            );
            return Err(e);
        }

        info!("tgcalls: left chat {}", self.chat_id);
        Ok(())
    }

    async fn join_presentation(&mut self, screen: VideoDescription) -> Result<(), TgCallsError> {
        self.require_joined()?;
        if self.presentation_active {
            return Err(TgCallsError::AlreadyJoined);
        }
        let ntg_id = ntg_chat_id(self.chat_id);
        let params_json = self.ntg.init_presentation(ntg_id).await?;
        debug!("tgcalls: presentation params ({} bytes)", params_json.len());

        let call = self
            .active_call
            .as_ref()
            .ok_or(TgCallsError::NotJoined)?
            .clone();
        let transport_json = signaling::join_presentation(&self.client, call, &params_json).await?;
        debug!(
            "tgcalls: presentation transport received ({} bytes)",
            transport_json.len()
        );

        self.ntg.connect(ntg_id, &transport_json, true).await?;

        // Keep whatever is already streaming (mic, camera): ntgcalls drops
        // every device missing from the description.
        let mut desc = self.media.clone();
        desc.screen = Some(screen);
        if let Err(e) = self
            .ntg
            .set_stream_sources(ntg_id, StreamMode::Capture, &desc)
            .await
        {
            let _ = self.ntg.stop_presentation(ntg_id).await;
            if let Some(call) = self.active_call.clone() {
                let _ = signaling::leave_presentation(&self.client, call).await;
            }
            return Err(e.into());
        }
        self.media = desc;

        self.presentation_active = true;
        if let Err(e) = self.push_media_state().await {
            warn!(
                "tgcalls: media state sync after presentation start failed: {}",
                e
            );
        }
        info!("tgcalls: presentation started in chat {}", self.chat_id);
        Ok(())
    }

    async fn stop_presentation(&mut self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        if !self.presentation_active {
            return Ok(());
        }
        let ntg_id = ntg_chat_id(self.chat_id);
        if let Err(e) = self.ntg.stop_presentation(ntg_id).await {
            warn!("tgcalls: ntg stop_presentation error (ignored): {}", e);
        }
        let call = self
            .active_call
            .as_ref()
            .ok_or(TgCallsError::NotJoined)?
            .clone();
        let leave_result = signaling::leave_presentation(&self.client, call).await;

        // Same reasoning as leave(): the ntg-level stop already happened
        // above, so don't leave presentation_active stuck true on a
        // signaling failure - that would incorrectly block a future
        // join_presentation() until this is retried.
        self.presentation_active = false;
        self.media.screen = None;
        if let Err(e) = self.push_media_state().await {
            warn!(
                "tgcalls: media state sync after presentation stop failed: {}",
                e
            );
        }

        leave_result?;
        info!("tgcalls: presentation stopped in chat {}", self.chat_id);
        Ok(())
    }

    pub(crate) fn is_presentation_active(&self) -> bool {
        self.presentation_active
    }

    /// `user_id` is the participant whose video you want to subscribe to.
    pub async fn add_incoming_video(
        &mut self,
        user_id: i64,
        endpoint: &str,
        ssrc_groups: &[SsrcGroup],
    ) -> Result<u32, TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .add_incoming_video(ntg_chat_id(self.chat_id), user_id, endpoint, ssrc_groups)
            .await?)
    }

    pub async fn remove_incoming_video(&mut self, endpoint: &str) -> Result<bool, TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .remove_incoming_video(ntg_chat_id(self.chat_id), endpoint)
            .await?)
    }

    pub async fn send_external_frame(
        &mut self,
        device: StreamDevice,
        frame: &[u8],
        frame_data: &FrameData,
    ) -> Result<(), TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .send_external_frame(ntg_chat_id(self.chat_id), device, frame, frame_data)
            .await?)
    }

    pub async fn send_broadcast_timestamp(&mut self, timestamp: i64) -> Result<(), TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .send_broadcast_timestamp(ntg_chat_id(self.chat_id), timestamp)
            .await?)
    }

    pub async fn send_broadcast_part(
        &mut self,
        segment_id: i64,
        part_id: i32,
        status: MediaSegmentPartStatus,
        quality_update: bool,
        frame: &[u8],
    ) -> Result<(), TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .send_broadcast_part(
                ntg_chat_id(self.chat_id),
                segment_id,
                part_id,
                status,
                quality_update,
                Some(frame),
            )
            .await?)
    }

    /// Replaces everything being captured (`StreamMode::Capture`: mic,
    /// camera, screen) or played back (`StreamMode::Playback`: where
    /// incoming audio / video goes, e.g. a recording target) in one call.
    /// Anything missing from `media` is removed. For changing one source
    /// and keeping the rest, use [`Call::set_audio`], [`Call::set_video`]
    /// or [`Call::set_screen`].
    pub async fn set_sources(
        &mut self,
        stream_mode: StreamMode,
        media: &MediaDescription,
    ) -> Result<(), TgCallsError> {
        self.require_joined()?;
        self.ntg
            .set_stream_sources(ntg_chat_id(self.chat_id), stream_mode, media)
            .await?;
        if stream_mode == StreamMode::Capture {
            self.media = media.clone();
            // Dropping the screen source ends the share, so close the
            // presentation connection too instead of leaving it dangling.
            if self.presentation_active && media.screen.is_none() {
                self.stop_presentation().await?;
            }
            self.push_media_state().await?;
        }
        Ok(())
    }

    /// Turns the camera on (`Some`), swaps it, or turns it off (`None`)
    /// without touching the mic or screen. Works at any time while joined.
    pub async fn set_video(
        &mut self,
        camera: Option<VideoDescription>,
    ) -> Result<(), TgCallsError> {
        let mut media = self.media.clone();
        media.camera = camera;
        self.set_sources(StreamMode::Capture, &media).await
    }

    /// Sets, swaps or removes the outgoing audio source. To silence the
    /// mic while keeping the source, use [`Call::mute`] instead.
    pub async fn set_audio(
        &mut self,
        microphone: Option<AudioDescription>,
    ) -> Result<(), TgCallsError> {
        let mut media = self.media.clone();
        media.microphone = microphone;
        self.set_sources(StreamMode::Capture, &media).await
    }

    /// Starts screen sharing (`Some`), swaps the shared source, or stops
    /// sharing (`None`). Handles the separate presentation connection.
    pub async fn set_screen(
        &mut self,
        screen: Option<VideoDescription>,
    ) -> Result<(), TgCallsError> {
        match screen {
            Some(s) if self.presentation_active => {
                let mut media = self.media.clone();
                media.screen = Some(s);
                self.set_sources(StreamMode::Capture, &media).await
            }
            Some(s) => self.join_presentation(s).await,
            None => {
                self.stop_presentation().await?;
                if self.media.screen.is_some() {
                    let mut media = self.media.clone();
                    media.screen = None;
                    self.set_sources(StreamMode::Capture, &media).await?;
                }
                Ok(())
            }
        }
    }

    /// What is currently being captured (mic, camera, screen).
    pub fn current_media(&self) -> &MediaDescription {
        &self.media
    }

    /// Forwards ntgcalls' state changes to Telegram, in order, and to the
    /// event handler. ntgcalls calls back on its native thread, so the
    /// callback only queues; a task on this runtime does the RPC.
    fn start_state_sync(&mut self, call: tl::enums::InputGroupCall) {
        let (tx, mut rx) = mpsc::unbounded_channel::<MediaState>();
        self.ntg.on_upgrade(move |_chat_id, state| {
            let _ = tx.send(state);
        });
        let client = self.client.clone();
        let handler = self.event_handler.clone();
        let sync = self.sync.clone();
        tokio::spawn(async move {
            while let Some(state) = rx.recv().await {
                if let Some(h) = &handler {
                    h(CallEvent::MediaStateChanged(state.clone()));
                }
                if let Err(e) = sync.push(&client, call.clone(), &state).await {
                    warn!("tgcalls: media state sync failed: {}", e);
                }
            }
        });
    }

    /// Tells Telegram the state ntgcalls has right now. Every control calls
    /// this directly, because ntgcalls' own state callback does not fire
    /// for every change. A push that changes nothing is skipped.
    async fn push_media_state(&self) -> Result<(), TgCallsError> {
        let call = self.active_call.clone().ok_or(TgCallsError::NotJoined)?;
        self.sync
            .refresh(&self.client, call, &self.ntg, ntg_chat_id(self.chat_id))
            .await
    }

    pub async fn cpu_usage(&mut self) -> Result<f64, TgCallsError> {
        Ok(self.ntg.cpu_usage().await?)
    }

    pub async fn call_type(&mut self) -> Result<CallType, TgCallsError> {
        self.require_joined()?;
        Ok(self.ntg.get_call_type(ntg_chat_id(self.chat_id)).await?)
    }

    pub async fn connection_mode(&mut self) -> Result<ConnectionMode, TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .get_connection_mode(ntg_chat_id(self.chat_id))
            .await?)
    }

    /// Pauses the outgoing stream (audio and video). Other participants
    /// see your video as paused.
    pub async fn pause(&mut self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        self.ntg.pause(ntg_chat_id(self.chat_id)).await?;
        self.push_media_state().await
    }

    pub async fn resume(&mut self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        self.ntg.resume(ntg_chat_id(self.chat_id)).await?;
        self.push_media_state().await
    }

    /// Seconds of the given stream played so far.
    pub async fn played_seconds(&mut self, stream_mode: StreamMode) -> Result<u64, TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .time(ntg_chat_id(self.chat_id), stream_mode)
            .await?)
    }

    /// Silences your mic without leaving or changing the source.
    pub async fn mute(&mut self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        self.ntg.mute(ntg_chat_id(self.chat_id)).await?;
        self.push_media_state().await
    }

    pub async fn unmute(&mut self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        self.ntg.unmute(ntg_chat_id(self.chat_id)).await?;
        self.push_media_state().await
    }

    /// Sets how loud `user_id` sounds to *you* in this call. 0 = muted for
    /// you, 10000 = 100% (default), up to 20000 = 200%. This is a
    /// per-listener preference, not a broadcast-wide control.
    pub async fn set_volume(&self, user_id: i64, volume: i32) -> Result<(), TgCallsError> {
        self.require_joined()?;
        let call = self
            .active_call
            .as_ref()
            .ok_or(TgCallsError::NotJoined)?
            .clone();
        let access_hash = signaling::get_user_access_hash(&self.client, user_id).await?;
        signaling::set_volume(&self.client, call, user_id, access_hash, volume).await
    }

    /// Fetches every current participant in the group call (paginated
    /// internally).
    pub async fn get_participants(
        &self,
    ) -> Result<Vec<tl::types::GroupCallParticipant>, TgCallsError> {
        self.require_joined()?;
        let call = self
            .active_call
            .as_ref()
            .ok_or(TgCallsError::NotJoined)?
            .clone();
        signaling::get_participants(&self.client, call).await
    }

    /// The voice chat was discarded for everyone - `discarded_id` is the
    /// group call's raw ID from the `GroupCallDiscarded` update. No-op if
    /// it isn't this call's ID.
    pub fn handle_call_ended(&mut self, discarded_id: i64) {
        if self.group_call_id() != Some(discarded_id) {
            return;
        }
        self.emit(CallEvent::Ended);
        self.state = CallState::Idle;
        self.active_call = None;
    }

    /// Auto subscribes/unsubscribes incoming video as participants' camera
    /// or screen-share sources change. [`crate::Calls`] wires this for you;
    /// call it directly only if you're managing `Call` yourself.
    pub async fn handle_participants_update(
        &mut self,
        participants: &[tl::enums::GroupCallParticipant],
    ) -> Result<(), TgCallsError> {
        if self.state != CallState::Joined {
            return Ok(());
        }

        for p in participants {
            let tl::enums::GroupCallParticipant::GroupCallParticipant(p) = p;
            let Some(user_id) = peer_user_id(&p.peer) else {
                continue;
            };

            self.emit(CallEvent::ParticipantUpdate {
                user_id,
                action: participant_action(p),
                is_self: p.is_self,
            });

            if p.is_self {
                if p.left {
                    self.emit(CallEvent::Left);
                    self.state = CallState::Idle;
                    self.active_call = None;
                    return Ok(());
                }
                continue;
            }

            let camera = if p.left {
                None
            } else {
                extract_video(&p.video)
            };
            let presentation = if p.left {
                None
            } else {
                extract_video(&p.presentation)
            };

            self.sync_video_source(user_id, VideoKind::Camera, camera)
                .await;
            self.sync_video_source(user_id, VideoKind::Presentation, presentation)
                .await;
        }
        Ok(())
    }

    /// Adds/removes an incoming video subscription for one participant's
    /// camera or presentation source, only when the endpoint actually
    /// changed. Never holds the tracking lock across an await.
    async fn sync_video_source(
        &mut self,
        user_id: i64,
        kind: VideoKind,
        wanted: Option<(String, Vec<SsrcGroup>)>,
    ) {
        let current = {
            let subs = self.video_subs.lock().unwrap();
            subs.get(&user_id).and_then(|e| match kind {
                VideoKind::Camera => e.camera_endpoint.clone(),
                VideoKind::Presentation => e.presentation_endpoint.clone(),
            })
        };
        let wanted_endpoint = wanted.as_ref().map(|(e, _)| e.clone());
        if current == wanted_endpoint {
            return;
        }

        if let Some(old) = current {
            let _ = self.remove_incoming_video(&old).await;
        }

        match wanted {
            Some((endpoint, groups)) => {
                if self
                    .add_incoming_video(user_id, &endpoint, &groups)
                    .await
                    .is_err()
                {
                    return;
                }
                let mut subs = self.video_subs.lock().unwrap();
                let entry = subs.entry(user_id).or_default();
                match kind {
                    VideoKind::Camera => entry.camera_endpoint = Some(endpoint),
                    VideoKind::Presentation => entry.presentation_endpoint = Some(endpoint),
                }
            }
            None => {
                let mut subs = self.video_subs.lock().unwrap();
                if let Some(entry) = subs.get_mut(&user_id) {
                    match kind {
                        VideoKind::Camera => entry.camera_endpoint = None,
                        VideoKind::Presentation => entry.presentation_endpoint = None,
                    }
                    if entry.camera_endpoint.is_none() && entry.presentation_endpoint.is_none() {
                        subs.remove(&user_id);
                    }
                }
            }
        }
    }

    pub fn state(&self) -> CallState {
        self.state
    }

    /// The active group call's raw ID, used by [`crate::Calls`] to route
    /// `GroupCallParticipants` updates back to the right chat. `None` if
    /// not currently joined.
    pub fn group_call_id(&self) -> Option<i64> {
        match self.active_call.as_ref()? {
            tl::enums::InputGroupCall::InputGroupCall(c) => Some(c.id),
            _ => None,
        }
    }

    pub fn is_joined(&self) -> bool {
        self.state == CallState::Joined
    }

    pub async fn media_state(&mut self) -> Result<ntgcalls::MediaState, TgCallsError> {
        self.require_joined()?;
        Ok(self.ntg.get_state(ntg_chat_id(self.chat_id)).await?)
    }

    fn require_joined(&self) -> Result<(), TgCallsError> {
        if self.state != CallState::Joined {
            return Err(TgCallsError::NotJoined);
        }
        Ok(())
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        if self.state != CallState::Joined {
            return;
        }
        warn!("tgcalls: Call dropped while still joined - stopping stream");

        // Drop can't await; ntg.stop() must run via block_on since NTgCalls
        // isn't Sync and can't cross Handle::spawn's Send bound.
        let ntg = std::mem::take(&mut self.ntg);
        let chat_id = ntg_chat_id(self.chat_id);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let handle2 = handle.clone();
            handle.spawn_blocking(move || {
                handle2.block_on(async move {
                    let _ = ntg.stop(chat_id).await;
                });
            });
        }
    }
}
