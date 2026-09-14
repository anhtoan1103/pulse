//! Notification Service (docs/pulse-architecture.md #2.6): an outbox table
//! filled by event producers, delivered by email from the worker.

pub mod email;
pub mod incident_email;
pub mod service;
