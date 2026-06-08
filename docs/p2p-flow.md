# P2P call flow

Two sides: caller (outgoing) and callee (incoming). Both exchange DH keys via
Telegram before connecting WebRTC directly.

## Outgoing call

1. Fetch DH config: `messages.getDhConfig`.
2. `TgCalls::create_p2p(user_id)`
3. `TgCalls::init_exchange(user_id, dh_config, &[])`. Returns `g_a_hash`.
4. Send `phone.requestCall` with `g_a_hash`.
5. Wait for `updatePhoneCall` (accepted). Extract `g_b` and protocol versions.
6. `TgCalls::exchange_keys(user_id, &g_b, 0)`. Returns `AuthParams`.
7. Send `phone.confirmCall` with `g_a_or_b` and `key_fingerprint`.
   Receive RTC servers from the response.
8. Register `on_signaling_data` and `on_connection_change` callbacks.
9. `TgCalls::connect_p2p(user_id, servers, versions, p2p_allowed)`
10. Run the signaling pump until `ConnectionState::Connected` or failed.
11. Call `set_stream_sources` to start media.

## Incoming call

1. Receive `updatePhoneCall` (requested).
2. Fetch DH config: `messages.getDhConfig`.
3. `TgCalls::create_p2p(user_id)`
4. `TgCalls::init_exchange(user_id, dh_config, &call.g_a_hash)`. Returns `g_b`.
5. Send `phone.acceptCall` with `g_b`.
6. Wait for `updatePhoneCall` (confirmed). Extract `g_a_or_b`, `key_fingerprint`, RTC servers.
7. `TgCalls::exchange_keys(user_id, &g_a_or_b, key_fingerprint)`
8. Register `on_signaling_data` and `on_connection_change` callbacks.
9. `TgCalls::connect_p2p(user_id, servers, versions, p2p_allowed)`
10. Run the signaling pump until `ConnectionState::Connected` or failed.
11. Call `set_stream_sources` to start media.

## Signaling pump

Runs after `connect_p2p`. Must stay alive until `ConnectionState::Connected`.

Outgoing: ntgcalls fires `on_signaling_data` with bytes to send.
Forward them immediately via `phone.sendSignalingData`.

Incoming: `updatePhoneCallSignalingData` arrives from Telegram.
Forward the bytes via `TgCalls::send_signaling_data`.

`Connected` fires only after ICE consent and DTLS are both complete, not just
after channel negotiation. The remote's DTLS fingerprint (InitialSetup) can
arrive after the channel is up. Stopping the pump before that causes an ICE
timeout.

## Hanging up

```
TgCalls::stop(user_id)
phone.discardCall
```
