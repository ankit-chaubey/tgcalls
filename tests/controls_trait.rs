// Compile-time check that every call type offers the same controls through
// `CallControls`. Needs no network or account.
use std::sync::Arc;

use tgcalls::{Call, CallControls, ConferenceCall, P2PCall};

fn assert_controls<T: CallControls>() {}

#[test]
fn every_call_type_implements_call_controls() {
    assert_controls::<Call>();
    assert_controls::<P2PCall>();
    assert_controls::<ConferenceCall>();
    assert_controls::<Arc<ConferenceCall>>();
    assert_controls::<Arc<P2PCall>>();
}

// Generic code written once against the trait compiles for all of them.
#[allow(dead_code)]
async fn hush_and_hide(call: &mut impl CallControls) -> Result<(), tgcalls::TgCallsError> {
    call.mute().await?;
    call.set_video(None).await?;
    call.set_screen(None).await
}
