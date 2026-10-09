//! Platform-independent core of LUMA.
//!
//! Nothing in this crate touches the OS, the network, or a clock, so all of it
//! is deterministic and unit tested. The Tauri app wires it to real devices.

pub mod action;
pub mod annotation;
pub mod audio;
pub mod command;
pub mod geometry;
pub mod ink;
pub mod lesson;
pub mod markup;
pub mod prompt;
pub mod refine;
pub mod sequencer;
pub mod sse;
pub mod session;
pub mod snap;
pub mod speech;
