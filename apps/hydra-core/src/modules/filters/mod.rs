//! Filter HTTP adapters; shared snapshot transactions are owned by the control aggregate.
mod http;
pub use http::{FilterApi, router};
