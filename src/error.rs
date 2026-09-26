use thiserror::Error;

/// Errors that can occur during hook installation or removal.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// No executable memory could be allocated within 2GB of the target function.
    #[error("failed to allocate trampoline within 2GB of target")]
    TrampolineAlloc,

    /// Changing the protection of a page failed.
    #[error("failed to change protection of the page at {address:#x}")]
    ProtectFailed {
        address: usize,
        #[source]
        source: std::io::Error,
    },

    /// Querying the protection of a page failed.
    #[error("failed to query memory at {address:#x}")]
    QueryFailed {
        address: usize,
        #[source]
        source: std::io::Error,
    },

    /// Synchronizing instruction caches after writing code failed.
    #[error("failed to synchronize instruction caches")]
    FlushFailed {
        #[source]
        source: std::io::Error,
    },

    /// The memory to patch is not mapped as readable, executable code.
    #[error("memory at {address:#x} is not readable executable code")]
    NotExecutable { address: usize },

    /// The target's instructions run into unreadable memory before the patch ends.
    #[error("prologue runs into unreadable memory at {address:#x}")]
    PrologueUnreadable { address: usize },

    /// The target function returns or jumps away before the patch ends.
    #[error("target too small to hook: need {need} bytes, found {have}")]
    InsufficientSpace { need: usize, have: usize },

    /// Instruction decoding or relocation failed.
    #[error("instruction relocation failed")]
    RelocationFailed,

    /// The range to patch overlaps a hook this crate has already installed.
    #[error("code at {address:#x} is already hooked")]
    AlreadyHooked { address: usize },

    /// The bytes at the target are not the ones this hook wrote or expected.
    #[error("code at {address:#x} was modified by someone else")]
    PatchModified { address: usize },
}

/// Convenience alias for `std::result::Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
