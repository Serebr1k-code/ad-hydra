//! Filter HTTP adapters; shared snapshot transactions are owned by the control aggregate.
mod checker_http;
mod http;
pub use http::{FilterApi, preview_router, router};
