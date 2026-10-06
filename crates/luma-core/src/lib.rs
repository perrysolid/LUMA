//! Platform-independent core of LUMA.
//!
//! Nothing in this crate touches the OS, the network, or a clock, so all of it
//! is deterministic and unit tested. The Tauri app wires it to real devices.

pub mod annotation;
pub mod audio;
pub mod geometry;
pub mod markup;
pub mod prompt;
pub mod sequencer;
pub mod sse;
pub mod session;
pub mod speech;
