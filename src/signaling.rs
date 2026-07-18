use crate::error::TgCallsError;
use ferogram::{tl, Client};
use std::time::{SystemTime, UNIX_EPOCH};

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

    extract_created_call(updates).ok_or_else(|| {
        TgCallsError::TransportParse("no GroupCall in createGroupCall response".into())
    })
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

pub async fn set_muted(
    client: &Client,
    call: tl::enums::InputGroupCall,
    muted: bool,
) -> Result<(), TgCallsError> {
    client
        .invoke(&tl::functions::phone::EditGroupCallParticipant {
            call,
            participant: tl::enums::InputPeer::PeerSelf,
            muted: Some(muted),
            volume: None,
            raise_hand: None,
            video_stopped: None,
            video_paused: None,
            presentation_paused: None,
        })
        .await?;
    Ok(())
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

#[allow(dead_code)]
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

fn extract_created_call(updates: tl::enums::Updates) -> Option<tl::enums::InputGroupCall> {
    let list = match updates {
        tl::enums::Updates::Updates(u) => u.updates,
        tl::enums::Updates::Combined(u) => u.updates,
        _ => return None,
    };
    for upd in list {
        if let tl::enums::Update::GroupCall(u) = upd {
            if let tl::enums::GroupCall::GroupCall(gc) = u.call {
                return Some(tl::enums::InputGroupCall::InputGroupCall(
                    tl::types::InputGroupCall {
                        id: gc.id,
                        access_hash: gc.access_hash,
                    },
                ));
            }
        }
    }
    None
}
