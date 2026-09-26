use super::*;
use crate::memory::Protection;
use crate::patch::fake::{self, Faults};
use std::sync::atomic::{AtomicPtr, Ordering};

static TRAMPOLINE_1: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static TRAMPOLINE_2: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());

#[inline(never)]
extern "C" fn target_1(x: i32) -> i32 {
    std::hint::black_box(std::hint::black_box(x) + 1)
}

extern "C" fn detour_1(x: i32) -> i32 {
    let original: extern "C" fn(i32) -> i32 =
        unsafe { std::mem::transmute(TRAMPOLINE_1.load(Ordering::SeqCst)) };
    original(x) + 100
}

#[inline(never)]
extern "C" fn target_2(x: i32) -> i32 {
    std::hint::black_box(std::hint::black_box(x) * 2)
}

extern "C" fn detour_2(x: i32) -> i32 {
    let original: extern "C" fn(i32) -> i32 =
        unsafe { std::mem::transmute(TRAMPOLINE_2.load(Ordering::SeqCst)) };
    original(x) + 1000
}

#[test]
fn hook_and_unhook() {
    assert_eq!(target_1(5), 6);
    let hook = unsafe { Hook::install(target_1 as *const u8, detour_1 as *const u8) }.unwrap();
    TRAMPOLINE_1.store(hook.trampoline() as *mut u8, Ordering::SeqCst);
    assert_eq!(target_1(5), 106);
    hook.unhook().unwrap();
    assert_eq!(target_1(5), 6);
}

#[test]
fn hook_auto_unhook_on_drop() {
    assert_eq!(target_2(7), 14);
    {
        let hook = unsafe { Hook::install(target_2 as *const u8, detour_2 as *const u8) }.unwrap();
        TRAMPOLINE_2.store(hook.trampoline() as *mut u8, Ordering::SeqCst);
        assert_eq!(target_2(7), 1014);
    }
    assert_eq!(target_2(7), 14);
}

#[test]
fn hooks_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Hook>();
    assert_send_sync::<UnhookError>();
}

// mov eax, imm32
fn mov_eax(value: u8) -> [u8; 5] {
    [0xB8, value, 0, 0, 0]
}

const RET: u8 = 0xC3;
const NOP: u8 = 0x90;

/// Pages of code allocated for a test, so tests control page boundaries and
/// protections. Detours live in the first page, which keeps every jump rel32.
struct CodePages {
    base: *mut u8,
    count: usize,
    page_size: usize,
}

impl CodePages {
    fn new(count: usize) -> Self {
        let page_size = memory::page_size();
        let base = platform::map(page_size * count);
        let pages = Self {
            base,
            count,
            page_size,
        };
        pages.write(0, &[0xCC; 32]);
        pages
    }

    fn address(&self, offset: usize) -> usize {
        self.base as usize + offset
    }

    fn page(&self, index: usize) -> usize {
        self.address(index * self.page_size)
    }

    fn write(&self, offset: usize, bytes: &[u8]) {
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.base.add(offset), bytes.len())
        };
    }

    fn read(&self, offset: usize, len: usize) -> Vec<u8> {
        unsafe { std::slice::from_raw_parts(self.base.add(offset), len) }.to_vec()
    }

    fn protect(&self, index: usize, protection: Protection) {
        (memory::SYSTEM.protect)(self.page(index), self.page_size, protection).unwrap();
    }

    fn protection(&self, index: usize) -> Option<Protection> {
        (memory::SYSTEM.query)(self.page(index)).unwrap()
    }

    // mov eax, 7; ret
    fn detour(&self) -> usize {
        self.write(0, &mov_eax(7));
        self.write(5, &[RET]);
        self.address(0)
    }

    fn call(&self, address: usize) -> i32 {
        let function: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        function()
    }
}

impl Drop for CodePages {
    fn drop(&mut self) {
        unsafe { platform::unmap(self.base, self.page_size * self.count) };
    }
}

#[cfg(unix)]
mod platform {
    pub fn map(len: usize) -> *mut u8 {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED);
        ptr as *mut u8
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

    pub fn map(len: usize) -> *mut u8 {
        let ptr = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                len,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        assert!(!ptr.is_null());
        ptr as *mut u8
    }

    pub unsafe fn unmap(ptr: *mut u8, _len: usize) {
        VirtualFree(ptr as *mut std::ffi::c_void, 0, MEM_RELEASE);
    }
}

#[test]
fn patch_straddling_pages_restores_each_page_protection() {
    let pages = CodePages::new(2);
    let detour = pages.detour();
    let target = pages.page_size - 2;
    pages.write(target, &mov_eax(42));
    pages.write(target + 5, &[NOP; 12]);
    pages.write(target + 17, &[RET]);
    let original = pages.read(target, 18);
    let read_write_execute = Protection::READ_EXECUTE.writable();
    pages.protect(0, Protection::READ_EXECUTE);
    pages.protect(1, read_write_execute);

    // protections are checked before any code runs: Rosetta write-protects
    // writable pages once it translates code on them
    let hook =
        unsafe { Hook::install(pages.address(target) as *const u8, detour as *const u8) }.unwrap();
    assert_eq!(pages.protection(0), Some(Protection::READ_EXECUTE));
    assert_eq!(pages.protection(1), Some(read_write_execute));
    assert_eq!(pages.call(pages.address(target)), 7);
    assert_eq!(pages.call(hook.trampoline() as usize), 42);

    // undo Rosetta's write-protection so removal is checked against the original
    pages.protect(1, read_write_execute);
    hook.unhook().unwrap();
    assert_eq!(pages.protection(0), Some(Protection::READ_EXECUTE));
    assert_eq!(pages.protection(1), Some(read_write_execute));
    assert_eq!(pages.read(target, 18), original);
    assert_eq!(pages.call(pages.address(target)), 42);
}

#[test]
fn prologue_ending_at_an_inaccessible_page_is_hookable() {
    let pages = CodePages::new(2);
    let detour = pages.detour();
    let target = pages.page_size - 5;
    pages.write(target, &mov_eax(42));
    pages.protect(0, Protection::READ_EXECUTE);
    pages.protect(1, Protection::NONE);

    let hook =
        unsafe { Hook::install(pages.address(target) as *const u8, detour as *const u8) }.unwrap();
    assert_eq!(pages.call(pages.address(target)), 7);
    hook.unhook().unwrap();
    assert_eq!(pages.read(target, 5), mov_eax(42));
}

#[test]
fn prologue_running_into_an_inaccessible_page_is_rejected() {
    let pages = CodePages::new(2);
    let detour = pages.detour();
    let target = pages.page_size - 3;
    pages.write(target, &[NOP; 3]);
    pages.protect(0, Protection::READ_EXECUTE);
    pages.protect(1, Protection::NONE);

    let error = unsafe { Hook::install(pages.address(target) as *const u8, detour as *const u8) }
        .unwrap_err();
    assert!(matches!(error, Error::PrologueUnreadable { address } if address == pages.page(1)));
    assert_eq!(pages.read(target, 3), [NOP; 3]);
}

#[test]
fn data_pages_are_rejected() {
    let pages = CodePages::new(2);
    let detour = pages.detour();
    let target = pages.page(1);
    pages.write(pages.page_size, &[NOP; 16]);
    pages.protect(0, Protection::READ_EXECUTE);

    let error = unsafe { Hook::install(target as *const u8, detour as *const u8) }.unwrap_err();
    assert!(matches!(error, Error::NotExecutable { address } if address == target));
}

#[test]
fn functions_that_return_early_are_rejected() {
    let pages = CodePages::new(1);
    let detour = pages.detour();
    pages.write(16, &[NOP, RET, NOP, NOP, NOP, NOP]);
    pages.protect(0, Protection::READ_EXECUTE);

    let error =
        unsafe { Hook::install(pages.address(16) as *const u8, detour as *const u8) }.unwrap_err();
    assert!(matches!(
        error,
        Error::InsufficientSpace { need: 5, have: 2 }
    ));
    assert_eq!(pages.read(16, 2), [NOP, RET]);
}

#[test]
fn invalid_prologues_leave_the_target_untouched() {
    let pages = CodePages::new(1);
    let detour = pages.detour();
    pages.write(16, &[0x06, NOP, NOP, NOP, NOP, NOP]);
    pages.protect(0, Protection::READ_EXECUTE);

    let error =
        unsafe { Hook::install(pages.address(16) as *const u8, detour as *const u8) }.unwrap_err();
    assert!(matches!(error, Error::RelocationFailed));
    assert_eq!(pages.read(16, 6), [0x06, NOP, NOP, NOP, NOP, NOP]);
}

#[test]
fn overlapping_hooks_are_rejected() {
    let pages = CodePages::new(1);
    let detour = pages.detour();
    pages.write(16, &[NOP; 24]);
    pages.write(40, &[RET]);
    pages.protect(0, Protection::READ_EXECUTE);
    let target = pages.address(16);

    let hook = unsafe { Hook::install(target as *const u8, detour as *const u8) }.unwrap();
    for address in [target, target + 2] {
        let error =
            unsafe { Hook::install(address as *const u8, detour as *const u8) }.unwrap_err();
        assert!(matches!(error, Error::AlreadyHooked { .. }), "{error:?}");
    }
    hook.unhook().unwrap();

    let hook = unsafe { Hook::install((target + 2) as *const u8, detour as *const u8) }.unwrap();
    hook.unhook().unwrap();
}

#[test]
fn modified_patches_are_not_overwritten() {
    let pages = CodePages::new(1);
    let detour = pages.detour();
    pages.write(16, &[NOP; 8]);
    pages.write(24, &[RET]);
    pages.protect(0, Protection::READ_EXECUTE.writable());

    let hook =
        unsafe { Hook::install(pages.address(16) as *const u8, detour as *const u8) }.unwrap();
    let patch = pages.read(16, 5);
    pages.write(16, &[0xCC]);

    let error = hook.unhook().unwrap_err();
    assert!(matches!(error.error(), Error::PatchModified { .. }));
    assert_eq!(pages.read(16, 1), [0xCC]);

    pages.write(16, &patch[..1]);
    error.into_hook().unhook().unwrap();
    assert_eq!(pages.read(16, 8), [NOP; 8]);
}

/// A leaked buffer whose middle page the fake memory reports as code. Leaked
/// because a failed drop keeps its range registered for the life of the process.
fn fake_code() -> usize {
    let page_size = memory::page_size();
    let bytes = Vec::leak(vec![NOP; page_size * 3]);
    let base = memory::page_start(bytes.as_ptr() as usize, page_size) + page_size;
    fake::register(base, page_size);
    base
}

fn read(address: usize, len: usize) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec()
}

#[test]
fn failed_install_restores_the_target_and_releases_the_range() {
    let target = fake_code();
    let detour = target + 0x800;
    fake::inject(Faults {
        protect_call: Some(0),
        flush_call: None,
    });
    let error = unsafe { Hook::install_in(&fake::MEMORY, target, detour) }.unwrap_err();
    assert!(matches!(error, Error::ProtectFailed { .. }));
    assert_eq!(read(target, 5), [NOP; 5]);

    fake::inject(NO_FAULTS);
    let hook = unsafe { Hook::install_in(&fake::MEMORY, target, detour) }.unwrap();
    assert_eq!(read(target, 1), [0xE9]);
    hook.unhook().unwrap();
    assert_eq!(read(target, 5), [NOP; 5]);
}

const NO_FAULTS: Faults = Faults {
    protect_call: None,
    flush_call: None,
};

#[test]
fn failed_unhook_returns_the_installed_hook() {
    let target = fake_code();
    let mut hook = unsafe { Hook::install_in(&fake::MEMORY, target, target + 0x800) }.unwrap();
    let patch = read(target, 5);

    let unlock_fails = Faults {
        protect_call: Some(0),
        flush_call: None,
    };
    let flush_fails = Faults {
        protect_call: None,
        flush_call: Some(0),
    };
    let relock_fails = Faults {
        protect_call: Some(1),
        flush_call: None,
    };
    for faults in [unlock_fails, flush_fails, relock_fails] {
        fake::inject(faults);
        let error = hook.unhook().unwrap_err();
        assert_eq!(read(target, 5), patch);
        hook = error.into_hook();
    }

    fake::inject(NO_FAULTS);
    hook.unhook().unwrap();
    assert_eq!(read(target, 5), [NOP; 5]);
}

#[test]
fn failed_drop_keeps_the_hook_installed() {
    let target = fake_code();
    let hook = unsafe { Hook::install_in(&fake::MEMORY, target, target + 0x800) }.unwrap();
    let patch = read(target, 5);

    fake::inject(Faults {
        protect_call: Some(0),
        flush_call: None,
    });
    drop(hook);
    assert_eq!(read(target, 5), patch);

    fake::inject(NO_FAULTS);
    let error = unsafe { Hook::install_in(&fake::MEMORY, target, target + 0x800) }.unwrap_err();
    assert!(matches!(error, Error::AlreadyHooked { .. }));
}
