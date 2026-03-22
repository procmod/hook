use thiserror::Error;

/// Errors that can occur during hook installation or removal.
#[derive(Debug, Error)]
pub enum Error {
    /// Failed to allocate executable memory within 2GB of the target function.
    #[error("failed to allocate trampoline within 2GB of target")]
    TrampolineAlloc,

    /// Failed to change memory protection on the target function's page.
    #[error("failed to change memory protection")]
    ProtectFailed,

    /// The target function is too small to hook.
    #[error("target too small to hook: need {need} bytes, found {have}")]
    InsufficientSpace { need: usize, have: usize },

    /// Instruction decoding or relocation failed.
    #[error("instruction relocation failed")]
    RelocationFailed,

    /// Attempted to unhook a hook that is not currently installed.
    #[error("hook not installed")]
    NotInstalled,
}

/// Convenience alias for `std::result::Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
