//! High-level chat-keyed manager on top of [`Call`].
//!
//! `ntgcalls::NTgCalls` is `Send` but deliberately not `Sync` - the native
//! instance can move between threads but two threads can never touch it at
//! once. Every one of its methods takes `&self` internally, so any async
//! call into it captures `&NTgCalls` across an internal await, which can
//! never be `Send` regardless of how `Call` itself is wrapped. There's no
//! way around this other than the standard fix for a `Send`-not-`Sync`
//! resource: give each chat's `Call` a dedicated thread that owns it for
//! life, and talk to it only through channels. That's what this file is.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ferogram::middleware::{BoxFuture, Middleware, Next};
use ferogram::tl;
use ferogram::Update;
use ntgcalls::{CallType, ConnectionMode, MediaDescription, StreamMode};
use tokio::sync::{mpsc, oneshot};

use crate::media::{auto_media_at, probe_duration};
use crate::{auto_media, error::TgCallsError, Call, CallEvent};

type EventHandler = Arc<dyn Fn(i64, CallEvent) + Send + Sync>;
type Reply<T> = oneshot::Sender<Result<T, TgCallsError>>;

/// How far into the current stream you are. `total`/`remaining` are `None`
/// for sources with no fixed length (a live stream, a device, a recording
/// target) - `ffprobe` genuinely can't tell you that.
#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    pub elapsed: Duration,
    pub total: Option<Duration>,
}

impl Progress {
    pub fn remaining(&self) -> Option<Duration> {
        self.total.map(|t| t.saturating_sub(self.elapsed))
    }

    pub fn percent(&self) -> Option<f64> {
        self.total
            .filter(|t| !t.is_zero())
            .map(|t| self.elapsed.as_secs_f64() / t.as_secs_f64() * 100.0)
    }
}

/// A one-call snapshot of everything about a chat's call worth checking at
/// once, instead of five separate round trips.
#[derive(Debug, Clone)]
pub struct CallStatus {
    pub joined: bool,
    pub call_type: Option<CallType>,
    pub connection_mode: Option<ConnectionMode>,
    pub muted: bool,
    pub video_paused: bool,
    pub progress: Progress,
}

enum Command {
    Play(MediaDescription, Option<Duration>, Reply<()>),
    Seek(MediaDescription, Option<Duration>, Duration, Reply<()>),
    Record(MediaDescription, Reply<()>),
    Leave(Reply<()>),
    Pause(Reply<()>),
    Resume(Reply<()>),
    Mute(Reply<()>),
    Unmute(Reply<()>),
    SetVolume(i64, i32, Reply<()>),
    GetParticipants(Reply<Vec<tl::types::GroupCallParticipant>>),
    CallType(Reply<CallType>),
    ConnectionMode(Reply<ConnectionMode>),
    Progress(Reply<Progress>),
    Status(Reply<CallStatus>),
    IsJoined(oneshot::Sender<bool>),
    RouteUpdate(tl::types::UpdateGroupCallParticipants),
    RouteCallEnded(i64),
}

/// A running chat's actor thread - just a channel handle, the `Call` itself
/// never leaves its thread.
#[derive(Clone)]
struct Worker {
    tx: mpsc::UnboundedSender<Command>,
}

impl Worker {
    fn spawn(client: ferogram::Client, chat_id: i64, on_event: Option<EventHandler>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Command>();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tgcalls worker: failed to start runtime");

            rt.block_on(async move {
                let mut call = Call::new(client, chat_id);
                if let Some(handler) = on_event {
                    call.on_event(move |event| handler(chat_id, event));
                }

                // Tracks the *current* stream's position, since ntgcalls'
                // own time() accumulates across the whole session rather
                // than resetting per-track (confirmed against the native
                // source: sink objects are created once and reused across
                // set_stream_sources calls). `baseline_secs` is time()'s
                // value captured right after the current track started, so
                // `time() - baseline_secs` is always this track's elapsed
                // time regardless of what came before it.
                let mut track_duration: Option<Duration> = None;
                let mut seek_offset = Duration::ZERO;
                let mut baseline_secs: u64 = 0;

                while let Some(cmd) = rx.recv().await {
                    match cmd {
                        Command::Play(media, duration, reply) => {
                            let result = if call.is_joined() {
                                call.set_stream_sources(StreamMode::Capture, &media).await
                            } else {
                                call.create_and_join(media).await
                            };
                            if result.is_ok() {
                                track_duration = duration;
                                seek_offset = Duration::ZERO;
                                baseline_secs =
                                    call.played_seconds(StreamMode::Capture).await.unwrap_or(0);
                            }
                            let _ = reply.send(result);
                        }
                        Command::Seek(media, duration, offset, reply) => {
                            let result = if call.is_joined() {
                                call.set_stream_sources(StreamMode::Capture, &media).await
                            } else {
                                call.create_and_join(media).await
                            };
                            if result.is_ok() {
                                track_duration = duration;
                                seek_offset = offset;
                                baseline_secs =
                                    call.played_seconds(StreamMode::Capture).await.unwrap_or(0);
                            }
                            let _ = reply.send(result);
                        }
                        Command::Record(media, reply) => {
                            let result = async {
                                if !call.is_joined() {
                                    call.create_and_join(silent_media()).await?;
                                }
                                call.record(&media).await
                            }
                            .await;
                            let _ = reply.send(result);
                        }
                        Command::Leave(reply) => {
                            let result = call.leave().await;
                            let _ = reply.send(result);
                            break;
                        }
                        Command::Pause(reply) => {
                            let _ = reply.send(call.pause().await);
                        }
                        Command::Resume(reply) => {
                            let _ = reply.send(call.resume().await);
                        }
                        Command::Mute(reply) => {
                            let _ = reply.send(call.mute().await);
                        }
                        Command::Unmute(reply) => {
                            let _ = reply.send(call.unmute().await);
                        }
                        Command::SetVolume(user_id, volume, reply) => {
                            let _ = reply.send(call.set_volume(user_id, volume).await);
                        }
                        Command::GetParticipants(reply) => {
                            let _ = reply.send(call.get_participants().await);
                        }
                        Command::CallType(reply) => {
                            let _ = reply.send(call.call_type().await);
                        }
                        Command::ConnectionMode(reply) => {
                            let _ = reply.send(call.connection_mode().await);
                        }
                        Command::Progress(reply) => {
                            let result =
                                call.played_seconds(StreamMode::Capture).await.map(|raw| {
                                    let delta =
                                        Duration::from_secs(raw.saturating_sub(baseline_secs));
                                    Progress {
                                        elapsed: seek_offset + delta,
                                        total: track_duration,
                                    }
                                });
                            let _ = reply.send(result);
                        }
                        Command::Status(reply) => {
                            let result = async {
                                let raw =
                                    call.played_seconds(StreamMode::Capture).await.unwrap_or(0);
                                let delta = Duration::from_secs(raw.saturating_sub(baseline_secs));
                                let progress = Progress {
                                    elapsed: seek_offset + delta,
                                    total: track_duration,
                                };
                                let media_state = call.media_state().await.ok();
                                Ok(CallStatus {
                                    joined: call.is_joined(),
                                    call_type: call.call_type().await.ok(),
                                    connection_mode: call.connection_mode().await.ok(),
                                    muted: media_state.as_ref().map(|s| s.muted).unwrap_or(false),
                                    video_paused: media_state
                                        .map(|s| s.video_paused)
                                        .unwrap_or(false),
                                    progress,
                                })
                            }
                            .await;
                            let _ = reply.send(result);
                        }
                        Command::IsJoined(reply) => {
                            let _ = reply.send(call.is_joined());
                        }
                        Command::RouteUpdate(u) => {
                            let matches = matches!(
                                &u.call,
                                tl::enums::InputGroupCall::InputGroupCall(g)
                                    if call.group_call_id() == Some(g.id)
                            );
                            if matches {
                                let _ = call.handle_participants_update(&u.participants).await;
                            }
                        }
                        Command::RouteCallEnded(discarded_id) => {
                            call.handle_call_ended(discarded_id);
                        }
                    }
                }
            });
        });

        Self { tx }
    }

    async fn send<T>(&self, build: impl FnOnce(Reply<T>) -> Command) -> Result<T, TgCallsError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(build(reply_tx))
            .map_err(|_| TgCallsError::WorkerGone)?;
        reply_rx.await.map_err(|_| TgCallsError::WorkerGone)?
    }
}

fn silent_media() -> MediaDescription {
    MediaDescription {
        microphone: None,
        speaker: None,
        camera: None,
        screen: None,
    }
}

/// Chat-keyed manager on top of [`Call`]. One instance covers every chat
/// your bot is in; register it once as middleware and video auto-subscribe
/// works everywhere for free.
///
/// ```rust,no_run
/// # use ferogram::filters::Dispatcher;
/// # use tgcalls::Calls;
/// # async fn example(client: ferogram::Client) {
/// let calls = Calls::new(client);
/// let mut dp = Dispatcher::new();
/// dp.middleware(calls.clone());
///
/// calls.play(-100123456789, "video.mp4").await.unwrap();
/// # }
/// ```
#[derive(Clone)]
pub struct Calls {
    client: ferogram::Client,
    active: Arc<tokio::sync::Mutex<HashMap<i64, Worker>>>,
    event_handler: Arc<std::sync::Mutex<Option<EventHandler>>>,
}

impl Calls {
    pub fn new(client: ferogram::Client) -> Self {
        Self {
            client,
            active: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            event_handler: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Registers a handler for [`CallEvent`] across every tracked chat -
    /// stream end, participant changes, unexpected removal, the call
    /// ending. Set before a chat's first `play()`/`record()` - a chat
    /// already joined won't pick up a handler registered after the fact.
    pub fn on_event(&self, handler: impl Fn(i64, CallEvent) + Send + Sync + 'static) {
        *self.event_handler.lock().unwrap() = Some(Arc::new(handler));
    }

    async fn get_or_create(&self, chat_id: i64) -> Worker {
        let mut active = self.active.lock().await;
        if let Some(worker) = active.get(&chat_id) {
            return worker.clone();
        }
        let handler = self.event_handler.lock().unwrap().clone();
        let worker = Worker::spawn(self.client.clone(), chat_id, handler);
        active.insert(chat_id, worker.clone());
        worker
    }

    async fn get(&self, chat_id: i64) -> Result<Worker, TgCallsError> {
        self.active
            .lock()
            .await
            .get(&chat_id)
            .cloned()
            .ok_or(TgCallsError::NotJoined)
    }

    /// Joins and streams `source`, or switches the stream if already joined.
    /// Starts a new voice chat first if none is active yet. Picks audio-only
    /// vs audio+video by probing the source. For anything else (custom
    /// resolution, screen share, external frames, or joining without
    /// auto-starting) use [`Call`] directly.
    pub async fn play(&self, chat_id: i64, source: impl Into<String>) -> Result<(), TgCallsError> {
        let source = source.into();
        // auto_media/probe_duration shell out to ffprobe synchronously (up
        // to PROBE_TIMEOUT) - keep that off whatever runtime thread called
        // us, not just off the worker's.
        let (media, duration) = tokio::task::spawn_blocking({
            let source = source.clone();
            move || (auto_media(&source, 1280, 720, 30), probe_duration(&source))
        })
        .await
        .expect("tgcalls: media probe task panicked");

        let worker = self.get_or_create(chat_id).await;
        worker.send(|r| Command::Play(media, duration, r)).await
    }

    /// Restarts the stream at `offset` into `source` (fast, keyframe-based
    /// input seeking - not frame-accurate, see the `media` module docs).
    /// Joins first if not already in the call, same as `play`. Only
    /// meaningful for a seekable source (a file or a static URL, not a
    /// live stream).
    pub async fn seek(
        &self,
        chat_id: i64,
        source: impl Into<String>,
        offset: Duration,
    ) -> Result<(), TgCallsError> {
        let source = source.into();
        let (media, duration) = tokio::task::spawn_blocking({
            let source = source.clone();
            move || {
                (
                    auto_media_at(&source, offset, 1280, 720, 30),
                    probe_duration(&source),
                )
            }
        })
        .await
        .expect("tgcalls: media probe task panicked");

        let worker = self.get_or_create(chat_id).await;
        worker
            .send(|r| Command::Seek(media, duration, offset, r))
            .await
    }

    /// Records `media` (see `Media::record_audio`/`record_video`/
    /// `record_screen`), joining silently first if not already in the call.
    pub async fn record(&self, chat_id: i64, media: MediaDescription) -> Result<(), TgCallsError> {
        self.get_or_create(chat_id)
            .await
            .send(|r| Command::Record(media, r))
            .await
    }

    /// How far into the current stream `chat_id` is. `total`/`remaining`
    /// are `None` for sources ffprobe couldn't measure (a live stream, a
    /// device, a recording target).
    pub async fn progress(&self, chat_id: i64) -> Result<Progress, TgCallsError> {
        self.get(chat_id).await?.send(Command::Progress).await
    }

    /// A one-call snapshot of `chat_id`'s call: joined state, call type,
    /// connection mode, mute/video-pause flags, and progress - instead of
    /// five separate round trips.
    pub async fn status(&self, chat_id: i64) -> Result<CallStatus, TgCallsError> {
        self.get(chat_id).await?.send(Command::Status).await
    }

    /// Leaves `chat_id`'s call and tears down its worker thread.
    pub async fn leave(&self, chat_id: i64) -> Result<(), TgCallsError> {
        let worker = self.get(chat_id).await?;
        let result = worker.send(Command::Leave).await;
        self.active.lock().await.remove(&chat_id);
        result
    }

    pub async fn pause(&self, chat_id: i64) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.send(Command::Pause).await
    }

    pub async fn resume(&self, chat_id: i64) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.send(Command::Resume).await
    }

    pub async fn mute(&self, chat_id: i64) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.send(Command::Mute).await
    }

    pub async fn unmute(&self, chat_id: i64) -> Result<(), TgCallsError> {
        self.get(chat_id).await?.send(Command::Unmute).await
    }

    pub async fn set_volume(
        &self,
        chat_id: i64,
        user_id: i64,
        volume: i32,
    ) -> Result<(), TgCallsError> {
        self.get(chat_id)
            .await?
            .send(|r| Command::SetVolume(user_id, volume, r))
            .await
    }

    pub async fn get_participants(
        &self,
        chat_id: i64,
    ) -> Result<Vec<tl::types::GroupCallParticipant>, TgCallsError> {
        self.get(chat_id)
            .await?
            .send(Command::GetParticipants)
            .await
    }

    pub async fn call_type(&self, chat_id: i64) -> Result<CallType, TgCallsError> {
        self.get(chat_id).await?.send(Command::CallType).await
    }

    pub async fn connection_mode(&self, chat_id: i64) -> Result<ConnectionMode, TgCallsError> {
        self.get(chat_id).await?.send(Command::ConnectionMode).await
    }

    /// Leaves every tracked chat concurrently and tears down their worker
    /// threads. Returns each chat's leave result, so one bad chat doesn't
    /// block reporting on the rest.
    ///
    /// Not a `Drop` impl - `Calls` is cheap to clone (every clone shares the
    /// same state), so tying shutdown to one clone's lifetime would fire at
    /// the wrong time whenever any clone happened to drop first. Call this
    /// once, yourself, from your own shutdown handling (e.g. on Ctrl+C).
    pub async fn shutdown(&self) -> Vec<(i64, Result<(), TgCallsError>)> {
        let chat_ids: Vec<i64> = self.active.lock().await.keys().copied().collect();

        let mut tasks = tokio::task::JoinSet::new();
        for chat_id in chat_ids {
            let calls = self.clone();
            tasks.spawn(async move {
                let result = calls.leave(chat_id).await;
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

    pub async fn is_joined(&self, chat_id: i64) -> bool {
        let Some(worker) = self.active.lock().await.get(&chat_id).cloned() else {
            return false;
        };
        let (tx, rx) = oneshot::channel();
        if worker.tx.send(Command::IsJoined(tx)).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// Broadcasts a `GroupCallParticipants` update to every tracked chat;
    /// each worker checks whether it's actually theirs before acting on it.
    /// Runs automatically once `Calls` is registered as middleware.
    async fn route_update(&self, u: &tl::types::UpdateGroupCallParticipants) {
        let workers: Vec<_> = self.active.lock().await.values().cloned().collect();
        for worker in workers {
            let _ = worker.tx.send(Command::RouteUpdate(u.clone()));
        }
    }

    /// Same broadcast-and-let-the-worker-check pattern, for a discarded
    /// group call (the voice chat ending for everyone).
    async fn route_call_ended(&self, discarded_id: i64) {
        let workers: Vec<_> = self.active.lock().await.values().cloned().collect();
        for worker in workers {
            let _ = worker.tx.send(Command::RouteCallEnded(discarded_id));
        }
    }
}

impl Middleware for Calls {
    fn call(&self, update: Update, next: Next) -> BoxFuture {
        let calls = self.clone();
        Box::pin(async move {
            if let Update::Raw(raw) = &update {
                match &raw.inner {
                    tl::enums::Update::GroupCallParticipants(u) => {
                        calls.route_update(u).await;
                    }
                    tl::enums::Update::GroupCall(u) => {
                        if let tl::enums::GroupCall::Discarded(d) = &u.call {
                            calls.route_call_ended(d.id).await;
                        }
                    }
                    _ => {}
                }
            }
            next.run(update).await
        })
    }
}
