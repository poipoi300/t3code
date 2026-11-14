//! Native generated text and speech services; does not target wasm32.

pub mod api;
pub mod audio;
pub mod cloud;
pub mod codex;
pub mod dictionary;
pub mod generation;
pub mod host_titles;
pub mod models;
pub mod native;
#[cfg(feature = "native-speech")]
pub mod native_worker;
#[cfg(feature = "orchestration-host")]
pub mod orchestration;
pub mod post_processing;
pub mod prompts;
pub mod provider_runners;
pub mod speech;
pub mod text;
pub mod title_context;

pub use generation::{GenerationError, GenerationRequest, TextGeneration, TextRunner};
pub use tokio_util::sync::CancellationToken;
