mod catalog;
mod catalog_http;
mod completion;
mod effectiveness_dto;
mod effectiveness_http;
mod effectiveness_repository;
mod flag_counts;
mod packages;
pub mod process_results;
mod run_labels;
pub(crate) use packages::validate as validate_python_packages;
mod retention;
pub(crate) mod runtimes;
pub use catalog::{FarmService, FarmTeam};
mod http;
mod scheduler;
mod service;
pub use http::{FarmApi, router};
pub use service::*;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
mod effectiveness_tests;
