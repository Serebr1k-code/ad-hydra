// These ranges describe scatter/gather spans, rather than collections of indices.
#![allow(clippy::single_range_in_vec_init)]
mod format;
mod framing;
mod http;
pub use format::Format;
pub use framing::{Frame, Framer, MAX_FRAME};
#[cfg(test)]
mod tests;

pub fn validate(config: &hydra_domain::Forwarding) -> anyhow::Result<()> {
    Format::compile(&config.flag_pattern)?;
    Framer::new(config.framing.clone(), false)?;
    Ok(())
}
