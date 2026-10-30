use crate::error::TgCallsError;
use ferogram::{tl, Client};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

async fn resolve_channel(client: &Client, chat_id: i64) -> Result<(i64, i64), TgCallsError> {
    let channel_id = if chat_id < -1_000_000_000_000 {
        (-chat_id) - 1_000_000_000_000
    } else {
        -chat_id
    };

    let chats = client
        .invoke(&tl::functions::channels::GetChannels {
            id: vec![tl::enums::InputChannel::InputChannel(
                tl::types::InputChannel {
                    channel_id,
                    access_hash: 0,
                },
            )],
        })
        .await?;

    let access_hash = match chats {
        tl::enums::messages::Chats::Chats(c) => extract_access_hash(c.chats)?,
        tl::enums::messages::Chats::Slice(c) => extract_access_hash(c.chats)?,
    };

    Ok((channel_id, access_hash))
}

pub async fn resolve_call(
    client: &Client,
    chat_id: i64,
) -> Result<tl::enums::InputGroupCall, TgCallsError> {
    let (channel_id, access_hash) = resolve_channel(client, chat_id).await?;

    let full = client
        .invoke(&tl::functions::channels::GetFullChannel {
            channel: tl::enums::InputChannel::InputChannel(tl::types::InputChannel {
                channel_id,
                access_hash,
            }),
        })
        .await?;

    let full_chat = match full {
        tl::enums::messages::ChatFull::ChatFull(f) => f.full_chat,
    };

    match full_chat {
        tl::enums::ChatFull::ChannelFull(cf) => cf.call.ok_or(TgCallsError::NoActiveGroupCall),
        _ => Err(TgCallsError::NoActiveGroupCall),
    }
}

/// Starts a new voice chat in `chat_id`. Fails if one is already active -
/// use `resolve_call` first (or `resolve_or_create_call`) if that's fine.
pub async fn create_group_call(
    client: &Client,
    chat_id: i64,
) -> Result<tl::enums::InputGroupCall, TgCallsError> {
    let (channel_id, access_hash) = resolve_channel(client, chat_id).await?;

    // Telegram just needs this for de-duplicating retried requests; a
    // time-based value is unique enough without pulling in a `rand` dep.
    let random_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as i32)
        .unwrap_or(0);

    let updates = client
        .invoke(&tl::functions::phone::CreateGroupCall {
            rtmp_stream: false,
            peer: tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
                channel_id,
                access_hash,
            }),
            random_id,
            title: None,
            schedule_date: None,
        })
        .await?;

    let (call, _invite_link) = extract_created_call(updates).ok_or_else(|| {
        TgCallsError::TransportParse("no GroupCall in createGroupCall response".into())
    })?;
    Ok(call)
}

/// `resolve_call`, but starts a voice chat first if none is active yet.
pub async fn resolve_or_create_call(
    client: &Client,
    chat_id: i64,
) -> Result<tl::enums::InputGroupCall, TgCallsError> {
    match resolve_call(client, chat_id).await {
        Err(TgCallsError::NoActiveGroupCall) => create_group_call(client, chat_id).await,
        other => other,
    }
}

pub async fn join_call(
    client: &Client,
    call: tl::enums::InputGroupCall,
    params_json: &str,
) -> Result<String, TgCallsError> {
    let req = tl::functions::phone::JoinGroupCall {
        muted: false,
        video_stopped: false,
        call,
        join_as: tl::enums::InputPeer::PeerSelf,
        invite_hash: None,
        public_key: None,
        block: None,
        params: tl::enums::DataJson::DataJson(tl::types::DataJson {
            data: params_json.to_string(),
        }),
    };
    let updates = client.invoke(&req).await?;
    extract_transport_json(updates, false)
        .ok_or_else(|| TgCallsError::TransportParse("no GroupCallConnection in response".into()))
}

pub async fn join_presentation(
    client: &Client,
    call: tl::enums::InputGroupCall,
    params_json: &str,
) -> Result<String, TgCallsError> {
    let req = tl::functions::phone::JoinGroupCallPresentation {
        call,
        params: tl::enums::DataJson::DataJson(tl::types::DataJson {
            data: params_json.to_string(),
        }),
    };
    let updates = client.invoke(&req).await?;
    extract_transport_json(updates, true).ok_or_else(|| {
        TgCallsError::TransportParse("no GroupCallConnection(presentation) in response".into())
    })
}

pub async fn leave_presentation(
    client: &Client,
    call: tl::enums::InputGroupCall,
) -> Result<(), TgCallsError> {
    client
        .invoke(&tl::functions::phone::LeaveGroupCallPresentation { call })
        .await?;
    Ok(())
}

pub async fn leave_call(
    client: &Client,
    call: tl::enums::InputGroupCall,
    source_ssrc: i32,
) -> Result<(), TgCallsError> {
    client
        .invoke(&tl::functions::phone::LeaveGroupCall {
            call,
            source: source_ssrc,
        })
        .await?;
    Ok(())
}

/// Pushes this side's media state (mute, camera, screen share, pauses) to
/// Telegram so other participants see what is really being sent.
///
/// Two things feed it: ntgcalls' state-change callback and every control
/// call (`mute`, `set_camera`, `play`, ...) directly. The callback does not
/// reliably fire for every change on every connection, so the direct push
/// is what guarantees Telegram hears about it. Both paths go through here,
/// which serializes the requests so they reach Telegram in order, and
/// skips a push when the flags are the same as the last successful one.
///
/// Video and presentation pause flags are only sent while that source
/// exists, since Telegram rejects pausing something that isn't being sent.
#[derive(Clone, Default)]
pub struct MediaStateSync {
    last: Arc<Mutex<Option<ParticipantFlags>>>,
}

impl MediaStateSync {
    pub async fn push(
        &self,
        client: &Client,
        call: tl::enums::InputGroupCall,
        state: &ntgcalls::MediaState,
    ) -> Result<(), TgCallsError> {
        let flags = participant_flags(state);
        // Held across the request on purpose: a second push waits for the
        // first, then sees its flags and skips if nothing changed.
        let mut last = self.last.lock().await;
        if last.as_ref() == Some(&flags) {
            return Ok(());
        }
        client
            .invoke(&tl::functions::phone::EditGroupCallParticipant {
                call,
                participant: tl::enums::InputPeer::PeerSelf,
                muted: Some(flags.muted),
                volume: None,
                raise_hand: None,
                video_stopped: Some(flags.video_stopped),
                video_paused: flags.video_paused,
                presentation_paused: flags.presentation_paused,
            })
            .await?;
        *last = Some(flags);
        Ok(())
    }

    /// Reads ntgcalls' current state for `chat_id` and pushes it.
    pub async fn refresh(
        &self,
        client: &Client,
        call: tl::enums::InputGroupCall,
        ntg: &ntgcalls::NTgCalls,
        chat_id: i64,
    ) -> Result<(), TgCallsError> {
        let state = ntg.get_state(chat_id).await?;
        self.push(client, call, &state).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParticipantFlags {
    muted: bool,
    video_stopped: bool,
    video_paused: Option<bool>,
    presentation_paused: Option<bool>,
}

fn participant_flags(state: &ntgcalls::MediaState) -> ParticipantFlags {
    ParticipantFlags {
        muted: state.muted,
        video_stopped: state.video_stopped,
        video_paused: (!state.video_stopped).then_some(state.video_paused),
        presentation_paused: (!state.presentation_stopped).then_some(state.presentation_paused),
    }
}

/// Sets how loud `user_id` sounds to *you* in this call (0 = muted for you,
/// 10000 = 100%, up to 20000 = 200%). This is a per-listener preference
/// synced to your account, not a broadcast-wide volume control.
pub async fn set_volume(
    client: &Client,
    call: tl::enums::InputGroupCall,
    user_id: i64,
    access_hash: i64,
    volume: i32,
) -> Result<(), TgCallsError> {
    client
        .invoke(&tl::functions::phone::EditGroupCallParticipant {
            call,
            participant: tl::enums::InputPeer::User(tl::types::InputPeerUser {
                user_id,
                access_hash,
            }),
            muted: None,
            volume: Some(volume.clamp(0, 20000)),
            raise_hand: None,
            video_stopped: None,
            video_paused: None,
            presentation_paused: None,
        })
        .await?;
    Ok(())
}

pub async fn get_participants(
    client: &Client,
    call: tl::enums::InputGroupCall,
) -> Result<Vec<tl::types::GroupCallParticipant>, TgCallsError> {
    let mut out = Vec::new();
    let mut offset = String::new();
    loop {
        let resp = client
            .invoke(&tl::functions::phone::GetGroupParticipants {
                call: call.clone(),
                ids: vec![],
                sources: vec![],
                offset: offset.clone(),
                limit: 100,
            })
            .await?;
        let tl::enums::phone::GroupParticipants::GroupParticipants(page) = resp;
        let page_len = page.participants.len();
        for p in page.participants {
            let tl::enums::GroupCallParticipant::GroupCallParticipant(p) = p;
            out.push(p);
        }
        if page_len < 100 || page.next_offset.is_empty() {
            break;
        }
        offset = page.next_offset;
    }
    Ok(out)
}

pub async fn get_dh_config(client: &Client) -> Result<tl::types::messages::DhConfig, TgCallsError> {
    match client
        .invoke(&tl::functions::messages::GetDhConfig {
            version: 0,
            random_length: 256,
        })
        .await?
    {
        tl::enums::messages::DhConfig::DhConfig(c) => Ok(c),
        tl::enums::messages::DhConfig::NotModified(_) => Err(TgCallsError::TransportParse(
            "DhConfig not available".into(),
        )),
    }
}

pub async fn get_user_access_hash(client: &Client, user_id: i64) -> Result<i64, TgCallsError> {
    let users = client
        .invoke(&tl::functions::users::GetUsers {
            id: vec![tl::enums::InputUser::InputUser(tl::types::InputUser {
                user_id,
                access_hash: 0,
            })],
        })
        .await?;
    for u in users {
        if let tl::enums::User::User(user) = u {
            if user.id == user_id {
                return Ok(user.access_hash.unwrap_or(0));
            }
        }
    }
    Err(TgCallsError::TransportParse(format!(
        "user {} not found",
        user_id
    )))
}

pub async fn get_self_user_id(client: &Client) -> Result<i64, TgCallsError> {
    let users = client
        .invoke(&tl::functions::users::GetUsers {
            id: vec![tl::enums::InputUser::UserSelf],
        })
        .await?;
    for u in users {
        if let tl::enums::User::User(user) = u {
            return Ok(user.id);
        }
    }
    Err(TgCallsError::TransportParse(
        "could not resolve self user id".into(),
    ))
}

/// Falls back to hardcoded defaults if get_protocol() errors.
pub fn build_protocol() -> Result<tl::enums::PhoneCallProtocol, TgCallsError> {
    let proto = ntgcalls::NTgCalls::get_protocol().unwrap_or_else(|_| ntgcalls::Protocol {
        min_layer: 92,
        max_layer: 92,
        udp_p2p: true,
        udp_reflector: true,
        library_versions: vec!["8.0.0".into(), "9.0.0".into()],
    });
    Ok(tl::enums::PhoneCallProtocol::PhoneCallProtocol(
        tl::types::PhoneCallProtocol {
            udp_p2p: proto.udp_p2p,
            udp_reflector: proto.udp_reflector,
            min_layer: proto.min_layer,
            max_layer: proto.max_layer,
            library_versions: proto.library_versions,
        },
    ))
}

pub fn input_phone_call(id: i64, access_hash: i64) -> tl::enums::InputPhoneCall {
    tl::enums::InputPhoneCall::InputPhoneCall(tl::types::InputPhoneCall { id, access_hash })
}

pub async fn send_p2p_signaling(
    client: &Client,
    peer: tl::enums::InputPhoneCall,
    data: &[u8],
) -> Result<(), TgCallsError> {
    client
        .invoke(&tl::functions::phone::SendSignalingData {
            peer,
            data: data.to_vec(),
        })
        .await?;
    Ok(())
}

pub async fn discard_call(
    client: &Client,
    peer: tl::enums::InputPhoneCall,
    duration: i32,
    video: bool,
) -> Result<(), TgCallsError> {
    client
        .invoke(&tl::functions::phone::DiscardCall {
            video,
            peer,
            duration,
            reason: tl::enums::PhoneCallDiscardReason::Hangup,
            connection_id: 0,
        })
        .await?;
    Ok(())
}

/// Extracts the slug from a conference deep link
/// (`t.me/call/<slug>`/`https://t.me/call/<slug>` or `tg://call?slug=<slug>`,
/// per <https://core.telegram.org/api/links#conference-links>). Returns
/// `None` for anything else - not a validity check beyond shape, the slug
/// itself is only proven valid once you actually join with it.
pub fn parse_conference_link(link: &str) -> Option<String> {
    let link = link.trim();
    if let Some(rest) = link
        .strip_prefix("tg://call?")
        .or_else(|| link.strip_prefix("tg:call?"))
    {
        for pair in rest.split('&') {
            if let Some(slug) = pair.strip_prefix("slug=") {
                return Some(slug.to_string());
            }
        }
        return None;
    }
    let without_scheme = link
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    for host in ["t.me/call/", "telegram.me/call/", "telegram.dog/call/"] {
        if let Some(rest) = without_scheme.strip_prefix(host) {
            let slug = rest.split(['?', '#']).next().unwrap_or(rest);
            if !slug.is_empty() {
                return Some(slug.to_string());
            }
        }
    }
    None
}

/// Starts a brand new E2E conference call. Returns the call handle, the
/// join transport JSON, and the conference's invite link
/// (`t.me/call/<slug>`) if the server sent one back - `groupCall.invite_link`
/// comes for free with creation, no separate `exportGroupCallInvite` call
/// needed for conferences (unlike classic video chats/livestreams, where
/// that's the only way to get a link).
pub async fn create_conference_call(
    client: &Client,
    payload: &str,
    video_stopped: bool,
    block: Vec<u8>,
    public_key: [u8; 32],
) -> Result<(tl::enums::InputGroupCall, String, Option<String>), TgCallsError> {
    let random_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as i32)
        .unwrap_or(0);

    let updates = client
        .invoke(&tl::functions::phone::CreateConferenceCall {
            muted: false,
            video_stopped,
            join: true,
            random_id,
            public_key: Some(public_key),
            block: Some(block),
            params: Some(tl::enums::DataJson::DataJson(tl::types::DataJson {
                data: payload.to_string(),
            })),
        })
        .await?;

    let (call, invite_link) = extract_created_call(updates.clone()).ok_or_else(|| {
        TgCallsError::TransportParse("no GroupCall in createConferenceCall response".into())
    })?;
    let transport = extract_transport_json(updates, false).ok_or_else(|| {
        TgCallsError::TransportParse(
            "no GroupCallConnection in createConferenceCall response".into(),
        )
    })?;
    Ok((call, transport, invite_link))
}

/// Rings `user_id` into an already-created conference (the "invite" step -
/// without this the conference exists but nobody else is being called).
pub async fn invite_conference_call_participant(
    client: &Client,
    call: tl::enums::InputGroupCall,
    user_id: i64,
    access_hash: i64,
    video: bool,
) -> Result<(), TgCallsError> {
    client
        .invoke(&tl::functions::phone::InviteConferenceCallParticipant {
            video,
            call,
            user_id: tl::enums::InputUser::InputUser(tl::types::InputUser {
                user_id,
                access_hash,
            }),
        })
        .await?;
    Ok(())
}

/// Joins (or rejoins - e.g. after a P2P->conference migration) an existing
/// conference given the block/public_key from `init_conference`. Returns
/// the transport JSON plus any chain blocks Telegram piggy-backed on the
/// join response itself (common when you're behind on the chain).
pub async fn join_conference_call(
    client: &Client,
    call: tl::enums::InputGroupCall,
    payload: &str,
    video_stopped: bool,
    block: Vec<u8>,
    public_key: [u8; 32],
) -> Result<(String, Option<tl::types::UpdateGroupCallChainBlocks>), TgCallsError> {
    let updates = client
        .invoke(&tl::functions::phone::JoinGroupCall {
            muted: false,
            video_stopped,
            call,
            join_as: tl::enums::InputPeer::PeerSelf,
            invite_hash: None,
            public_key: Some(public_key),
            block: Some(block),
            params: tl::enums::DataJson::DataJson(tl::types::DataJson {
                data: payload.to_string(),
            }),
        })
        .await?;

    let chain_blocks = extract_chain_blocks(&updates);
    let transport = extract_transport_json(updates, false).ok_or_else(|| {
        TgCallsError::TransportParse(
            "no GroupCallConnection in joinGroupCall(conference) response".into(),
        )
    })?;
    Ok((transport, chain_blocks))
}

/// Relays a block you produced (`NTgCalls::on_outbound_block`) to every
/// other participant via the server.
pub async fn send_conference_call_broadcast(
    client: &Client,
    call: tl::enums::InputGroupCall,
    block: Vec<u8>,
) -> Result<(), TgCallsError> {
    client
        .invoke(&tl::functions::phone::SendConferenceCallBroadcast { call, block })
        .await?;
    Ok(())
}

/// Fetches a range of blocks for one subchain - used both to answer
/// `NTgCalls::on_subchain_request` (catching up after a short poll) and to
/// fetch just the last block for a P2P->conference migration
/// (`sub_chain_id: 0, offset: -1, limit: 1`).
pub async fn get_conference_chain_blocks(
    client: &Client,
    call: tl::enums::InputGroupCall,
    sub_chain_id: i32,
    offset: i32,
    limit: i32,
) -> Result<Option<tl::types::UpdateGroupCallChainBlocks>, TgCallsError> {
    let updates = client
        .invoke(&tl::functions::phone::GetGroupCallChainBlocks {
            call,
            sub_chain_id,
            offset,
            limit,
        })
        .await?;
    Ok(extract_chain_blocks(&updates))
}

fn extract_chain_blocks(
    updates: &tl::enums::Updates,
) -> Option<tl::types::UpdateGroupCallChainBlocks> {
    let list: &[tl::enums::Update] = match updates {
        tl::enums::Updates::Updates(u) => &u.updates,
        tl::enums::Updates::Combined(u) => &u.updates,
        _ => return None,
    };
    for upd in list {
        if let tl::enums::Update::GroupCallChainBlocks(cb) = upd {
            return Some(cb.clone());
        }
    }
    None
}

fn extract_access_hash(chats: Vec<tl::enums::Chat>) -> Result<i64, TgCallsError> {
    for chat in chats {
        if let tl::enums::Chat::Channel(ch) = chat {
            return Ok(ch.access_hash.unwrap_or(0));
        }
    }
    Err(TgCallsError::NoActiveGroupCall)
}

fn extract_transport_json(updates: tl::enums::Updates, presentation: bool) -> Option<String> {
    let list = match updates {
        tl::enums::Updates::Updates(u) => u.updates,
        tl::enums::Updates::Combined(u) => u.updates,
        _ => return None,
    };
    for upd in list {
        if let tl::enums::Update::GroupCallConnection(gc) = upd {
            if gc.presentation == presentation {
                let tl::enums::DataJson::DataJson(d) = gc.params;
                return Some(d.data);
            }
        }
    }
    None
}

fn extract_created_call(
    updates: tl::enums::Updates,
) -> Option<(tl::enums::InputGroupCall, Option<String>)> {
    let list = match updates {
        tl::enums::Updates::Updates(u) => u.updates,
        tl::enums::Updates::Combined(u) => u.updates,
        _ => return None,
    };
    for upd in list {
        if let tl::enums::Update::GroupCall(u) = upd {
            if let tl::enums::GroupCall::GroupCall(gc) = u.call {
                let call = tl::enums::InputGroupCall::InputGroupCall(tl::types::InputGroupCall {
                    id: gc.id,
                    access_hash: gc.access_hash,
                });
                return Some((call, gc.invite_link));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(
        muted: bool,
        video_paused: bool,
        video_stopped: bool,
        presentation_paused: bool,
        presentation_stopped: bool,
    ) -> ntgcalls::MediaState {
        ntgcalls::MediaState {
            muted,
            video_paused,
            video_stopped,
            presentation_paused,
            presentation_stopped,
        }
    }

    #[test]
    fn audio_only_reports_video_stopped_and_no_pause_flags() {
        let f = participant_flags(&state(false, false, true, false, true));
        assert_eq!(
            f,
            ParticipantFlags {
                muted: false,
                video_stopped: true,
                video_paused: None,
                presentation_paused: None,
            }
        );
    }

    #[test]
    fn same_state_gives_equal_flags_so_the_push_is_skipped() {
        let a = participant_flags(&state(false, false, false, false, true));
        let b = participant_flags(&state(false, false, false, false, true));
        assert_eq!(a, b);
    }

    #[test]
    fn every_control_changes_the_flags_so_it_is_pushed() {
        let base = participant_flags(&state(false, false, false, false, true));
        let muted = participant_flags(&state(true, false, false, false, true));
        let paused = participant_flags(&state(false, true, false, false, true));
        let no_camera = participant_flags(&state(false, false, true, false, true));
        let sharing = participant_flags(&state(false, false, false, false, false));
        for changed in [muted, paused, no_camera, sharing] {
            assert_ne!(base, changed);
        }
    }

    #[test]
    fn camera_on_reports_pause_state() {
        let f = participant_flags(&state(false, true, false, false, true));
        assert!(!f.video_stopped);
        assert_eq!(f.video_paused, Some(true));
        assert_eq!(f.presentation_paused, None);
    }

    #[test]
    fn screen_share_reports_presentation_pause_state() {
        let f = participant_flags(&state(true, false, true, true, false));
        assert!(f.muted);
        assert_eq!(f.video_paused, None);
        assert_eq!(f.presentation_paused, Some(true));
    }
}
