// Checks the native ntgcalls behavior that tgcalls' single-source controls
// (set_camera, set_microphone, set_screen) and state sync rely on. Needs
// the real native library but no network or Telegram account.
use ntgcalls::{MediaDescription, NTgCalls, StreamMode};
use tgcalls::Media;

const CHAT: i64 = 4242;

fn none() -> MediaDescription {
    MediaDescription {
        microphone: None,
        speaker: None,
        camera: None,
        screen: None,
    }
}

async fn new_call() -> NTgCalls {
    let ntg = NTgCalls::new();
    ntg.create_call(CHAT).await.expect("create_call");
    ntg
}

#[tokio::test]
async fn fresh_call_has_no_video_and_is_not_muted() {
    let ntg = new_call().await;
    let st = ntg.get_state(CHAT).await.unwrap();
    assert!(!st.muted);
    assert!(st.video_stopped);
    assert!(st.presentation_stopped);
}

#[tokio::test]
async fn camera_source_flips_video_stopped_and_none_removes_it() {
    let ntg = new_call().await;

    let mut media = none();
    media.microphone = Some(Media::external_audio(48000, 1));
    media.camera = Some(Media::external_video(640, 360, 30));
    ntg.set_stream_sources(CHAT, StreamMode::Capture, &media)
        .await
        .unwrap();
    assert!(!ntg.get_state(CHAT).await.unwrap().video_stopped);

    // This is the rule set_camera(None) depends on: a missing device is removed.
    media.camera = None;
    ntg.set_stream_sources(CHAT, StreamMode::Capture, &media)
        .await
        .unwrap();
    assert!(ntg.get_state(CHAT).await.unwrap().video_stopped);
}

#[tokio::test]
async fn screen_source_flips_presentation_stopped() {
    let ntg = new_call().await;

    let mut media = none();
    media.microphone = Some(Media::external_audio(48000, 1));
    media.screen = Some(Media::external_video(640, 360, 30));
    ntg.set_stream_sources(CHAT, StreamMode::Capture, &media)
        .await
        .unwrap();
    let st = ntg.get_state(CHAT).await.unwrap();
    assert!(!st.presentation_stopped);
    assert!(st.video_stopped);
}

#[tokio::test]
async fn mute_and_unmute_toggle_state() {
    let ntg = new_call().await;
    let mut media = none();
    media.microphone = Some(Media::external_audio(48000, 1));
    ntg.set_stream_sources(CHAT, StreamMode::Capture, &media)
        .await
        .unwrap();

    ntg.mute(CHAT).await.unwrap();
    assert!(ntg.get_state(CHAT).await.unwrap().muted);
    ntg.unmute(CHAT).await.unwrap();
    assert!(!ntg.get_state(CHAT).await.unwrap().muted);
}
