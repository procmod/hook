use crate::error::{Error, Result};

/// Page protection in the platform's native encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Protection(u32);

/// The memory operations a patch transaction needs, so tests can inject failures.
pub struct Memory {
    pub query: fn(usize) -> Result<Option<Protection>>,
    pub protect: fn(usize, usize, Protection) -> Result<()>,
    pub flush: fn(usize, usize) -> Result<()>,
}

pub static SYSTEM: Memory = Memory {
    query: platform::query,
    protect: platform::protect,
    flush: platform::flush,
};

pub use platform::page_size;

pub fn page_start(address: usize, page_size: usize) -> usize {
    address & !(page_size - 1)
}

pub fn pages(address: usize, len: usize, page_size: usize) -> impl Iterator<Item = usize> {
    let first = page_start(address, page_size);
    let last = page_start(address + len.max(1) - 1, page_size);
    (first..=last).step_by(page_size)
}

fn os_error(address: usize) -> Error {
    Error::ProtectFailed {
        address,
        source: std::io::Error::last_os_error(),
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{os_error, Protection};
    use crate::error::{Error, Result};

    impl Protection {
        #[cfg(test)]
        pub const NONE: Self = Self(libc::PROT_NONE as u32);
        pub const READ_EXECUTE: Self = Self((libc::PROT_READ | libc::PROT_EXEC) as u32);

        pub fn readable(self) -> bool {
            self.0 & libc::PROT_READ as u32 != 0
        }

        pub fn executable(self) -> bool {
            self.0 & libc::PROT_EXEC as u32 != 0
        }

        pub fn writable(self) -> Self {
            Self(self.0 | libc::PROT_WRITE as u32)
        }
    }

    pub fn page_size() -> usize {
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }

    pub fn query(address: usize) -> Result<Option<Protection>> {
        let maps = std::fs::read_to_string("/proc/self/maps")
            .map_err(|source| Error::QueryFailed { address, source })?;
        Ok(maps.lines().find_map(|line| parse_line(line, address)))
    }

    fn parse_line(line: &str, address: usize) -> Option<Protection> {
        let mut fields = line.split_whitespace();
        let (start, end) = fields.next()?.split_once('-')?;
        let start = usize::from_str_radix(start, 16).ok()?;
        let end = usize::from_str_radix(end, 16).ok()?;
        if !(start..end).contains(&address) {
            return None;
        }
        let perms = fields.next()?.as_bytes();
        let flag = |index: usize, byte: u8, bit: i32| {
            if perms.get(index) == Some(&byte) {
                bit as u32
            } else {
                0
            }
        };
        Some(Protection(
            flag(0, b'r', libc::PROT_READ)
                | flag(1, b'w', libc::PROT_WRITE)
                | flag(2, b'x', libc::PROT_EXEC),
        ))
    }

    pub fn protect(start: usize, len: usize, protection: Protection) -> Result<()> {
        if unsafe { libc::mprotect(start as *mut libc::c_void, len, protection.0 as i32) } != 0 {
            return Err(os_error(start));
        }
        Ok(())
    }

    const MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE: libc::c_long = 1 << 5;
    const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE: libc::c_long = 1 << 6;

    // x86 keeps instruction caches coherent, but a core that prefetched the old bytes
    // must execute a serializing instruction before running the new ones. membarrier
    // forces that on every thread of the process. kernels older than 4.16 lack it,
    // and there the caller's quiescence obligation is all that remains
    pub fn flush(_address: usize, _len: usize) -> Result<()> {
        static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let registered = *REGISTERED.get_or_init(|| unsafe {
            libc::syscall(
                libc::SYS_membarrier,
                MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE,
                0,
            ) == 0
        });
        if !registered {
            return Ok(());
        }
        let result = unsafe {
            libc::syscall(
                libc::SYS_membarrier,
                MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE,
                0,
            )
        };
        if result != 0 {
            return Err(Error::FlushFailed {
                source: std::io::Error::last_os_error(),
            });
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_maps_lines() {
            let line = "7f0000001000-7f0000003000 r-xp 00000000 08:01 42 /usr/lib/libgame.so";
            assert_eq!(
                parse_line(line, 0x7f00_0000_2fff),
                Some(Protection::READ_EXECUTE)
            );
            assert_eq!(parse_line(line, 0x7f00_0000_3000), None);
            assert_eq!(parse_line(line, 0x7f00_0000_0fff), None);
            let line = "7f0000001000-7f0000002000 ---p 00000000 00:00 0";
            assert_eq!(parse_line(line, 0x7f00_0000_1000), Some(Protection::NONE));
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{os_error, Protection};
    use crate::error::{Error, Result};

    const VM_PROT_READ: u32 = 0x01;
    const VM_PROT_WRITE: u32 = 0x02;
    const VM_PROT_EXECUTE: u32 = 0x04;
    const VM_PROT_COPY: u32 = 0x10;
    const KERN_SUCCESS: i32 = 0;
    const KERN_INVALID_ADDRESS: i32 = 1;
    const VM_REGION_BASIC_INFO_64: i32 = 9;
    const VM_REGION_BASIC_INFO_64_COUNT: u32 = 9;

    #[repr(C)]
    #[derive(Default)]
    struct RegionBasicInfo64 {
        protection: i32,
        max_protection: i32,
        inheritance: u32,
        shared: u32,
        reserved: u32,
        offset: u64,
        behavior: i32,
        user_wired_count: u16,
    }

    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn mach_vm_protect(task: u32, address: u64, size: u64, maximum: i32, prot: i32) -> i32;
        fn mach_vm_region(
            task: u32,
            address: *mut u64,
            size: *mut u64,
            flavor: i32,
            info: *mut RegionBasicInfo64,
            count: *mut u32,
            object_name: *mut u32,
        ) -> i32;
        fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
    }

    impl Protection {
        #[cfg(test)]
        pub const NONE: Self = Self(0);
        pub const READ_EXECUTE: Self = Self(VM_PROT_READ | VM_PROT_EXECUTE);

        pub fn readable(self) -> bool {
            self.0 & VM_PROT_READ != 0
        }

        pub fn executable(self) -> bool {
            self.0 & VM_PROT_EXECUTE != 0
        }

        pub fn writable(self) -> Self {
            Self(self.0 | VM_PROT_WRITE)
        }
    }

    pub fn page_size() -> usize {
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }

    pub fn query(address: usize) -> Result<Option<Protection>> {
        let mut region = address as u64;
        let mut size = 0u64;
        let mut info = RegionBasicInfo64::default();
        let mut count = VM_REGION_BASIC_INFO_64_COUNT;
        let mut object_name = 0u32;
        let kr = unsafe {
            mach_vm_region(
                mach_task_self(),
                &mut region,
                &mut size,
                VM_REGION_BASIC_INFO_64,
                &mut info,
                &mut count,
                &mut object_name,
            )
        };
        if kr == KERN_INVALID_ADDRESS {
            return Ok(None);
        }
        if kr != KERN_SUCCESS {
            return Err(Error::QueryFailed {
                address,
                source: std::io::Error::from_raw_os_error(kr),
            });
        }
        // mach_vm_region reports the next region when the address is unmapped
        if region > address as u64 {
            return Ok(None);
        }
        Ok(Some(Protection(info.protection as u32)))
    }

    // signed code pages cannot be made writable in place. VM_PROT_COPY gives the
    // process a private copy of the page that it may write
    pub fn protect(start: usize, len: usize, protection: Protection) -> Result<()> {
        let mut native = protection.0;
        if native & VM_PROT_WRITE != 0 {
            native |= VM_PROT_COPY;
        }
        let kr = unsafe {
            mach_vm_protect(mach_task_self(), start as u64, len as u64, 0, native as i32)
        };
        if kr != KERN_SUCCESS {
            return Err(os_error(start));
        }
        Ok(())
    }

    pub fn flush(address: usize, len: usize) -> Result<()> {
        unsafe { sys_icache_invalidate(address as *mut libc::c_void, len) };
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use super::{os_error, Protection};
    use crate::error::{Error, Result};
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE,
        PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD,
        PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY,
    };
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    const ACCESS_MASK: u32 = 0xFF;

    impl Protection {
        #[cfg(test)]
        pub const NONE: Self = Self(windows_sys::Win32::System::Memory::PAGE_NOACCESS);
        pub const READ_EXECUTE: Self = Self(PAGE_EXECUTE_READ);

        fn access(self) -> u32 {
            self.0 & ACCESS_MASK
        }

        pub fn readable(self) -> bool {
            self.0 & PAGE_GUARD == 0
                && matches!(
                    self.access(),
                    PAGE_READONLY
                        | PAGE_READWRITE
                        | PAGE_WRITECOPY
                        | PAGE_EXECUTE_READ
                        | PAGE_EXECUTE_READWRITE
                        | PAGE_EXECUTE_WRITECOPY
                )
        }

        pub fn executable(self) -> bool {
            matches!(
                self.access(),
                PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
            )
        }

        pub fn writable(self) -> Self {
            let access = match self.access() {
                PAGE_EXECUTE | PAGE_EXECUTE_READ => PAGE_EXECUTE_READWRITE,
                PAGE_READONLY => PAGE_READWRITE,
                access => access,
            };
            Self((self.0 & !ACCESS_MASK) | access)
        }
    }

    pub fn page_size() -> usize {
        let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
        unsafe { GetSystemInfo(&mut info) };
        info.dwPageSize as usize
    }

    pub fn query(address: usize) -> Result<Option<Protection>> {
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let written = unsafe {
            VirtualQuery(
                address as *const std::ffi::c_void,
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0 {
            return Ok(None);
        }
        if info.State != MEM_COMMIT {
            return Ok(None);
        }
        Ok(Some(Protection(info.Protect)))
    }

    pub fn protect(start: usize, len: usize, protection: Protection) -> Result<()> {
        let mut previous = 0u32;
        let result = unsafe {
            VirtualProtect(
                start as *const std::ffi::c_void,
                len,
                protection.0,
                &mut previous,
            )
        };
        if result == 0 {
            return Err(os_error(start));
        }
        Ok(())
    }

    pub fn flush(address: usize, len: usize) -> Result<()> {
        let result = unsafe {
            FlushInstructionCache(GetCurrentProcess(), address as *const std::ffi::c_void, len)
        };
        if result == 0 {
            return Err(Error::FlushFailed {
                source: std::io::Error::last_os_error(),
            });
        }
        Ok(())
    }
}
