use thiserror::Error;

#[derive(Debug, Error)]
pub enum TgCallsError {
    #[error("ferogram error: {0}")]
    Ferogram(#[from] ferogram::InvocationError),

    #[error("ntgcalls error: {0}")]
    NtgCalls(#[from] ntgcalls::Error),

    #[error("not joined, call join() first")]
    NotJoined,

    #[error("already joined a call in this chat")]
    AlreadyJoined,

    #[error("no active group call in this chat")]
    NoActiveGroupCall,

    #[error("failed to parse Telegram transport response: {0}")]
    TransportParse(String),

    #[error("P2P call already active for this user")]
    P2PAlreadyActive,

    #[error("no active P2P call for this user")]
    P2PNotActive,

    #[error("this chat's call worker thread is gone (panicked or already torn down)")]
    WorkerGone,

    #[error("too many concurrent calls active (limit: {0}) - leave one before joining another")]
    TooManyConcurrentCalls(usize),
}
