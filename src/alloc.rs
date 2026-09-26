use crate::error::{Error, Result};
use crate::memory::{self, Protection};

const MAX_DELTA: usize = 0x7FFF_0000;
const STEP: usize = 0x10000;

/// A writable block of memory within rel32 reach of a target.
pub struct Block {
    pub ptr: *mut u8,
    pub len: usize,
}

fn within_range(address: usize, len: usize, target: usize) -> bool {
    address.abs_diff(target) <= MAX_DELTA && (address + len).abs_diff(target) <= MAX_DELTA
}

fn candidates(target: usize) -> impl Iterator<Item = usize> {
    let base = target & !(STEP - 1);
    (1..=(MAX_DELTA / STEP))
        .flat_map(move |i| [base.checked_add(i * STEP), base.checked_sub(i * STEP)])
        .flatten()
        .filter(|&address| address != 0)
}

/// Allocate a read-write block near `target`.
pub fn alloc_near(target: usize) -> Result<Block> {
    let len = platform::granularity();
    candidates(target)
        .find_map(|hint| {
            let ptr = platform::map(hint, len)?;
            if within_range(ptr as usize, len, target) {
                Some(ptr)
            } else {
                unsafe { platform::unmap(ptr, len) };
                None
            }
        })
        .map(|ptr| Block { ptr, len })
        .ok_or(Error::TrampolineAlloc)
}

/// Copy `code` into the block, make it read-execute, and synchronize instruction caches.
///
/// # Safety
///
/// The block must be writable and not yet executed.
pub unsafe fn seal(block: &Block, code: &[u8]) -> Result<()> {
    debug_assert!(code.len() <= block.len);
    std::ptr::copy_nonoverlapping(code.as_ptr(), block.ptr, code.len());
    (memory::SYSTEM.protect)(block.ptr as usize, block.len, Protection::READ_EXECUTE)?;
    (memory::SYSTEM.flush)(block.ptr as usize, code.len())
}

/// # Safety
///
/// No thread may execute the block during or after the call.
pub unsafe fn free(block: &Block) {
    platform::unmap(block.ptr, block.len);
}

#[cfg(unix)]
mod platform {
    pub fn granularity() -> usize {
        crate::memory::page_size()
    }

    pub fn map(hint: usize, len: usize) -> Option<*mut u8> {
        let ptr = unsafe {
            libc::mmap(
                hint as *mut libc::c_void,
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED || ptr.is_null() {
            return None;
        }
        Some(ptr as *mut u8)
    }

    pub unsafe fn unmap(ptr: *mut u8, len: usize) {
        libc::munmap(ptr as *mut libc::c_void, len);
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
    };

    pub fn granularity() -> usize {
        super::STEP
    }

    pub fn map(hint: usize, len: usize) -> Option<*mut u8> {
        let ptr = unsafe {
            VirtualAlloc(
                hint as *const std::ffi::c_void,
                len,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if ptr.is_null() {
            return None;
        }
        Some(ptr as *mut u8)
    }

    pub unsafe fn unmap(ptr: *mut u8, _len: usize) {
        VirtualFree(ptr as *mut std::ffi::c_void, 0, MEM_RELEASE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_alternate_outward_and_skip_null() {
        let first: Vec<usize> = candidates(0x5_0000).take(4).collect();
        assert_eq!(first, [0x6_0000, 0x4_0000, 0x7_0000, 0x3_0000]);
        assert!(candidates(0x1_0000).all(|address| address != 0));
    }

    #[test]
    fn range_check_covers_the_whole_block() {
        assert!(within_range(0x1000 + MAX_DELTA - 0x100, 0x100, 0x1000));
        assert!(!within_range(0x1000 + MAX_DELTA - 0x100, 0x200, 0x1000));
    }

    #[test]
    fn allocates_near_a_target() {
        let target = allocates_near_a_target as usize;
        let block = alloc_near(target).unwrap();
        assert!(within_range(block.ptr as usize, block.len, target));
        unsafe {
            seal(&block, &[0xC3]).unwrap();
            let function: extern "C" fn() = std::mem::transmute(block.ptr);
            function();
            free(&block);
        }
    }
}
