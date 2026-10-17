//! Persistence shared by explicitly configured farm runtimes.
mod repository;
pub(crate) use repository::{compress_legacy_logs, decode_log, encode_log, persist_logs};
