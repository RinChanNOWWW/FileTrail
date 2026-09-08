#![forbid(unsafe_code)]

#[cfg(not(unix))]
compile_error!("Filetrail currently supports macOS and Linux only");

pub mod completion;
pub mod config;
pub mod daemon;
pub mod git;
pub mod lifecycle;
pub mod manifest;
pub mod service;
mod state;
pub mod sync;
