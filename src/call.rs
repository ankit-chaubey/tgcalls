use std::sync::{Arc, Mutex};

use ntgcalls::{
    ConnectionInfo, ConnectionState, FrameData, MediaDescription, MediaSegmentPartStatus, NTgCalls,
    SsrcGroup, StreamDevice, StreamMode, VideoDescription,
};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use crate::{error::TgCallsError, signaling};

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

pub struct Call {
    client: ferogram::Client,
    chat_id: i64,
    ntg: NTgCalls,
    connect_slot: ConnectSlot,
    state: CallState,
    active_call: Option<ferogram::tl::enums::InputGroupCall>,
    presentation_active: bool,
}

// Telegram supergroup IDs come in as -100xxxxxxxxxx; strip the -100 prefix
// to get the raw channel_id that ntgcalls expects.
fn ntg_chat_id(chat_id: i64) -> i64 {
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
                            let _ = tx.send(Err(ntgcalls::Error::Connection));
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
        }
    }

    pub async fn join(&mut self, media: MediaDescription) -> Result<(), TgCallsError> {
        if self.state != CallState::Idle {
            return Err(TgCallsError::AlreadyJoined);
        }
        self.state = CallState::Joining;

        let call = signaling::resolve_call(&self.client, self.chat_id).await?;
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

        self.ntg
            .set_stream_sources(ntg_id, StreamMode::Capture, &media)
            .await?;

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

        if let Some(call) = self.active_call.take() {
            signaling::leave_call(&self.client, call, 0).await?;
        }

        self.state = CallState::Idle;
        info!("tgcalls: left chat {}", self.chat_id);
        Ok(())
    }

    pub async fn join_presentation(
        &mut self,
        screen: VideoDescription,
    ) -> Result<(), TgCallsError> {
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

        let desc = MediaDescription {
            microphone: None,
            speaker: None,
            camera: None,
            screen: Some(screen),
        };
        self.ntg
            .set_stream_sources(ntg_id, StreamMode::Capture, &desc)
            .await?;

        self.presentation_active = true;
        info!("tgcalls: presentation started in chat {}", self.chat_id);
        Ok(())
    }

    pub async fn stop_presentation(&mut self) -> Result<(), TgCallsError> {
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
        signaling::leave_presentation(&self.client, call).await?;
        self.presentation_active = false;
        info!("tgcalls: presentation stopped in chat {}", self.chat_id);
        Ok(())
    }

    pub fn is_presentation_active(&self) -> bool {
        self.presentation_active
    }

    /// `user_id` is the participant whose video you want to subscribe to.
    pub async fn add_incoming_video(
        &self,
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

    pub async fn remove_incoming_video(&self, endpoint: &str) -> Result<bool, TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .remove_incoming_video(ntg_chat_id(self.chat_id), endpoint)
            .await?)
    }

    pub async fn send_external_frame(
        &self,
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

    pub async fn send_broadcast_timestamp(&self, timestamp: i64) -> Result<(), TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .send_broadcast_timestamp(ntg_chat_id(self.chat_id), timestamp)
            .await?)
    }

    pub async fn send_broadcast_part(
        &self,
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
                frame,
            )
            .await?)
    }

    pub async fn set_stream_sources(
        &self,
        stream_mode: StreamMode,
        media: &MediaDescription,
    ) -> Result<(), TgCallsError> {
        self.require_joined()?;
        self.ntg
            .set_stream_sources(ntg_chat_id(self.chat_id), stream_mode, media)
            .await?;
        Ok(())
    }

    pub async fn pause(&self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        self.ntg.pause(ntg_chat_id(self.chat_id)).await?;
        Ok(())
    }

    pub async fn resume(&self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        self.ntg.resume(ntg_chat_id(self.chat_id)).await?;
        Ok(())
    }

    /// Seconds of the given stream played so far.
    pub async fn played_seconds(&self, stream_mode: StreamMode) -> Result<u64, TgCallsError> {
        self.require_joined()?;
        Ok(self
            .ntg
            .time(ntg_chat_id(self.chat_id), stream_mode)
            .await?)
    }

    pub async fn mute(&self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        let call = self
            .active_call
            .as_ref()
            .ok_or(TgCallsError::NotJoined)?
            .clone();
        let _ = self.ntg.mute(ntg_chat_id(self.chat_id)).await;
        signaling::set_muted(&self.client, call, true).await
    }

    pub async fn unmute(&self) -> Result<(), TgCallsError> {
        self.require_joined()?;
        let call = self
            .active_call
            .as_ref()
            .ok_or(TgCallsError::NotJoined)?
            .clone();
        let _ = self.ntg.unmute(ntg_chat_id(self.chat_id)).await;
        signaling::set_muted(&self.client, call, false).await
    }

    pub fn state(&self) -> CallState {
        self.state
    }

    pub fn is_joined(&self) -> bool {
        self.state == CallState::Joined
    }

    pub async fn media_state(&self) -> Result<ntgcalls::MediaState, TgCallsError> {
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
