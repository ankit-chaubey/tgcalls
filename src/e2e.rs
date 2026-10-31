//! Telegram's E2E-encrypted "conference call" - a genuinely different
//! join/signaling flow from the classic group call in [`crate::Call`].
//! Membership and the shared encryption key live in a signed, append-only
//! block chain that clients build and verify themselves
//! (`ntgcalls::e2e::Session` on the native side); the server just relays
//! opaque blocks between participants instead of broadcasting a
//! participant list.
//!
//! # Why this is its own actor, not just another `Call` method
//!
//! `ntgcalls::NTgCalls` is `Send` but deliberately not `Sync` (see the
//! comment at the top of `calls.rs`). That's fine for a plain stream-end
//! notification - forward it and move on. It's trickier here: answering
//! `on_subchain_request` means doing a `phone.*` RPC and then calling back
//! into `ntg.apply_blocks(...)` and `ntg.finish_subchain_request(...)`, and
//! answering `on_request_participants` means an RPC followed by
//! `ntg.update_audio_ssrc_mappings(...)`. A plain `tokio::spawn`ed task
//! can't hold a live `&NTgCalls` across an await, since `NTgCalls: !Sync`.
//!
//! So [`ConferenceCall`] runs its own dedicated thread with its own
//! current-thread runtime, the same way `calls.rs`'s `Worker` does. Every
//! raw ntgcalls callback (`on_outbound_block`, `on_subchain_request`,
//! `on_request_participants`, `on_update_emojis`, `on_stream_end`) just
//! pushes a [`Command`] onto that actor's channel - no awaiting, no ntg
//! calls, no user code, so it's safe to call from ntgcalls' native WebRTC
//! thread. The receive loop is the one place that owns `ntg` and has a
//! real Tokio context, so it's the only place doing the actual
//! RPC-then-ntg-call work, one step at a time.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ferogram::middleware::{BoxFuture, Middleware, Next};
use ferogram::tl;
use ferogram::Update;
use ntgcalls::{
    AudioDescription, CallType, ConnectionInfo, ConnectionMode, ConnectionState, Frame, FrameData,
    MediaDescription, MediaState, NTgCalls, RemoteSource, SsrcGroup, SsrcMapping, StreamDevice,
    StreamMode, StreamType, SubchainRequest, VideoDescription,
};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::call::{no_media, ntg_chat_id, peer_user_id};
use crate::{error::TgCallsError, signaling, signaling::MediaStateSync};

type Reply<T> = oneshot::Sender<Result<T, TgCallsError>>;
type ConnectSlot = Arc<Mutex<Option<oneshot::Sender<Result<(), ntgcalls::Error>>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConferenceState {
    Idle,
    Joining,
    Joined,
    Leaving,
}

/// Everything a client might want to react to about a [`ConferenceCall`].
#[derive(Debug, Clone)]
pub enum ConferenceEvent {
    /// A stream (or a recording target) reached EOF - same meaning as
    /// [`crate::CallEvent::StreamEnded`].
    StreamEnded(StreamType, StreamDevice),
    /// The verification emoji fingerprint changed. Show this to the user
    /// so they can compare it with other participants out of band -
    /// that comparison *is* the E2E guarantee, ntgcalls can't do it for
    /// you. Also available on demand via
    /// [`ConferenceCall::fingerprint_emojis`].
    FingerprintUpdated(String),
    /// You were removed from (or otherwise left) the conference.
    Left,
    /// The participant set changed (someone joined or left). There's no
    /// P2P-style "they picked up" signal in the conference/group-call
    /// model - everyone just streams into a shared room - so this is the
    /// closest practical equivalent. Fires from `on_request_participants`,
    /// which ntgcalls calls whenever it needs a fresh SSRC mapping, which
    /// in practice means the participant set just changed. If you want to
    /// hold off streaming media until someone else has actually joined,
    /// start with `media: None` and call [`ConferenceCall::set_sources`] the
    /// first time this fires. It also updates the camera / mute state
    /// Telegram shows for you, so this works for video too. If the other
    /// side still shows no video, start with a placeholder camera
    /// (`MediaDescription { camera: Some(Media::external_video(..)), .. }`)
    /// instead: that makes the join itself announce video, and `set_sources` then
    /// swaps in the real source.
    ParticipantsChanged,
    /// Raw decoded media frames. `mode` is `Capture` (your own outgoing
    /// audio/video, as it's about to be sent) or `Playback` (someone
    /// else's, after E2E decryption and decode). Each `Frame.ssrc`
    /// identifies which participant a `Playback` frame came from - fetch
    /// the current ssrc->user_id mapping yourself with
    /// `signaling::get_participants` (`GroupCallParticipant.source` is the
    /// ssrc, `.peer` is who it belongs to) if you need to attribute frames
    /// to people, this crate doesn't cache that mapping for you.
    ///
    /// This is the only way to capture/record a conference locally -
    /// conferences don't support Telegram's server-side recording
    /// (`phone.toggleGroupCallRecord` is a video-chat/livestream-only
    /// feature), since the server never has the keys to decrypt E2E media.
    /// "Saving" is entirely on you: `data` is raw decoded bytes (PCM audio
    /// / raw video per `frame_data`'s dimensions), not a file - write it
    /// yourself, pipe it into an encoder, whatever you need. Fires at
    /// media rate (tens of times a second per active source), so keep the
    /// handler cheap; it runs inside the actor's own Tokio context, so
    /// `tokio::spawn` a task for any real I/O rather than doing it inline.
    Frames {
        mode: StreamMode,
        device: StreamDevice,
        frames: Vec<Frame>,
    },
    /// A participant's media source appeared, changed state, or
    /// disappeared - `RemoteSource.ssrc` matches `Frame.ssrc` above.
    RemoteSourceChanged(RemoteSource),
    /// Your own mute / pause / camera / screen state changed. Pushed to
    /// Telegram on a best-effort basis (failures are logged, not fatal).
    MediaStateChanged(MediaState),
}

/// What to do when starting a [`ConferenceCall`].
pub enum ConferenceTarget {
    /// Start a brand new conference in this chat and ring everyone in
    /// `invite`. A failed invite (bad access hash, blocked you, whatever)
    /// is logged and skipped rather than failing the whole `start()` - the
    /// conference is still created and joinable even if nobody else picks
    /// up yet. Ring more people later with [`ConferenceCall::invite`].
    Create { invite: Vec<i64> },
    /// Join (or rejoin - e.g. after a P2P->conference migration) an
    /// existing conference already linked to this chat. `None` for a fresh
    /// join with no chain history yet; `Some(last_block)` to resume from
    /// where you left off (get one via
    /// `signaling::get_conference_chain_blocks` with
    /// `sub_chain_id: 0, offset: -1, limit: 1`).
    ///
    /// This resolves the conference via `chat_id` - correct when you're
    /// already in the chat the conference is linked to. If you only have a
    /// shared link or were invited by message, use [`Self::JoinBySlug`] /
    /// [`Self::JoinByInviteMessage`] instead; per Telegram's own docs those
    /// are the correct paths for those two cases, not chat_id resolution.
    Join { last_block: Option<Vec<u8>> },
    /// Join using a conference deep link (`t.me/call/<slug>` /
    /// `tg://call?slug=<slug>`) - parse one with
    /// [`crate::parse_conference_link`]. Doesn't require being
    /// a member of any particular chat; the slug alone identifies the
    /// conference.
    JoinBySlug {
        slug: String,
        last_block: Option<Vec<u8>>,
    },
    /// Join using the message ID of a `messageActionConferenceCall` you
    /// were invited with - see [`ConferenceInvite`] /
    /// [`incoming_conference_call`]. This is the correct way to join an
    /// explicit invite (`phone.inviteConferenceCallParticipant`), per
    /// Telegram's docs - not chat_id resolution.
    JoinByInviteMessage {
        msg_id: i32,
        last_block: Option<Vec<u8>>,
    },
}

/// Whether `media` actually carries a camera/screen source - this is what
/// `video`/`video_stopped` mean everywhere in this module, derived instead
/// of taken as a separate parameter. A hand-passed `video: bool` next to a
/// `media` argument can trivially disagree with what's actually being
/// streamed (pass audio-only media but say `video: true`, or vice versa) -
/// there's exactly one source of truth for whether video is present, which
/// is the `MediaDescription` itself.
fn media_has_video(media: &Option<MediaDescription>) -> bool {
    media
        .as_ref()
        .is_some_and(|m| m.camera.is_some() || m.screen.is_some())
}

/// Tells Telegram the media state ntgcalls has right now. Every control
/// calls this directly, because ntgcalls' own state callback does not fire
/// for every change. Failures are logged, not returned: the local change
/// already happened, and other participants also hear about it from
/// ntgcalls itself.
async fn push_media_state(
    sync: &MediaStateSync,
    client: &ferogram::Client,
    ntg: &NTgCalls,
    ntg_id: i64,
    call: &Option<tl::enums::InputGroupCall>,
) {
    let Some(call) = call else { return };
    if let Err(e) = sync.refresh(client, call.clone(), ntg, ntg_id).await {
        warn!("tgcalls: media state sync failed: {}", e);
    }
}

/// One mid-call change to your own media, run on the actor thread.
enum Control {
    Mute,
    Unmute,
    Pause,
    Resume,
    Video(Option<VideoDescription>),
    Audio(Option<AudioDescription>),
    Screen(Option<VideoDescription>),
}

enum Command {
    Control(Control, Reply<()>),
    ExternalFrame {
        device: StreamDevice,
        data: Vec<u8>,
        frame_data: FrameData,
        reply: Reply<()>,
    },
    SetVolume {
        user_id: i64,
        volume: i32,
        reply: Reply<()>,
    },
    Participants(Reply<Vec<tl::types::GroupCallParticipant>>),
    MediaState(Reply<MediaState>),
    StateChanged(MediaState),
    Start {
        target: ConferenceTarget,
        media: Option<MediaDescription>,
        reply: Reply<()>,
    },
    Leave(Reply<()>),
    FingerprintEmojis(Reply<String>),
    IsJoined(oneshot::Sender<bool>),
    /// The conference's invite link (`t.me/call/<slug>`), if the server
    /// sent one back when it was created. `None` if you joined an existing
    /// conference rather than creating it - joining doesn't hand back a
    /// link the way creating does.
    InviteLink(oneshot::Sender<Option<String>>),
    /// Ring another user into an already-running conference. See
    /// [`ConferenceTarget::Create`] for ringing people at creation time -
    /// this is for adding someone mid-call. `video: None` signals the
    /// invitee based on whatever's currently playing (`media_has_video`);
    /// `Some(v)` overrides that - e.g. to ring someone as a video call
    /// while you're only actually streaming audio, or the reverse.
    Invite {
        user_id: i64,
        video: Option<bool>,
        reply: Reply<()>,
    },
    /// Replace what you're capturing or playing back in an already-running
    /// conference. Lets you join with `media: None` and start sending
    /// later - e.g. once [`ConferenceEvent::ParticipantsChanged`] fires,
    /// so you're not sending to an empty room.
    SetSources {
        mode: StreamMode,
        media: MediaDescription,
        reply: Reply<()>,
    },
    IncomingVideo {
        user_id: i64,
        endpoint: String,
        ssrc_groups: Vec<SsrcGroup>,
        reply: Reply<u32>,
    },
    RemoveIncomingVideo {
        endpoint: String,
        reply: Reply<bool>,
    },
    PlayedSeconds {
        mode: StreamMode,
        reply: Reply<u64>,
    },
    CallType(Reply<CallType>),
    ConnectionMode(Reply<ConnectionMode>),
    CpuUsage(Reply<f64>),
    /// Feed in an `UpdateGroupCallChainBlocks` you received on the normal
    /// update stream (someone else's new block) - wire this the same way
    /// `Calls` wires `route_update` for classic group calls.
    ApplyBlocks {
        call: tl::enums::InputGroupCall,
        sub_chain_id: i32,
        blocks: Vec<Vec<u8>>,
        next_offset: i32,
    },
    // Fed in from the raw ntgcalls callbacks - see the module docs. Each
    // of these is handled inline in the loop below, which is the only
    // place that ever touches `ntg`.
    OutboundBlock(Vec<u8>),
    SubchainRequest(SubchainRequest),
    RequestParticipants,
    EmojisUpdated(String),
    StreamEnded(StreamType, StreamDevice),
    Frames {
        mode: StreamMode,
        device: StreamDevice,
        frames: Vec<Frame>,
    },
    RemoteSourceChanged(RemoteSource),
}

/// One chat's E2E conference call, running on its own dedicated thread.
/// See the module docs for why.
pub struct ConferenceCall {
    tx: mpsc::UnboundedSender<Command>,
    chat_id: i64,
    /// Mirror of what the actor is capturing, so `current_media` doesn't
    /// need a round trip to the actor thread.
    media: Arc<Mutex<MediaDescription>>,
}

impl ConferenceCall {
    /// Spawns the worker thread, registers every conference-related
    /// ntgcalls callback, and wires them straight back into this actor.
    /// `handler` receives [`ConferenceEvent`]s and is called from inside
    /// the actor's own Tokio context, so it's free to `tokio::spawn` or
    /// `.await` things itself.
    pub fn new(
        client: ferogram::Client,
        chat_id: i64,
        handler: impl Fn(ConferenceEvent) + Send + Sync + 'static,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Command>();
        let actor_tx = tx.clone();
        let media_view = Arc::new(Mutex::new(no_media()));
        let media_mirror = media_view.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tgcalls conference worker: failed to start runtime");

            rt.block_on(async move {
                let mut ntg = NTgCalls::new();
                let connect_slot: ConnectSlot = Arc::new(Mutex::new(None));
                let mut state = ConferenceState::Idle;
                let mut active_call: Option<tl::enums::InputGroupCall> = None;
                let mut has_video = false;
                let mut current = no_media();
                let mut sync = MediaStateSync::default();
                let mut invite_link: Option<String> = None;
                let ntg_id = ntg_chat_id(chat_id);

                {
                    let connect_slot = connect_slot.clone();
                    ntg.on_connection_change(move |_chat_id, info: ConnectionInfo| {
                        let mut slot = connect_slot.lock().unwrap();
                        match info.state {
                            ConnectionState::Connected => {
                                if let Some(tx) = slot.take() {
                                    let _ = tx.send(Ok(()));
                                }
                            }
                            ConnectionState::Failed
                            | ConnectionState::Timeout
                            | ConnectionState::Closed => {
                                if let Some(tx) = slot.take() {
                                    let _ = tx.send(Err(ntgcalls::Error {
                                        kind: ntgcalls::ErrorKind::Connection,
                                        message: format!("connection ended: {:?}", info.state),
                                    }));
                                }
                            }
                            ConnectionState::Connecting => {}
                        }
                    });
                }

                // Every callback here does exactly one thing: push a
                // `Command`. Nothing async, nothing touching `ntg` - safe
                // to fire from ntgcalls' native WebRTC thread.
                {
                    let tx = actor_tx.clone();
                    ntg.on_stream_end(move |_chat_id, stream_type, device| {
                        let _ = tx.send(Command::StreamEnded(stream_type, device));
                    });
                }
                {
                    let tx = actor_tx.clone();
                    ntg.on_frames(move |_chat_id, mode, device, frames| {
                        let _ = tx.send(Command::Frames {
                            mode,
                            device,
                            frames,
                        });
                    });
                }
                {
                    let tx = actor_tx.clone();
                    ntg.on_remote_source_change(move |_chat_id, source| {
                        let _ = tx.send(Command::RemoteSourceChanged(source));
                    });
                }
                {
                    let tx = actor_tx.clone();
                    ntg.on_upgrade(move |_chat_id, state| {
                        let _ = tx.send(Command::StateChanged(state));
                    });
                }
                {
                    let tx = actor_tx.clone();
                    ntg.on_outbound_block(move |_chat_id, block| {
                        let _ = tx.send(Command::OutboundBlock(block));
                    });
                }
                {
                    let tx = actor_tx.clone();
                    ntg.on_subchain_request(move |_chat_id, req| {
                        let _ = tx.send(Command::SubchainRequest(req));
                    });
                }
                {
                    let tx = actor_tx.clone();
                    ntg.on_request_participants(move |_chat_id, _req| {
                        let _ = tx.send(Command::RequestParticipants);
                    });
                }
                {
                    let tx = actor_tx.clone();
                    ntg.on_update_emojis(move |_chat_id, emoji| {
                        let _ = tx.send(Command::EmojisUpdated(emoji));
                    });
                }

                while let Some(cmd) = rx.recv().await {
                    match cmd {
                        Command::Start {
                            target,
                            media,
                            reply,
                        } => {
                            has_video = media_has_video(&media);
                            current = media.clone().unwrap_or_else(no_media);
                            *media_mirror.lock().unwrap() = current.clone();
                            sync = MediaStateSync::default();
                            let ctx = StartCtx {
                                client: &client,
                                ntg_id,
                                chat_id,
                                connect_slot: &connect_slot,
                            };
                            let result =
                                start_conference(&ctx, &mut ntg, target, media, !has_video).await;
                            match result {
                                Ok((call, link)) => {
                                    // join announces "unmuted, video on" regardless
                                    // of media; send the real state.
                                    active_call = Some(call);
                                    push_media_state(&sync, &client, &ntg, ntg_id, &active_call)
                                        .await;
                                    invite_link = link;
                                    state = ConferenceState::Joined;
                                    let _ = reply.send(Ok(()));
                                }
                                Err(e) => {
                                    state = ConferenceState::Idle;
                                    let _ = reply.send(Err(e));
                                }
                            }
                        }
                        Command::Leave(reply) => {
                            let result = ntg.stop(ntg_id).await.map_err(TgCallsError::from);
                            current = no_media();
                            *media_mirror.lock().unwrap() = current.clone();
                            active_call = None;
                            invite_link = None;
                            state = ConferenceState::Idle;
                            let _ = reply.send(result);
                        }
                        Command::FingerprintEmojis(reply) => {
                            let result = ntg
                                .get_emojis_fingerprint(ntg_id)
                                .await
                                .map_err(TgCallsError::from);
                            let _ = reply.send(result);
                        }
                        Command::IsJoined(reply) => {
                            let _ = reply.send(state == ConferenceState::Joined);
                        }
                        Command::InviteLink(reply) => {
                            let _ = reply.send(invite_link.clone());
                        }
                        Command::Invite {
                            user_id,
                            video,
                            reply,
                        } => {
                            let video = video.unwrap_or(has_video);
                            let result = match &active_call {
                                Some(call) => {
                                    invite_participant(&client, call, user_id, video).await
                                }
                                None => Err(TgCallsError::NotJoined),
                            };
                            let _ = reply.send(result);
                        }
                        Command::SetSources { mode, media, reply } => {
                            let result = if state != ConferenceState::Joined {
                                Err(TgCallsError::NotJoined)
                            } else {
                                let result = ntg
                                    .set_stream_sources(ntg_id, mode, &media)
                                    .await
                                    .map_err(TgCallsError::from);
                                // Only what you send changes the media state
                                // Telegram shows; playback targets don't.
                                if result.is_ok() && mode == StreamMode::Capture {
                                    has_video = media.camera.is_some() || media.screen.is_some();
                                    current = media;
                                    *media_mirror.lock().unwrap() = current.clone();
                                    push_media_state(&sync, &client, &ntg, ntg_id, &active_call)
                                        .await;
                                }
                                result
                            };
                            let _ = reply.send(result);
                        }
                        Command::IncomingVideo {
                            user_id,
                            endpoint,
                            ssrc_groups,
                            reply,
                        } => {
                            let result = if state != ConferenceState::Joined {
                                Err(TgCallsError::NotJoined)
                            } else {
                                ntg.add_incoming_video(ntg_id, user_id, &endpoint, &ssrc_groups)
                                    .await
                                    .map_err(TgCallsError::from)
                            };
                            let _ = reply.send(result);
                        }
                        Command::RemoveIncomingVideo { endpoint, reply } => {
                            let result = if state != ConferenceState::Joined {
                                Err(TgCallsError::NotJoined)
                            } else {
                                ntg.remove_incoming_video(ntg_id, &endpoint)
                                    .await
                                    .map_err(TgCallsError::from)
                            };
                            let _ = reply.send(result);
                        }
                        Command::PlayedSeconds { mode, reply } => {
                            let result = if state != ConferenceState::Joined {
                                Err(TgCallsError::NotJoined)
                            } else {
                                ntg.time(ntg_id, mode).await.map_err(TgCallsError::from)
                            };
                            let _ = reply.send(result);
                        }
                        Command::CallType(reply) => {
                            let result = if state != ConferenceState::Joined {
                                Err(TgCallsError::NotJoined)
                            } else {
                                ntg.get_call_type(ntg_id).await.map_err(TgCallsError::from)
                            };
                            let _ = reply.send(result);
                        }
                        Command::ConnectionMode(reply) => {
                            let result = if state != ConferenceState::Joined {
                                Err(TgCallsError::NotJoined)
                            } else {
                                ntg.get_connection_mode(ntg_id)
                                    .await
                                    .map_err(TgCallsError::from)
                            };
                            let _ = reply.send(result);
                        }
                        Command::CpuUsage(reply) => {
                            let _ = reply.send(ntg.cpu_usage().await.map_err(TgCallsError::from));
                        }
                        Command::Control(control, reply) => {
                            let result: Result<(), TgCallsError> = async {
                                if state != ConferenceState::Joined {
                                    return Err(TgCallsError::NotJoined);
                                }
                                match control {
                                    Control::Mute => {
                                        ntg.mute(ntg_id).await?;
                                    }
                                    Control::Unmute => {
                                        ntg.unmute(ntg_id).await?;
                                    }
                                    Control::Pause => {
                                        ntg.pause(ntg_id).await?;
                                    }
                                    Control::Resume => {
                                        ntg.resume(ntg_id).await?;
                                    }
                                    source => {
                                        let mut next = current.clone();
                                        match source {
                                            Control::Video(c) => next.camera = c,
                                            Control::Audio(m) => next.microphone = m,
                                            Control::Screen(sc) => next.screen = sc,
                                            _ => unreachable!("handled above"),
                                        }
                                        ntg.set_stream_sources(ntg_id, StreamMode::Capture, &next)
                                            .await?;
                                        has_video = next.camera.is_some() || next.screen.is_some();
                                        current = next;
                                        *media_mirror.lock().unwrap() = current.clone();
                                    }
                                }
                                push_media_state(&sync, &client, &ntg, ntg_id, &active_call).await;
                                Ok(())
                            }
                            .await;
                            let _ = reply.send(result);
                        }
                        Command::ExternalFrame {
                            device,
                            data,
                            frame_data,
                            reply,
                        } => {
                            let result = if state == ConferenceState::Joined {
                                ntg.send_external_frame(ntg_id, device, &data, &frame_data)
                                    .await
                                    .map_err(TgCallsError::from)
                            } else {
                                Err(TgCallsError::NotJoined)
                            };
                            let _ = reply.send(result);
                        }
                        Command::SetVolume {
                            user_id,
                            volume,
                            reply,
                        } => {
                            let result = match (&active_call, state == ConferenceState::Joined) {
                                (Some(call), true) => {
                                    async {
                                        let access_hash =
                                            signaling::get_user_access_hash(&client, user_id)
                                                .await?;
                                        signaling::set_volume(
                                            &client,
                                            call.clone(),
                                            user_id,
                                            access_hash,
                                            volume,
                                        )
                                        .await
                                    }
                                    .await
                                }
                                _ => Err(TgCallsError::NotJoined),
                            };
                            let _ = reply.send(result);
                        }
                        Command::Participants(reply) => {
                            let result = match (&active_call, state == ConferenceState::Joined) {
                                (Some(call), true) => {
                                    signaling::get_participants(&client, call.clone()).await
                                }
                                _ => Err(TgCallsError::NotJoined),
                            };
                            let _ = reply.send(result);
                        }
                        Command::MediaState(reply) => {
                            let result = if state == ConferenceState::Joined {
                                ntg.get_state(ntg_id).await.map_err(TgCallsError::from)
                            } else {
                                Err(TgCallsError::NotJoined)
                            };
                            let _ = reply.send(result);
                        }
                        Command::StateChanged(st) => {
                            handler(ConferenceEvent::MediaStateChanged(st.clone()));
                            if let Some(call) = &active_call {
                                if let Err(e) = sync.push(&client, call.clone(), &st).await {
                                    warn!("tgcalls: media state sync failed: {}", e);
                                }
                            }
                        }
                        Command::ApplyBlocks {
                            call,
                            sub_chain_id,
                            blocks,
                            next_offset,
                        } => {
                            let matches = match (&active_call, &call) {
                                (
                                    Some(tl::enums::InputGroupCall::InputGroupCall(a)),
                                    tl::enums::InputGroupCall::InputGroupCall(b),
                                ) => a.id == b.id,
                                _ => false,
                            };
                            if matches {
                                if let Err(e) = ntg
                                    .apply_blocks(ntg_id, sub_chain_id, next_offset, &blocks, false)
                                    .await
                                {
                                    warn!("tgcalls: apply_blocks (pushed) failed: {}", e);
                                }
                            }
                        }
                        Command::OutboundBlock(block) => {
                            if let Some(call) = &active_call {
                                if let Err(e) = signaling::send_conference_call_broadcast(
                                    &client,
                                    call.clone(),
                                    block,
                                )
                                .await
                                {
                                    warn!("tgcalls: failed to broadcast outbound block: {}", e);
                                }
                            }
                        }
                        Command::SubchainRequest(req) => {
                            if let Some(call) = &active_call {
                                match signaling::get_conference_chain_blocks(
                                    &client,
                                    call.clone(),
                                    req.subchain,
                                    req.height,
                                    req.limit,
                                )
                                .await
                                {
                                    Ok(Some(cb)) => {
                                        if let Err(e) = ntg
                                            .apply_blocks(
                                                ntg_id,
                                                cb.sub_chain_id,
                                                cb.next_offset,
                                                &cb.blocks,
                                                true,
                                            )
                                            .await
                                        {
                                            warn!(
                                                "tgcalls: apply_blocks (short poll) failed: {}",
                                                e
                                            );
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        warn!("tgcalls: getGroupCallChainBlocks failed: {}", e)
                                    }
                                }
                                if let Err(e) =
                                    ntg.finish_subchain_request(ntg_id, req.subchain).await
                                {
                                    warn!("tgcalls: finish_subchain_request failed: {}", e);
                                }
                            }
                        }
                        Command::RequestParticipants => {
                            if let Some(call) = &active_call {
                                match signaling::get_participants(&client, call.clone()).await {
                                    Ok(participants) => {
                                        let mappings: Vec<SsrcMapping> = participants
                                            .iter()
                                            .filter_map(|p| {
                                                peer_user_id(&p.peer).map(|user_id| SsrcMapping {
                                                    user_id,
                                                    ssrc: p.source,
                                                })
                                            })
                                            .collect();
                                        if let Err(e) =
                                            ntg.update_audio_ssrc_mappings(ntg_id, &mappings).await
                                        {
                                            warn!(
                                                "tgcalls: update_audio_ssrc_mappings failed: {}",
                                                e
                                            );
                                        }
                                        handler(ConferenceEvent::ParticipantsChanged);
                                    }
                                    Err(e) => warn!("tgcalls: getGroupParticipants failed: {}", e),
                                }
                            }
                        }
                        Command::EmojisUpdated(emoji) => {
                            handler(ConferenceEvent::FingerprintUpdated(emoji));
                        }
                        Command::StreamEnded(stream_type, device) => {
                            handler(ConferenceEvent::StreamEnded(stream_type, device));
                        }
                        Command::Frames {
                            mode,
                            device,
                            frames,
                        } => {
                            handler(ConferenceEvent::Frames {
                                mode,
                                device,
                                frames,
                            });
                        }
                        Command::RemoteSourceChanged(source) => {
                            handler(ConferenceEvent::RemoteSourceChanged(source));
                        }
                    }
                }

                let _ = ntg.stop(ntg_id).await;
            });
        });

        Self {
            tx,
            chat_id,
            media: media_view,
        }
    }

    /// Starts or joins the conference. See [`ConferenceTarget`]. Whether
    /// video is signaled is derived from `media` (does it have a
    /// camera/screen source), not a separate flag - see `media_has_video`.
    pub async fn start(
        &self,
        target: ConferenceTarget,
        media: Option<MediaDescription>,
    ) -> Result<(), TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Start {
                target,
                media,
                reply,
            })
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    pub async fn leave(&self) -> Result<(), TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Leave(reply))
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    /// The safety-number-style verification emoji. Compare this with
    /// every other participant out of band (voice, in person, ...) - a
    /// mismatch means someone's being MITM'd.
    pub async fn fingerprint_emojis(&self) -> Result<String, TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::FingerprintEmojis(reply))
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    pub async fn is_joined(&self) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Command::IsJoined(reply)).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// The conference's invite link (`t.me/call/<slug>`) - share this to
    /// let anyone join, no per-user invite needed. Only set if you created
    /// the conference (`ConferenceTarget::Create`); joining an existing one
    /// doesn't hand back a link.
    pub async fn invite_link(&self) -> Option<String> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Command::InviteLink(reply)).is_err() {
            return None;
        }
        rx.await.ok().flatten()
    }

    /// Rings another user into an already-running conference - the
    /// mid-call equivalent of [`ConferenceTarget::Create`]'s `invite`
    /// list. Call this as many times as you have people to add; there's
    /// no fixed participant limit enforced here (Telegram's own cap on
    /// this call type is 200 participants).
    ///
    /// `video: None` signals the invitee based on whatever's currently
    /// playing (from the last `start()`/`set_sources()` - see `media_has_video`).
    /// Pass `Some(v)` to override that for this specific invite - e.g. you
    /// might ring someone as a video call while only actually streaming
    /// audio to the room, or vice versa; the ring hint and your own stream
    /// don't have to match.
    pub async fn invite(&self, user_id: i64, video: Option<bool>) -> Result<(), TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Invite {
                user_id,
                video,
                reply,
            })
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    /// Replaces everything being captured (`StreamMode::Capture`: mic,
    /// camera, screen) or played back (`StreamMode::Playback`: where
    /// incoming audio / video goes, e.g. a recording target) in one call.
    /// Anything missing from `media` is removed. Useful if you joined with
    /// `media: None` and want to hold off sending until
    /// [`ConferenceEvent::ParticipantsChanged`] fires, so you're not
    /// sending to an empty room. For changing one source and keeping the
    /// rest, use [`ConferenceCall::set_audio`], [`ConferenceCall::set_video`]
    /// or [`ConferenceCall::set_screen`].
    pub async fn set_sources(
        &self,
        mode: StreamMode,
        media: &MediaDescription,
    ) -> Result<(), TgCallsError> {
        let media = media.clone();
        self.ask(|reply| Command::SetSources { mode, media, reply })
            .await
    }

    /// Sends a command to the actor thread and waits for its reply.
    async fn ask<T>(&self, build: impl FnOnce(Reply<T>) -> Command) -> Result<T, TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(build(reply))
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    /// Runs one control change on the actor thread and waits for it.
    async fn control(&self, control: Control) -> Result<(), TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Control(control, reply))
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    /// Silences your mic without leaving or changing the source.
    pub async fn mute(&self) -> Result<(), TgCallsError> {
        self.control(Control::Mute).await
    }

    pub async fn unmute(&self) -> Result<(), TgCallsError> {
        self.control(Control::Unmute).await
    }

    /// Pauses your outgoing stream (audio and video).
    pub async fn pause(&self) -> Result<(), TgCallsError> {
        self.control(Control::Pause).await
    }

    pub async fn resume(&self) -> Result<(), TgCallsError> {
        self.control(Control::Resume).await
    }

    /// Turns the camera on (`Some`), swaps it, or turns it off (`None`)
    /// without touching the mic or screen.
    pub async fn set_video(&self, camera: Option<VideoDescription>) -> Result<(), TgCallsError> {
        self.control(Control::Video(camera)).await
    }

    /// Sets, swaps or removes the outgoing audio source. To silence the
    /// mic while keeping the source, use [`ConferenceCall::mute`].
    pub async fn set_audio(
        &self,
        microphone: Option<AudioDescription>,
    ) -> Result<(), TgCallsError> {
        self.control(Control::Audio(microphone)).await
    }

    /// Starts (`Some`), swaps or stops (`None`) screen sharing.
    pub async fn set_screen(&self, screen: Option<VideoDescription>) -> Result<(), TgCallsError> {
        self.control(Control::Screen(screen)).await
    }

    pub async fn media_state(&self) -> Result<MediaState, TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::MediaState(reply))
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    /// What is currently being captured (mic, camera, screen).
    pub fn current_media(&self) -> MediaDescription {
        self.media.lock().unwrap().clone()
    }

    /// Pushes one raw frame into a source configured with
    /// `MediaSource::External` (see `Media::external_video` and
    /// `Media::external_audio`).
    pub async fn send_external_frame(
        &self,
        device: StreamDevice,
        frame: &[u8],
        frame_data: &FrameData,
    ) -> Result<(), TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::ExternalFrame {
                device,
                data: frame.to_vec(),
                frame_data: frame_data.clone(),
                reply,
            })
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    /// Sets how loud `user_id` sounds to *you*. 0 = muted for you, 10000 =
    /// 100% (default), up to 20000 = 200%. A per-listener preference, not a
    /// broadcast-wide control.
    pub async fn set_volume(&self, user_id: i64, volume: i32) -> Result<(), TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::SetVolume {
                user_id,
                volume,
                reply,
            })
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    /// Subscribes to a remote video stream. `user_id` is whose stream it
    /// is; same arguments as [`crate::Call::add_incoming_video`].
    pub async fn add_incoming_video(
        &self,
        user_id: i64,
        endpoint: &str,
        ssrc_groups: &[SsrcGroup],
    ) -> Result<u32, TgCallsError> {
        let endpoint = endpoint.to_owned();
        let ssrc_groups = ssrc_groups.to_vec();
        self.ask(|reply| Command::IncomingVideo {
            user_id,
            endpoint,
            ssrc_groups,
            reply,
        })
        .await
    }

    pub async fn remove_incoming_video(&self, endpoint: &str) -> Result<bool, TgCallsError> {
        let endpoint = endpoint.to_owned();
        self.ask(|reply| Command::RemoveIncomingVideo { endpoint, reply })
            .await
    }

    /// Seconds of the given stream played so far.
    pub async fn played_seconds(&self, mode: StreamMode) -> Result<u64, TgCallsError> {
        self.ask(|reply| Command::PlayedSeconds { mode, reply })
            .await
    }

    pub async fn call_type(&self) -> Result<CallType, TgCallsError> {
        self.ask(Command::CallType).await
    }

    pub async fn connection_mode(&self) -> Result<ConnectionMode, TgCallsError> {
        self.ask(Command::ConnectionMode).await
    }

    /// CPU used by the native call stack, as a fraction of one core.
    pub async fn cpu_usage(&self) -> Result<f64, TgCallsError> {
        self.ask(Command::CpuUsage).await
    }

    /// Fetches every current participant (paginated internally).
    pub async fn get_participants(
        &self,
    ) -> Result<Vec<tl::types::GroupCallParticipant>, TgCallsError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Command::Participants(reply))
            .map_err(|_| TgCallsError::WorkerGone)?;
        rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }

    /// Feed in an `UpdateGroupCallChainBlocks` received on the normal
    /// update stream. Register this the same way `Calls` registers
    /// `route_update` for classic group calls - see `examples/conference_calls.rs`.
    /// Safe to call for updates that belong to a different call/chat - it
    /// checks the update's `call` id against this conference's own before
    /// touching `ntg`, same pattern as `Calls`' `RouteUpdate`. That's what
    /// lets [`ConferenceCalls`] broadcast to every tracked chat instead of
    /// needing its own filtering logic.
    pub fn apply_chain_blocks(&self, u: &tl::types::UpdateGroupCallChainBlocks) {
        let _ = self.tx.send(Command::ApplyBlocks {
            call: u.call.clone(),
            sub_chain_id: u.sub_chain_id,
            blocks: u.blocks.clone(),
            next_offset: u.next_offset,
        });
    }

    pub fn chat_id(&self) -> i64 {
        self.chat_id
    }
}

fn public_key_array(bytes: &[u8]) -> Result<[u8; 32], TgCallsError> {
    bytes.try_into().map_err(|_| {
        TgCallsError::TransportParse(format!(
            "conference public key was {} bytes, expected 32",
            bytes.len()
        ))
    })
}

/// Resolves `user_id`'s access hash and sends `phone.inviteConferenceCallParticipant`.
/// Logs both the attempt and the outcome at `debug`/`info` - the RPC itself
/// doesn't otherwise show up in `RUST_LOG=debug` output, so this is the
/// only way to actually see whether an invite fired and what Telegram said
/// back, short of instrumenting ferogram's transport layer.
async fn invite_participant(
    client: &ferogram::Client,
    call: &tl::enums::InputGroupCall,
    user_id: i64,
    video: bool,
) -> Result<(), TgCallsError> {
    debug!("tgcalls: resolving access hash for {}", user_id);
    let access_hash = signaling::get_user_access_hash(client, user_id).await?;
    debug!(
        "tgcalls: inviting {} (access_hash={}) into conference",
        user_id, access_hash
    );
    signaling::invite_conference_call_participant(
        client,
        call.clone(),
        user_id,
        access_hash,
        video,
    )
    .await?;
    info!("tgcalls: invited {} into conference", user_id);
    Ok(())
}

/// Per-call context for [`start_conference`] - just bundles what would
/// otherwise be four separate parameters (clippy's `too_many_arguments`
/// threshold is 7; this plus `ntg`/`target`/`media`/`video_stopped` was 8).
struct StartCtx<'a> {
    client: &'a ferogram::Client,
    ntg_id: i64,
    chat_id: i64,
    connect_slot: &'a ConnectSlot,
}

async fn start_conference(
    ctx: &StartCtx<'_>,
    ntg: &mut NTgCalls,
    target: ConferenceTarget,
    media: Option<MediaDescription>,
    video_stopped: bool,
) -> Result<(tl::enums::InputGroupCall, Option<String>), TgCallsError> {
    let client = ctx.client;
    let ntg_id = ctx.ntg_id;
    let chat_id = ctx.chat_id;
    let connect_slot = ctx.connect_slot;

    let last_block = match &target {
        ConferenceTarget::Join { last_block }
        | ConferenceTarget::JoinBySlug { last_block, .. }
        | ConferenceTarget::JoinByInviteMessage { last_block, .. } => last_block.clone(),
        ConferenceTarget::Create { .. } => None,
    };

    // init_conference refuses to run unless `ntg_id` already has a
    // connection slot registered - it either migrates an existing P2PCall
    // in place or (for anything else already there) just replaces it, but
    // either way `exists(chat_id)` has to be true first, or it throws
    // `ConnectionNotFound`. `ConferenceCall` always starts from a brand new
    // `NTgCalls` with nothing registered yet, so we reserve that slot
    // ourselves with `create_p2p_call` right before `init_conference` -
    // conference join intentionally goes through the P2P-call code path,
    // not `create_call`.
    ntg.create_p2p_call(ntg_id).await?;
    let my_id = signaling::get_self_user_id(client).await?;
    let params = ntg
        .init_conference(ntg_id, my_id, last_block.as_deref())
        .await?;
    let public_key = public_key_array(&params.public_key)?;

    let (tx, rx) = oneshot::channel();
    *connect_slot.lock().unwrap() = Some(tx);

    let (call, transport_json, invite_link) = match target {
        ConferenceTarget::Create { invite } => {
            let (call, transport, invite_link) = signaling::create_conference_call(
                client,
                &params.payload,
                video_stopped,
                params.block.clone(),
                public_key,
            )
            .await?;
            for user_id in invite {
                if let Err(e) = invite_participant(client, &call, user_id, !video_stopped).await {
                    warn!(
                        "tgcalls: failed to invite {} into conference: {}",
                        user_id, e
                    );
                }
            }
            (call, transport, invite_link)
        }
        // Same join RPC either way - only how `call` is identified differs,
        // per https://core.telegram.org/api/end-to-end/group-calls: chat_id
        // lookup, deep-link slug, or the invite message id, respectively.
        ConferenceTarget::Join { .. } => {
            let call = signaling::resolve_call(client, chat_id).await?;
            join_conference(
                client,
                ntg,
                ntg_id,
                call,
                &params,
                video_stopped,
                public_key,
            )
            .await?
        }
        ConferenceTarget::JoinBySlug { slug, .. } => {
            let call = tl::enums::InputGroupCall::Slug(tl::types::InputGroupCallSlug { slug });
            join_conference(
                client,
                ntg,
                ntg_id,
                call,
                &params,
                video_stopped,
                public_key,
            )
            .await?
        }
        ConferenceTarget::JoinByInviteMessage { msg_id, .. } => {
            let call =
                tl::enums::InputGroupCall::InviteMessage(tl::types::InputGroupCallInviteMessage {
                    msg_id,
                });
            join_conference(
                client,
                ntg,
                ntg_id,
                call,
                &params,
                video_stopped,
                public_key,
            )
            .await?
        }
    };

    ntg.connect(ntg_id, &transport_json, false).await?;
    // connect() only dispatches the SDP; Connected arrives via the
    // connect_slot callback. set_stream_sources has to come after this,
    // not before - configuring a media source before the transport exists
    // silently doesn't work (ntgcalls doesn't buffer/replay it once the
    // connection later comes up). Matches the proven order in
    // Call::try_join, which does connect -> await Connected -> THEN
    // set_stream_sources for exactly this reason.
    rx.await.expect("connection callback never fired")?;

    if let Some(media) = &media {
        ntg.set_stream_sources(ntg_id, StreamMode::Capture, media)
            .await?;
    }

    Ok((call, invite_link))
}

/// Shared join path for [`ConferenceTarget::Join`]/`JoinBySlug`/
/// `JoinByInviteMessage` - only `call` differs between them. Applies any
/// chain blocks Telegram piggy-backs on the join response immediately.
/// Joining never returns an invite link the way creating does, so the
/// third tuple slot is always `None` - kept for a uniform return type with
/// the `Create` branch.
async fn join_conference(
    client: &ferogram::Client,
    ntg: &mut NTgCalls,
    ntg_id: i64,
    call: tl::enums::InputGroupCall,
    params: &ntgcalls::ConferenceJoinParams,
    video_stopped: bool,
    public_key: [u8; 32],
) -> Result<(tl::enums::InputGroupCall, String, Option<String>), TgCallsError> {
    let (transport, chain_blocks) = signaling::join_conference_call(
        client,
        call.clone(),
        &params.payload,
        video_stopped,
        params.block.clone(),
        public_key,
    )
    .await?;
    if let Some(cb) = chain_blocks {
        let _ = ntg
            .apply_blocks(ntg_id, cb.sub_chain_id, cb.next_offset, &cb.blocks, false)
            .await;
    }
    Ok((call, transport, None))
}

/// Chat-keyed manager on top of [`ConferenceCall`] - the `Calls` equivalent
/// for conferences. One instance covers every chat your client runs a
/// conference in; register it once as middleware and `UpdateGroupCallChainBlocks`
/// gets routed to the right chat automatically, the same way `Calls`
/// auto-routes `UpdateGroupCallParticipants`.
///
/// Each tracked chat still gets its own dedicated actor thread underneath
/// (see the module docs for why) - this type just keeps a `chat_id ->
/// ConferenceCall` map and broadcasts routed updates to all of them, each
/// one self-filtering by call id (see [`ConferenceCall::apply_chain_blocks`]).
///
/// ```rust,no_run
/// # use ferogram::filters::Dispatcher;
/// # use tgcalls::ConferenceCalls;
/// # async fn example(client: ferogram::Client) {
/// let conferences = ConferenceCalls::new(client);
/// let mut dp = Dispatcher::new();
/// dp.middleware(conferences.clone());
///
/// conferences.create(-100123456789, vec![987654321], None).await.unwrap();
/// # }
/// ```
/// A [`ConferenceEvent`] handler tagged with which chat it's for -
/// `ConferenceCalls` covers many chats, unlike a single `ConferenceCall`'s
/// handler.
type ManagerEventHandler = Arc<dyn Fn(i64, ConferenceEvent) + Send + Sync>;

#[derive(Clone)]
pub struct ConferenceCalls {
    client: ferogram::Client,
    active: Arc<tokio::sync::Mutex<HashMap<i64, Arc<ConferenceCall>>>>,
    event_handler: Arc<Mutex<Option<ManagerEventHandler>>>,
    max_concurrent: Option<usize>,
}

impl ConferenceCalls {
    pub fn new(client: ferogram::Client) -> Self {
        Self {
            client,
            active: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            event_handler: Arc::new(Mutex::new(None)),
            max_concurrent: None,
        }
    }

    /// Same as [`ConferenceCalls::new`], but caps how many chats can have
    /// an active conference at once. Each tracked chat gets its own
    /// dedicated actor thread (see the module docs) - fine at tens or low
    /// hundreds of concurrent conferences, but unbounded growth isn't free
    /// at very high counts. `start`/`create`/`join` return
    /// [`TgCallsError::TooManyConcurrentCalls`] instead of starting a new
    /// conference once the limit is hit; chats already active are
    /// unaffected (same rule as `Calls::with_concurrency_limit`).
    pub fn with_concurrency_limit(client: ferogram::Client, max_concurrent: usize) -> Self {
        Self {
            max_concurrent: Some(max_concurrent),
            ..Self::new(client)
        }
    }

    /// Registers a handler for [`ConferenceEvent`] across every tracked
    /// chat. Set before a chat's first `create()`/`join()` - a chat
    /// already started won't pick up a handler registered after the fact
    /// (same rule as `Calls::on_event`).
    pub fn on_event(&self, handler: impl Fn(i64, ConferenceEvent) + Send + Sync + 'static) {
        *self.event_handler.lock().unwrap() = Some(Arc::new(handler));
    }

    async fn get_or_create(&self, chat_id: i64) -> Result<Arc<ConferenceCall>, TgCallsError> {
        if let Some(conference) = self.active.lock().await.get(&chat_id) {
            return Ok(conference.clone());
        }

        if let Some(max) = self.max_concurrent {
            if self.active.lock().await.len() >= max {
                return Err(TgCallsError::TooManyConcurrentCalls(max));
            }
        }

        // Spawned outside the lock: starting a conference (an OS thread
        // plus its own Tokio runtime) shouldn't hold up lookups for every
        // other chat while it happens. If two callers race for the same
        // chat_id, the loser's conference is simply dropped here - its
        // thread exits on its own once the sender goes away, since
        // nothing was ever joined on it yet (same rule as
        // `Calls::get_or_create`).
        let handler = self.event_handler.lock().unwrap().clone();
        let conference = Arc::new(ConferenceCall::new(
            self.client.clone(),
            chat_id,
            move |event| {
                if let Some(handler) = &handler {
                    handler(chat_id, event);
                }
            },
        ));

        let mut active = self.active.lock().await;
        Ok(active.entry(chat_id).or_insert(conference).clone())
    }

    async fn get(&self, chat_id: i64) -> Result<Arc<ConferenceCall>, TgCallsError> {
        self.active
            .lock()
            .await
            .get(&chat_id)
            .cloned()
            .ok_or(TgCallsError::NotJoined)
    }

    /// Starts or joins `chat_id`'s conference with any [`ConferenceTarget`] -
    /// `create`/`join` below cover the two common cases; use this directly
    /// for [`ConferenceTarget::JoinBySlug`]/[`ConferenceTarget::JoinByInviteMessage`]
    /// (e.g. via [`ConferenceInvite::target`]), which don't have their own
    /// dedicated manager method since there's no single natural `chat_id`
    /// to key a slug-joined conference under - that choice is yours.
    pub async fn start(
        &self,
        chat_id: i64,
        target: ConferenceTarget,
        media: Option<MediaDescription>,
    ) -> Result<(), TgCallsError> {
        let conference = self.get_or_create(chat_id).await?;
        conference.start(target, media).await
    }

    /// Starts a brand new conference in `chat_id` and rings everyone in
    /// `invite`. Ring more people later with [`ConferenceCalls::invite`].
    /// Whether video is signaled is derived from `media`, not a separate
    /// flag - see `media_has_video`.
    pub async fn create(
        &self,
        chat_id: i64,
        invite: Vec<i64>,
        media: Option<MediaDescription>,
    ) -> Result<(), TgCallsError> {
        self.start(chat_id, ConferenceTarget::Create { invite }, media)
            .await
    }

    /// Joins (or rejoins) an existing conference already linked to
    /// `chat_id`. `last_block` if you already have one cached for this
    /// chat (e.g. persisted from a previous run) - `None` for a fresh
    /// join. For joining via a shared link or an explicit invite message
    /// instead, use [`Self::start`] with [`ConferenceTarget::JoinBySlug`] /
    /// [`ConferenceTarget::JoinByInviteMessage`] - see
    /// [`ConferenceTarget::Join`]'s docs for why those aren't the same
    /// thing as this.
    pub async fn join(
        &self,
        chat_id: i64,
        last_block: Option<Vec<u8>>,
        media: Option<MediaDescription>,
    ) -> Result<(), TgCallsError> {
        self.start(chat_id, ConferenceTarget::Join { last_block }, media)
            .await
    }

    /// Rings another user into `chat_id`'s already-running conference. See
    /// [`ConferenceCall::invite`] - `video: None` auto-derives from
    /// whatever's playing, `Some(v)` overrides it for this invite.
    pub async fn invite(
        &self,
        chat_id: i64,
        user_id: i64,
        video: Option<bool>,
    ) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.invite(user_id, video).await
    }

    /// Replaces everything `chat_id`'s conference is capturing or playing
    /// back. See [`ConferenceCall::set_sources`].
    pub async fn set_sources(
        &self,
        chat_id: i64,
        mode: StreamMode,
        media: &MediaDescription,
    ) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.set_sources(mode, media).await
    }

    pub async fn mute(&self, chat_id: i64) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.mute().await
    }

    pub async fn unmute(&self, chat_id: i64) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.unmute().await
    }

    pub async fn pause(&self, chat_id: i64) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.pause().await
    }

    pub async fn resume(&self, chat_id: i64) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.resume().await
    }

    pub async fn set_video(
        &self,
        chat_id: i64,
        camera: Option<VideoDescription>,
    ) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.set_video(camera).await
    }

    pub async fn set_audio(
        &self,
        chat_id: i64,
        microphone: Option<AudioDescription>,
    ) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.set_audio(microphone).await
    }

    pub async fn set_screen(
        &self,
        chat_id: i64,
        screen: Option<VideoDescription>,
    ) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.set_screen(screen).await
    }

    pub async fn media_state(&self, chat_id: i64) -> Result<MediaState, TgCallsError> {
        self.get(chat_id).await?.media_state().await
    }

    /// What `chat_id`'s conference is currently capturing.
    pub async fn current_media(&self, chat_id: i64) -> Result<MediaDescription, TgCallsError> {
        Ok(self.get(chat_id).await?.current_media())
    }

    /// See [`ConferenceCall::send_external_frame`].
    pub async fn send_external_frame(
        &self,
        chat_id: i64,
        device: StreamDevice,
        frame: &[u8],
        frame_data: &FrameData,
    ) -> Result<(), TgCallsError> {
        self.get(chat_id)
            .await?
            .send_external_frame(device, frame, frame_data)
            .await
    }

    /// See [`ConferenceCall::add_incoming_video`].
    pub async fn add_incoming_video(
        &self,
        chat_id: i64,
        user_id: i64,
        endpoint: &str,
        ssrc_groups: &[SsrcGroup],
    ) -> Result<u32, TgCallsError> {
        self.get(chat_id)
            .await?
            .add_incoming_video(user_id, endpoint, ssrc_groups)
            .await
    }

    pub async fn remove_incoming_video(
        &self,
        chat_id: i64,
        endpoint: &str,
    ) -> Result<bool, TgCallsError> {
        self.get(chat_id)
            .await?
            .remove_incoming_video(endpoint)
            .await
    }

    pub async fn played_seconds(
        &self,
        chat_id: i64,
        mode: StreamMode,
    ) -> Result<u64, TgCallsError> {
        self.get(chat_id).await?.played_seconds(mode).await
    }

    pub async fn call_type(&self, chat_id: i64) -> Result<CallType, TgCallsError> {
        self.get(chat_id).await?.call_type().await
    }

    pub async fn connection_mode(&self, chat_id: i64) -> Result<ConnectionMode, TgCallsError> {
        self.get(chat_id).await?.connection_mode().await
    }

    pub async fn cpu_usage(&self, chat_id: i64) -> Result<f64, TgCallsError> {
        self.get(chat_id).await?.cpu_usage().await
    }

    /// See [`ConferenceCall::set_volume`].
    pub async fn set_volume(
        &self,
        chat_id: i64,
        user_id: i64,
        volume: i32,
    ) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.set_volume(user_id, volume).await
    }

    /// See [`ConferenceCall::get_participants`].
    pub async fn get_participants(
        &self,
        chat_id: i64,
    ) -> Result<Vec<tl::types::GroupCallParticipant>, TgCallsError> {
        self.get(chat_id).await?.get_participants().await
    }

    /// Chats with a tracked conference.
    pub async fn chats(&self) -> Vec<i64> {
        self.active.lock().await.keys().copied().collect()
    }

    /// Leaves `chat_id`'s conference and tears down its worker thread.
    pub async fn leave(&self, chat_id: i64) -> Result<(), TgCallsError> {
        let conference = self.get(chat_id).await?;
        let result = conference.leave().await;
        self.active.lock().await.remove(&chat_id);
        result
    }

    /// The safety-number-style verification emoji for `chat_id`'s
    /// conference. See [`ConferenceCall::fingerprint_emojis`].
    pub async fn fingerprint_emojis(&self, chat_id: i64) -> Result<String, TgCallsError> {
        self.get(chat_id).await?.fingerprint_emojis().await
    }

    pub async fn is_joined(&self, chat_id: i64) -> bool {
        match self.active.lock().await.get(&chat_id) {
            Some(conference) => conference.is_joined().await,
            None => false,
        }
    }

    /// `chat_id`'s conference invite link. See [`ConferenceCall::invite_link`].
    pub async fn invite_link(&self, chat_id: i64) -> Option<String> {
        self.get(chat_id).await.ok()?.invite_link().await
    }

    /// Leaves every tracked chat concurrently and tears down their worker
    /// threads. Not a `Drop` impl for the same reason as `Calls::shutdown` -
    /// call it yourself from your own shutdown handling.
    pub async fn shutdown(&self) -> Vec<(i64, Result<(), TgCallsError>)> {
        let chat_ids: Vec<i64> = self.active.lock().await.keys().copied().collect();

        let mut tasks = tokio::task::JoinSet::new();
        for chat_id in chat_ids {
            let conferences = self.clone();
            tasks.spawn(async move {
                let result = conferences.leave(chat_id).await;
                (chat_id, result)
            });
        }

        let mut results = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok(pair) = joined {
                results.push(pair);
            }
        }
        results
    }

    /// Broadcasts an `UpdateGroupCallChainBlocks` to every tracked chat;
    /// each one checks whether it's actually theirs before acting on it
    /// (see [`ConferenceCall::apply_chain_blocks`]). Runs automatically
    /// once `ConferenceCalls` is registered as middleware.
    async fn route_chain_blocks(&self, u: &tl::types::UpdateGroupCallChainBlocks) {
        let conferences: Vec<_> = self.active.lock().await.values().cloned().collect();
        for conference in conferences {
            conference.apply_chain_blocks(u);
        }
    }
}

impl Middleware for ConferenceCalls {
    fn call(&self, update: Update, next: Next) -> BoxFuture {
        let conferences = self.clone();
        Box::pin(async move {
            if let Update::Raw(raw) = &update {
                if let tl::enums::Update::GroupCallChainBlocks(u) = &raw.inner {
                    conferences.route_chain_blocks(u).await;
                }
            }
            next.run(update).await
        })
    }
}

/// Reacts to a P2P call being upgraded to a conference mid-call
/// (`PhoneCallDiscardReasonMigrateConferenceCall`) by resolving the new
/// conference and joining it with whatever chain history is already
/// available. Returns `Ok(false)` for every other discard reason (or
/// `None`), so it's safe to call unconditionally from wherever you already
/// handle [`crate::P2PEvent::Discarded`] - no need to pattern-match the
/// reason yourself first:
///
/// ```rust,no_run
/// # use tgcalls::{ConferenceCalls, P2PEvent};
/// # async fn example(
/// #     client: ferogram::Client, conferences: ConferenceCalls, chat_id: i64,
/// #     event: P2PEvent,
/// # ) -> Result<(), tgcalls::TgCallsError> {
/// let P2PEvent::Discarded(reason) = event;
/// if tgcalls::migrate_from_p2p(&client, &conferences, chat_id, &reason, None).await? {
///     println!("call upgraded to a conference, rejoined as one");
/// }
/// # Ok(())
/// # }
/// ```
///
/// `chat_id` is the private chat the P2P call was running in - per
/// ferogram's convention that's the same as the other user's id.
///
/// Resolving the conference (`signaling::resolve_call`) relies on
/// Telegram having already linked the new group call to the chat by the
/// time this runs - it's not special-cased through the migration slug,
/// just the normal chat-linked-call lookup every other conference join
/// uses.
pub async fn migrate_from_p2p(
    client: &ferogram::Client,
    conferences: &ConferenceCalls,
    chat_id: i64,
    reason: &Option<tl::enums::PhoneCallDiscardReason>,
    media: Option<MediaDescription>,
) -> Result<bool, TgCallsError> {
    let Some(tl::enums::PhoneCallDiscardReason::MigrateConferenceCall(_)) = reason else {
        return Ok(false);
    };

    let call = signaling::resolve_call(client, chat_id).await?;
    let last_block = signaling::get_conference_chain_blocks(client, call, 0, -1, 1)
        .await?
        .and_then(|cb| cb.blocks.into_iter().next_back());

    conferences.join(chat_id, last_block, media).await?;
    Ok(true)
}

/// A conference call announced in a chat's message history. Telegram
/// signals a conference starting the same way it signals a classic call
/// starting - a `MessageActionConferenceCall` system message, not a
/// call-specific update - so this has nothing to do with `ConferenceCall`
/// or `ConferenceCalls` directly; it's just message parsing.
#[derive(Debug, Clone)]
pub struct ConferenceInvite {
    pub chat_id: i64,
    pub call_id: i64,
    /// The invite message's own ID - this is what actually identifies the
    /// conference for joining purposes (`InputGroupCallInviteMessage`),
    /// not `chat_id`. See [`Self::target`].
    pub msg_id: i32,
    pub video: bool,
    /// `true` if this message is reporting a call that was already missed -
    /// nothing left to join.
    pub missed: bool,
    /// `true` if the conference was still ongoing as of this message. Note
    /// this only reflects the moment the message was sent - it can still
    /// have ended by the time you act on it; a failed `join()` afterward is
    /// the real answer to "is it still going".
    pub active: bool,
}

impl ConferenceInvite {
    /// Shorthand for "there's actually something to join right now" -
    /// `active && !missed`.
    pub fn should_join(&self) -> bool {
        self.active && !self.missed
    }

    /// The correct [`ConferenceTarget`] to join this specific invite with -
    /// by invite message id, not `chat_id`, per
    /// <https://core.telegram.org/api/end-to-end/group-calls>.
    pub fn target(&self, last_block: Option<Vec<u8>>) -> ConferenceTarget {
        ConferenceTarget::JoinByInviteMessage {
            msg_id: self.msg_id,
            last_block,
        }
    }
}

/// Extracts a [`ConferenceInvite`] from an update, if it's a
/// `MessageActionConferenceCall` system message. Returns `None` for every
/// other update, so it's safe to call unconditionally on your update
/// stream alongside your other per-update checks. Nothing here decides
/// whether to join - that's up to your app.
///
/// ```rust,no_run
/// # use tgcalls::{incoming_conference_call, ConferenceCalls};
/// # async fn example(update: ferogram::Update, conferences: ConferenceCalls) {
/// if let Some(invite) = incoming_conference_call(&update) {
///     if invite.should_join() {
///         // Join by invite message id, not chat_id - see `ConferenceInvite::target`.
///         let _ = conferences.start(invite.chat_id, invite.target(None), None).await;
///     }
/// }
/// # }
/// ```
pub fn incoming_conference_call(update: &Update) -> Option<ConferenceInvite> {
    let Update::NewMessage(msg) = update else {
        return None;
    };
    let tl::enums::Message::Service(svc) = &msg.raw else {
        return None;
    };
    let tl::enums::MessageAction::ConferenceCall(action) = &svc.action else {
        return None;
    };
    Some(ConferenceInvite {
        chat_id: msg.chat_id(),
        call_id: action.call_id,
        msg_id: svc.id,
        video: action.video,
        missed: action.missed,
        active: action.active,
    })
}
