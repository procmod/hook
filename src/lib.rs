//! Inline function hooking for x86_64.
//!
//! Install detours on live functions, call originals through trampolines,
//! and cleanly restore when done.

#[cfg(target_arch = "x86_64")]
mod alloc;
#[cfg(all(doctest, target_arch = "x86_64"))]
mod boundary;
#[cfg(target_arch = "x86_64")]
mod error;
#[cfg(target_arch = "x86_64")]
mod hook;
#[cfg(target_arch = "x86_64")]
mod jump;
#[cfg(target_arch = "x86_64")]
mod memory;
#[cfg(target_arch = "x86_64")]
mod patch;
#[cfg(target_arch = "x86_64")]
mod trampoline;

#[cfg(target_arch = "x86_64")]
pub use error::{Error, Result};
#[cfg(target_arch = "x86_64")]
pub use hook::{Hook, UnhookError};
