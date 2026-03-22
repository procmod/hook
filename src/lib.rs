//! Inline function hooking for x86_64.
//!
//! Install detours on live functions, call originals through trampolines,
//! and cleanly restore when done.

#[cfg(target_arch = "x86_64")]
mod alloc;
#[cfg(target_arch = "x86_64")]
mod error;
#[cfg(target_arch = "x86_64")]
mod hook;
#[cfg(target_arch = "x86_64")]
mod jump;
#[cfg(target_arch = "x86_64")]
mod protect;
#[cfg(target_arch = "x86_64")]
mod relocate;

#[cfg(target_arch = "x86_64")]
pub use error::{Error, Result};
#[cfg(target_arch = "x86_64")]
pub use hook::Hook;
