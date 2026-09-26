use std::ops::Range;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::alloc::{self, Block};
use crate::error::{Error, Result};
use crate::jump;
use crate::memory::{self, Memory};
use crate::patch;
use crate::trampoline::{self, Site};

// ranges patched by live hooks. the lock also serializes every protection change
// this crate makes, so hooks sharing a page cannot race
static INSTALLED: Mutex<Vec<Range<usize>>> = Mutex::new(Vec::new());

fn installed() -> MutexGuard<'static, Vec<Range<usize>>> {
    INSTALLED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An installed inline hook that redirects a target function to a detour.
///
/// The hook overwrites the first instructions of the target function with a jump
/// to the detour. Those instructions are relocated into a nearby trampoline that
/// ends with a jump back into the target, so the detour can call the original.
///
/// Removing the hook, with [`unhook`](Hook::unhook) or by dropping it, restores the
/// original bytes and frees the trampoline. If removal fails when the hook is
/// dropped, the hook stays installed and its trampoline is leaked so that the
/// detour can keep calling it. To keep a hook installed for the rest of the
/// process, pass it to [`std::mem::forget`].
pub struct Hook {
    target: usize,
    trampoline: Block,
    original: Vec<u8>,
    patch: Vec<u8>,
    memory: &'static Memory,
    installed: bool,
}

// SAFETY: the hook owns its trampoline and patch state outright, and every
// mutation of shared code or protections happens under the INSTALLED lock
unsafe impl Send for Hook {}
unsafe impl Sync for Hook {}

struct Plan {
    target: usize,
    jump: Vec<u8>,
    prologue: Vec<u8>,
}

impl Hook {
    /// Install an inline hook at `target`, redirecting calls to `detour`.
    ///
    /// Returns a `Hook` whose [`trampoline`](Hook::trampoline) calls the original
    /// function. The patch covers the whole instructions that start within the
    /// first 5 bytes of `target`, or the first 14 bytes when `detour` is more than
    /// 2GB away.
    ///
    /// # Safety
    ///
    /// The caller takes on these obligations for the whole life of the hook,
    /// including its removal by [`unhook`](Hook::unhook) or drop:
    ///
    /// - `target` must point to the first instruction of a function, and that code
    ///   must stay mapped while the hook exists.
    /// - `detour` must be a function with the same signature and calling
    ///   convention as the target, and must stay valid while the hook exists.
    /// - No code may branch into the patched instructions other than to `target`
    ///   itself, because those instructions are moved into the trampoline.
    /// - During installation and removal, no thread may be executing, or about to
    ///   execute, an instruction that starts within the first 14 bytes of
    ///   `target`. procmod-hook does not suspend threads; establish this with your
    ///   own synchronization.
    /// - The trampoline may only be called while the hook exists. When the hook is
    ///   removed, no thread may be executing the trampoline or be about to call
    ///   it, including a detour that has not yet returned from it.
    /// - No code outside this crate may modify the patched bytes, or change the
    ///   protection of their pages, during installation and removal.
    pub unsafe fn install(target: *const u8, detour: *const u8) -> Result<Self> {
        Self::install_in(&memory::SYSTEM, target as usize, detour as usize)
    }

    unsafe fn install_in(memory: &'static Memory, target: usize, detour: usize) -> Result<Self> {
        let jump = jump::encode(target as u64, detour as u64);
        let mut hooks = installed();
        reject_overlap(&hooks, target..target + jump.len())?;
        let prologue = read_prologue(memory, target, trampoline::max_stolen_len(jump.len()))?;
        let plan = Plan {
            target,
            jump,
            prologue,
        };
        let block = alloc::alloc_near(target)?;
        let patch = match apply(memory, &plan, &block, &hooks) {
            Ok(patch) => patch,
            Err(error) => {
                alloc::free(&block);
                return Err(error);
            }
        };
        hooks.push(target..target + patch.len());
        Ok(Self {
            target,
            trampoline: block,
            original: plan.prologue[..patch.len()].to_vec(),
            patch,
            memory,
            installed: true,
        })
    }

    /// Returns a pointer to the trampoline that calls the original function.
    ///
    /// Transmute this to the original function's type to call it:
    ///
    /// ```ignore
    /// let original: extern "C" fn(i32) -> i32 = std::mem::transmute(hook.trampoline());
    /// ```
    pub fn trampoline(&self) -> *const u8 {
        self.trampoline.ptr
    }

    /// Remove the hook, restoring the original function bytes.
    ///
    /// Either the original bytes and page protections are fully restored and the
    /// trampoline is freed, or the hook stays installed and is returned inside the
    /// error so the removal can be retried.
    pub fn unhook(mut self) -> std::result::Result<(), UnhookError> {
        match self.remove() {
            Ok(()) => Ok(()),
            Err(error) => Err(UnhookError { hook: self, error }),
        }
    }

    fn remove(&mut self) -> Result<()> {
        let mut hooks = installed();
        // SAFETY: install's contract makes removal the caller's responsibility at
        // any point in the hook's life, including quiescence of target and trampoline
        unsafe {
            patch::replace(self.memory, self.target, &self.patch, &self.original)?;
            alloc::free(&self.trampoline);
        }
        hooks.retain(|range| range.start != self.target);
        self.installed = false;
        Ok(())
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        if self.installed {
            self.remove().ok();
        }
    }
}

impl std::fmt::Debug for Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hook")
            .field("target", &(self.target as *const u8))
            .field("trampoline", &self.trampoline.ptr)
            .field("patch_len", &self.patch.len())
            .finish()
    }
}

/// A failed [`Hook::unhook`]. The hook is still installed and can be recovered.
#[derive(Debug)]
pub struct UnhookError {
    hook: Hook,
    error: Error,
}

impl UnhookError {
    /// The reason removal failed.
    pub fn error(&self) -> &Error {
        &self.error
    }

    /// Take back the still-installed hook, for example to retry removal.
    pub fn into_hook(self) -> Hook {
        self.hook
    }
}

impl std::fmt::Display for UnhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to remove hook: {}", self.error)
    }
}

impl std::error::Error for UnhookError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

unsafe fn apply(
    memory: &Memory,
    plan: &Plan,
    block: &Block,
    hooks: &[Range<usize>],
) -> Result<Vec<u8>> {
    let site = Site {
        target: plan.target as u64,
        trampoline: block.ptr as u64,
        capacity: block.len,
        patch_len: plan.jump.len(),
    };
    let layout = trampoline::layout(&plan.prologue, &site)?;
    reject_overlap(hooks, plan.target..plan.target + layout.stolen_len)?;
    alloc::seal(block, &layout.code)?;
    let mut patch = plan.jump.clone();
    patch.resize(layout.stolen_len, 0x90);
    patch::replace(
        memory,
        plan.target,
        &plan.prologue[..layout.stolen_len],
        &patch,
    )?;
    Ok(patch)
}

// checked against the patch length before decoding, since decoding from the middle
// of another hook's jump fails in misleading ways, and against the full stolen
// range after it
fn reject_overlap(hooks: &[Range<usize>], range: Range<usize>) -> Result<()> {
    match hooks
        .iter()
        .find(|hooked| hooked.start < range.end && range.start < hooked.end)
    {
        Some(hooked) => Err(Error::AlreadyHooked {
            address: hooked.start.max(range.start),
        }),
        None => Ok(()),
    }
}

// reads only as far as memory stays readable, so a function at the end of a
// mapping cannot fault the decoder's lookahead
unsafe fn read_prologue(memory: &Memory, target: usize, max_len: usize) -> Result<Vec<u8>> {
    let page_size = memory::page_size();
    let first = memory::page_start(target, page_size);
    match (memory.query)(first)? {
        Some(protection) if protection.readable() && protection.executable() => {}
        _ => return Err(Error::NotExecutable { address: target }),
    }
    let wanted_end = target.saturating_add(max_len);
    let mut end = first + page_size;
    while end < wanted_end {
        match (memory.query)(end)? {
            Some(protection) if protection.readable() => end += page_size,
            _ => break,
        }
    }
    let len = end.min(wanted_end) - target;
    Ok(std::slice::from_raw_parts(target as *const u8, len).to_vec())
}

#[cfg(test)]
mod tests;
