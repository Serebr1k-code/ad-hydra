mod control;
mod diagnostics;
mod telemetry;
use crate::state::State;
use std::sync::Arc;
#[derive(Clone)]
pub struct Rpc {
    pub diagnostics: Arc<tokio::sync::Semaphore>,
    pub protocols: Arc<crate::inspection::protocols::Protocols>,
    pub state: Arc<State>,
    pub shutdown: tokio_util::sync::CancellationToken,
}
