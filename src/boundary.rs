//! Compile-time checks on hook ownership.
//!
//! ```
//! fn check(hook: procmod_hook::Hook) {
//!     if let Err(error) = hook.unhook() {
//!         let hook = error.into_hook();
//!         std::mem::forget(hook);
//!     }
//! }
//! ```
//!
//! Removal consumes the hook, so it cannot be removed twice or used afterwards:
//!
//! ```compile_fail,E0382
//! fn check(hook: procmod_hook::Hook) {
//!     let _ = hook.unhook();
//!     let _ = hook.trampoline();
//! }
//! ```
//!
//! Installation is unsafe:
//!
//! ```compile_fail,E0133
//! extern "C" fn target() {}
//! extern "C" fn detour() {}
//!
//! let _ = procmod_hook::Hook::install(target as *const u8, detour as *const u8);
//! ```
