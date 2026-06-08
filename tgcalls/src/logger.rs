use std::ffi::CStr;
use std::sync::Once;

use ntgcalls::{
    NTG_LOG_DEBUG, NTG_LOG_ERROR, NTG_LOG_INFO, NTG_LOG_WARNING, NTG_LOG_WEBRTC,
    ntg_log_message_struct,
};

static LOGGER_INIT: Once = Once::new();

/// Register the ntgcalls C logger once. Subsequent calls are no-ops.
pub(crate) fn init() {
    LOGGER_INIT.call_once(|| {
        unsafe { ntgcalls::ntg_register_logger(Some(log_callback)) };
    });
}

unsafe extern "C" fn log_callback(msg: ntg_log_message_struct) {
    let message = if msg.message.is_null() {
        return;
    } else {
        unsafe { CStr::from_ptr(msg.message) }.to_string_lossy()
    };

    let file = if msg.file.is_null() {
        "?".into()
    } else {
        unsafe { CStr::from_ptr(msg.file) }.to_string_lossy()
    };

    let source = if msg.source == NTG_LOG_WEBRTC {
        "webrtc"
    } else {
        "ntgcalls"
    };
    let line = msg.line;

    match msg.level {
        NTG_LOG_DEBUG => {
            tracing::debug!(target: "ntgcalls", source, file = %file, line, "{}", message)
        }
        NTG_LOG_INFO => {
            tracing::info! (target: "ntgcalls", source, file = %file, line, "{}", message)
        }
        NTG_LOG_WARNING => {
            tracing::warn! (target: "ntgcalls", source, file = %file, line, "{}", message)
        }
        NTG_LOG_ERROR => {
            tracing::error!(target: "ntgcalls", source, file = %file, line, "{}", message)
        }
        _ => tracing::debug!(target: "ntgcalls", source, file = %file, line, "{}", message),
    }
}
