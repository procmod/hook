use crate::alloc;
use crate::error::{Error, Result};
use crate::jump;
use crate::protect;
use crate::relocate;

const TRAMPOLINE_SIZE: usize = 64;

/// An installed inline hook that redirects a target function to a detour.
///
/// The hook overwrites the first few bytes of the target function with a jump
/// to the detour. A trampoline is allocated nearby containing the original
/// instructions and a jump back, allowing the detour to call the original.
///
/// The hook is automatically removed when dropped.
pub struct Hook {
    target: *mut u8,
    trampoline_ptr: *mut u8,
    original_bytes: Vec<u8>,
    stolen_len: usize,
    installed: bool,
}

unsafe impl Send for Hook {}
unsafe impl Sync for Hook {}

impl Hook {
    /// Install an inline hook at `target`, redirecting calls to `detour`.
    ///
    /// Returns a `Hook` whose [`trampoline`](Hook::trampoline) can be used
    /// to call the original function.
    ///
    /// # Safety
    ///
    /// - `target` must point to the start of a callable function in executable memory.
    /// - `detour` must be a function with the same calling convention and signature.
    /// - No thread may be executing the first 14 bytes of `target` during this call.
    pub unsafe fn install(target: *const u8, detour: *const u8) -> Result<Self> {
        let target_addr = target as u64;
        let detour_addr = detour as u64;

        let (patch, patch_len) = if let Some(rel) = jump::encode_rel32(target_addr, detour_addr) {
            (rel.to_vec(), jump::REL32_LEN)
        } else {
            (jump::encode_abs64(detour_addr).to_vec(), jump::ABS64_LEN)
        };

        let read_len = patch_len.max(16);
        let original_code = std::slice::from_raw_parts(target, read_len);

        let trampoline = alloc::alloc_near(target as usize, TRAMPOLINE_SIZE)?;
        let trampoline_addr = trampoline as u64;

        let relocated =
            match relocate::relocate(original_code, target_addr, trampoline_addr, patch_len) {
                Ok(r) => r,
                Err(e) => {
                    alloc::free(trampoline, TRAMPOLINE_SIZE);
                    return Err(e);
                }
            };

        let jump_back_rip = trampoline_addr + relocated.bytes.len() as u64;
        let continue_addr = target_addr + relocated.stolen_len as u64;
        let jump_back = match jump::encode_rel32(jump_back_rip, continue_addr) {
            Some(jb) => jb,
            None => {
                alloc::free(trampoline, TRAMPOLINE_SIZE);
                return Err(Error::RelocationFailed);
            }
        };

        std::ptr::copy_nonoverlapping(relocated.bytes.as_ptr(), trampoline, relocated.bytes.len());
        std::ptr::copy_nonoverlapping(
            jump_back.as_ptr(),
            trampoline.add(relocated.bytes.len()),
            jump::REL32_LEN,
        );

        let original_bytes = original_code[..relocated.stolen_len].to_vec();

        let old_prot = match protect::make_writable(target as usize, relocated.stolen_len) {
            Ok(p) => p,
            Err(e) => {
                alloc::free(trampoline, TRAMPOLINE_SIZE);
                return Err(e);
            }
        };

        std::ptr::copy_nonoverlapping(patch.as_ptr(), target as *mut u8, patch_len);

        if relocated.stolen_len > patch_len {
            std::ptr::write_bytes(
                (target as *mut u8).add(patch_len),
                0x90,
                relocated.stolen_len - patch_len,
            );
        }

        let _ = protect::restore_protection(target as usize, relocated.stolen_len, old_prot);

        Ok(Hook {
            target: target as *mut u8,
            trampoline_ptr: trampoline,
            original_bytes,
            stolen_len: relocated.stolen_len,
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
        self.trampoline_ptr as *const u8
    }

    /// Remove the hook, restoring the original function bytes.
    ///
    /// # Safety
    ///
    /// No thread may be executing the trampoline or the patched region of the target.
    pub unsafe fn unhook(&mut self) -> Result<()> {
        if !self.installed {
            return Err(Error::NotInstalled);
        }

        let old_prot = protect::make_writable(self.target as usize, self.stolen_len)?;
        std::ptr::copy_nonoverlapping(self.original_bytes.as_ptr(), self.target, self.stolen_len);
        let _ = protect::restore_protection(self.target as usize, self.stolen_len, old_prot);

        alloc::free(self.trampoline_ptr, TRAMPOLINE_SIZE);
        self.installed = false;

        Ok(())
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        if self.installed {
            unsafe {
                let _ = self.unhook();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicPtr, Ordering};

    // each test gets its own target/detour/trampoline to avoid races

    static TRAMP_1: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
    static TRAMP_2: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
    static TRAMP_3: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());

    #[inline(never)]
    extern "C" fn target_1(x: i32) -> i32 {
        std::hint::black_box(std::hint::black_box(x) + 1)
    }

    extern "C" fn detour_1(x: i32) -> i32 {
        let original: extern "C" fn(i32) -> i32 =
            unsafe { std::mem::transmute(TRAMP_1.load(Ordering::SeqCst)) };
        original(x) + 100
    }

    #[inline(never)]
    extern "C" fn target_2(x: i32) -> i32 {
        std::hint::black_box(std::hint::black_box(x) * 2)
    }

    extern "C" fn detour_2(x: i32) -> i32 {
        let original: extern "C" fn(i32) -> i32 =
            unsafe { std::mem::transmute(TRAMP_2.load(Ordering::SeqCst)) };
        original(x) + 1000
    }

    #[inline(never)]
    extern "C" fn target_3(x: i32) -> i32 {
        std::hint::black_box(std::hint::black_box(x) + 10)
    }

    extern "C" fn detour_3(x: i32) -> i32 {
        let original: extern "C" fn(i32) -> i32 =
            unsafe { std::mem::transmute(TRAMP_3.load(Ordering::SeqCst)) };
        original(x) + 500
    }

    #[test]
    fn hook_and_unhook() {
        assert_eq!(target_1(5), 6);

        let mut hook =
            unsafe { Hook::install(target_1 as *const u8, detour_1 as *const u8) }.unwrap();

        TRAMP_1.store(hook.trampoline() as *mut u8, Ordering::SeqCst);

        assert_eq!(target_1(5), 106);

        unsafe { hook.unhook().unwrap() };

        assert_eq!(target_1(5), 6);
    }

    #[test]
    fn hook_auto_unhook_on_drop() {
        assert_eq!(target_2(7), 14);

        {
            let hook =
                unsafe { Hook::install(target_2 as *const u8, detour_2 as *const u8) }.unwrap();

            TRAMP_2.store(hook.trampoline() as *mut u8, Ordering::SeqCst);

            assert_eq!(target_2(7), 1014);
        }

        assert_eq!(target_2(7), 14);
    }

    #[test]
    fn unhook_twice_fails() {
        let mut hook =
            unsafe { Hook::install(target_3 as *const u8, detour_3 as *const u8) }.unwrap();

        TRAMP_3.store(hook.trampoline() as *mut u8, Ordering::SeqCst);

        unsafe { hook.unhook().unwrap() };

        let result = unsafe { hook.unhook() };
        assert!(result.is_err());
    }
}
