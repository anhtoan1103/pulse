//! AI Analysis Service (docs/pulse-ai-design.md):
//! [`context`] prepares sanitized input, [`prompt`] frames it, [`llm`] calls
//! the provider, [`output`] validates the structured reply, and [`service`]
//! drives pending incidents through it.

pub mod context;
pub mod llm;
pub mod output;
pub mod prompt;
pub mod service;
