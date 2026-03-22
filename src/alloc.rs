use crate::error::{Error, Result};

const MAX_DELTA: usize = 0x7FFF_0000;
const STEP: usize = 0x10000;

fn within_range(addr: usize, target: usize) -> bool {
    let delta = if addr > target {
        addr - target
    } else {
        target - addr
    };
    delta <= MAX_DELTA
}

fn round_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

#[cfg(unix)]
pub unsafe fn alloc_near(target: usize, size: usize) -> Result<*mut u8> {
    let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
    let alloc_size = round_up(size, page_size);
    let base = target & !(STEP - 1);

    for i in 1..=(MAX_DELTA / STEP) {
        for addr in [base.wrapping_add(i * STEP), base.wrapping_sub(i * STEP)] {
            if addr == 0 {
                continue;
            }

            let ptr = libc::mmap(
                addr as *mut libc::c_void,
                alloc_size,
                libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );

            if ptr == libc::MAP_FAILED || ptr.is_null() {
                continue;
            }

            if within_range(ptr as usize, target) {
                return Ok(ptr as *mut u8);
            }

            libc::munmap(ptr, alloc_size);
        }
    }

    Err(Error::TrampolineAlloc)
}

#[cfg(unix)]
pub unsafe fn free(ptr: *mut u8, size: usize) {
    let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
    libc::munmap(ptr as *mut libc::c_void, round_up(size, page_size));
}

#[cfg(windows)]
pub unsafe fn alloc_near(target: usize, size: usize) -> Result<*mut u8> {
    use windows_sys::Win32::System::Memory::*;

    let alloc_size = round_up(size, STEP);
    let base = target & !(STEP - 1);

    for i in 1..=(MAX_DELTA / STEP) {
        for addr in [base.wrapping_add(i * STEP), base.wrapping_sub(i * STEP)] {
            if addr == 0 {
                continue;
            }

            let ptr = VirtualAlloc(
                addr as *const std::ffi::c_void,
                alloc_size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            );

            if ptr.is_null() {
                continue;
            }

            if within_range(ptr as usize, target) {
                return Ok(ptr as *mut u8);
            }

            VirtualFree(ptr, 0, MEM_RELEASE);
        }
    }

    Err(Error::TrampolineAlloc)
}

#[cfg(windows)]
pub unsafe fn free(ptr: *mut u8, _size: usize) {
    use windows_sys::Win32::System::Memory::*;
    VirtualFree(ptr as *mut std::ffi::c_void, 0, MEM_RELEASE);
}
