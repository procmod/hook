use crate::error::{Error, Result};

fn round_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

fn page_range(addr: usize, len: usize, page_size: usize) -> (usize, usize) {
    let start = addr & !(page_size - 1);
    let size = round_up(addr + len, page_size) - start;
    (start, size)
}

// macos requires mach_vm_protect with VM_PROT_COPY to make code pages writable.
// regular mprotect fails on signed code pages

#[cfg(target_os = "macos")]
mod mach {
    pub const VM_PROT_READ: i32 = 0x01;
    pub const VM_PROT_WRITE: i32 = 0x02;
    pub const VM_PROT_EXECUTE: i32 = 0x04;
    pub const VM_PROT_COPY: i32 = 0x10;
    pub const KERN_SUCCESS: i32 = 0;

    unsafe extern "C" {
        pub fn mach_task_self() -> u32;
        pub fn mach_vm_protect(
            target_task: u32,
            address: u64,
            size: u64,
            set_maximum: i32,
            new_protection: i32,
        ) -> i32;
    }
}

#[cfg(target_os = "macos")]
pub unsafe fn make_writable(addr: usize, len: usize) -> Result<u32> {
    let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
    let (start, size) = page_range(addr, len, page_size);

    let result = mach::mach_vm_protect(
        mach::mach_task_self(),
        start as u64,
        size as u64,
        0,
        mach::VM_PROT_READ | mach::VM_PROT_WRITE | mach::VM_PROT_COPY,
    );

    if result != mach::KERN_SUCCESS {
        return Err(Error::ProtectFailed);
    }

    Ok(0)
}

#[cfg(target_os = "macos")]
pub unsafe fn restore_protection(addr: usize, len: usize, _old: u32) -> Result<()> {
    let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
    let (start, size) = page_range(addr, len, page_size);

    let result = mach::mach_vm_protect(
        mach::mach_task_self(),
        start as u64,
        size as u64,
        0,
        mach::VM_PROT_READ | mach::VM_PROT_EXECUTE,
    );

    if result != mach::KERN_SUCCESS {
        return Err(Error::ProtectFailed);
    }

    Ok(())
}

#[cfg(target_os = "linux")]
pub unsafe fn make_writable(addr: usize, len: usize) -> Result<u32> {
    let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
    let (start, size) = page_range(addr, len, page_size);

    let result = libc::mprotect(
        start as *mut libc::c_void,
        size,
        libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
    );

    if result != 0 {
        return Err(Error::ProtectFailed);
    }

    Ok(0)
}

#[cfg(target_os = "linux")]
pub unsafe fn restore_protection(addr: usize, len: usize, _old: u32) -> Result<()> {
    let page_size = libc::sysconf(libc::_SC_PAGESIZE) as usize;
    let (start, size) = page_range(addr, len, page_size);

    let result = libc::mprotect(
        start as *mut libc::c_void,
        size,
        libc::PROT_READ | libc::PROT_EXEC,
    );

    if result != 0 {
        return Err(Error::ProtectFailed);
    }

    Ok(())
}

#[cfg(windows)]
pub unsafe fn make_writable(addr: usize, len: usize) -> Result<u32> {
    use windows_sys::Win32::System::Memory::*;

    let mut old = 0u32;
    let result = VirtualProtect(
        addr as *const std::ffi::c_void,
        len,
        PAGE_EXECUTE_READWRITE,
        &mut old,
    );

    if result == 0 {
        return Err(Error::ProtectFailed);
    }

    Ok(old)
}

#[cfg(windows)]
pub unsafe fn restore_protection(addr: usize, len: usize, old: u32) -> Result<()> {
    use windows_sys::Win32::System::Memory::*;

    let mut dummy = 0u32;
    let result = VirtualProtect(addr as *const std::ffi::c_void, len, old, &mut dummy);

    if result == 0 {
        return Err(Error::ProtectFailed);
    }

    Ok(())
}
