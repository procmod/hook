use crate::error::{Error, Result};
use crate::memory::{self, Memory, Protection};

struct Page {
    start: usize,
    original: Protection,
}

/// Replace `expected` with `bytes` at `address`, as one transaction.
///
/// On success the bytes are in place, instruction caches are synchronized, and every
/// page has its original protection. On failure the original bytes are in place; only
/// a failure to restore protection can leave a page writable.
///
/// # Safety
///
/// No thread may execute or modify the range during the call.
pub unsafe fn replace(
    memory: &Memory,
    address: usize,
    expected: &[u8],
    bytes: &[u8],
) -> Result<()> {
    debug_assert_eq!(expected.len(), bytes.len());
    let pages = code_pages(memory, address, bytes.len())?;
    if std::slice::from_raw_parts(address as *const u8, expected.len()) != expected {
        return Err(Error::PatchModified { address });
    }
    unlock(memory, &pages)?;
    if let Err(error) = write(memory, address, bytes) {
        write(memory, address, expected).ok();
        relock(memory, &pages).ok();
        return Err(error);
    }
    if let Err(error) = relock(memory, &pages) {
        write(memory, address, expected).ok();
        relock(memory, &pages).ok();
        return Err(error);
    }
    Ok(())
}

fn code_pages(memory: &Memory, address: usize, len: usize) -> Result<Vec<Page>> {
    let page_size = memory::page_size();
    memory::pages(address, len, page_size)
        .map(|start| match (memory.query)(start)? {
            Some(original) if original.readable() && original.executable() => {
                Ok(Page { start, original })
            }
            _ => Err(Error::NotExecutable {
                address: start.max(address),
            }),
        })
        .collect()
}

fn unlock(memory: &Memory, pages: &[Page]) -> Result<()> {
    let page_size = memory::page_size();
    for (index, page) in pages.iter().enumerate() {
        if let Err(error) = (memory.protect)(page.start, page_size, page.original.writable()) {
            relock(memory, &pages[..index]).ok();
            return Err(error);
        }
    }
    Ok(())
}

// every page is attempted even after a failure, so one bad page does not strand the rest
fn relock(memory: &Memory, pages: &[Page]) -> Result<()> {
    let page_size = memory::page_size();
    let mut first = Ok(());
    for page in pages {
        let result = (memory.protect)(page.start, page_size, page.original);
        if first.is_ok() {
            first = result;
        }
    }
    first
}

unsafe fn write(memory: &Memory, address: usize, bytes: &[u8]) -> Result<()> {
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len());
    (memory.flush)(address, bytes.len())
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static REGION: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
        static FAIL_PROTECT_CALL: Cell<Option<usize>> = const { Cell::new(None) };
        static FAIL_FLUSH_CALL: Cell<Option<usize>> = const { Cell::new(None) };
        static PROTECT_CALLS: Cell<usize> = const { Cell::new(0) };
        static FLUSH_CALLS: Cell<usize> = const { Cell::new(0) };
        static PROTECTIONS: std::cell::RefCell<Vec<(usize, Protection)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Memory whose pages inside the registered region are readable and executable,
    /// and whose protect and flush calls can be made to fail on a chosen call.
    pub static MEMORY: Memory = Memory {
        query,
        protect,
        flush,
    };

    pub struct Faults {
        pub protect_call: Option<usize>,
        pub flush_call: Option<usize>,
    }

    pub fn register(start: usize, len: usize) {
        REGION.set((start, start + len));
        PROTECTIONS.with_borrow_mut(Vec::clear);
        inject(Faults {
            protect_call: None,
            flush_call: None,
        });
    }

    pub fn inject(faults: Faults) {
        FAIL_PROTECT_CALL.set(faults.protect_call);
        FAIL_FLUSH_CALL.set(faults.flush_call);
        PROTECT_CALLS.set(0);
        FLUSH_CALLS.set(0);
    }

    pub fn protection(page: usize) -> Option<Protection> {
        PROTECTIONS.with_borrow(|protections| {
            protections
                .iter()
                .rev()
                .find(|(start, _)| *start == page)
                .map(|(_, protection)| *protection)
        })
    }

    fn query(address: usize) -> Result<Option<Protection>> {
        let (start, end) = REGION.get();
        if !(start..end).contains(&address) {
            return Ok(None);
        }
        Ok(Some(
            protection(address).unwrap_or(Protection::READ_EXECUTE),
        ))
    }

    fn fault(address: usize) -> Error {
        Error::ProtectFailed {
            address,
            source: std::io::Error::other("injected"),
        }
    }

    fn protect(start: usize, _len: usize, protection: Protection) -> Result<()> {
        let call = PROTECT_CALLS.get();
        PROTECT_CALLS.set(call + 1);
        if FAIL_PROTECT_CALL.get() == Some(call) {
            return Err(fault(start));
        }
        PROTECTIONS.with_borrow_mut(|protections| protections.push((start, protection)));
        Ok(())
    }

    fn flush(_address: usize, _len: usize) -> Result<()> {
        let call = FLUSH_CALLS.get();
        FLUSH_CALLS.set(call + 1);
        if FAIL_FLUSH_CALL.get() == Some(call) {
            return Err(Error::FlushFailed {
                source: std::io::Error::other("injected"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{self, Faults};
    use super::*;

    struct Buffer {
        bytes: Vec<u8>,
        page_size: usize,
    }

    impl Buffer {
        fn new() -> Self {
            let page_size = memory::page_size();
            let buffer = Self {
                bytes: vec![0xCC; page_size * 4],
                page_size,
            };
            fake::register(buffer.base(), page_size * 2);
            buffer
        }

        fn base(&self) -> usize {
            memory::page_start(self.bytes.as_ptr() as usize, self.page_size) + self.page_size
        }

        fn straddling(&self) -> usize {
            self.base() + self.page_size - 3
        }

        fn read(&self, address: usize, len: usize) -> Vec<u8> {
            unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec()
        }
    }

    const ORIGINAL: [u8; 6] = [0xCC; 6];
    const PATCH: [u8; 6] = [0xE9, 1, 2, 3, 4, 0x90];

    #[test]
    fn replaces_bytes_and_restores_each_page() {
        let buffer = Buffer::new();
        let address = buffer.straddling();
        unsafe { replace(&fake::MEMORY, address, &ORIGINAL, &PATCH) }.unwrap();
        assert_eq!(buffer.read(address, 6), PATCH);
        for page in memory::pages(address, 6, buffer.page_size) {
            assert_eq!(fake::protection(page), Some(Protection::READ_EXECUTE));
        }
    }

    #[test]
    fn rejects_unexpected_bytes() {
        let buffer = Buffer::new();
        let address = buffer.straddling();
        let error = unsafe { replace(&fake::MEMORY, address, &PATCH, &ORIGINAL) }.unwrap_err();
        assert!(matches!(error, Error::PatchModified { .. }));
        assert_eq!(buffer.read(address, 6), ORIGINAL);
    }

    #[test]
    fn rejects_non_code_pages() {
        let buffer = Buffer::new();
        let outside = buffer.base() + buffer.page_size * 2 - 3;
        let error = unsafe { replace(&fake::MEMORY, outside, &ORIGINAL, &PATCH) }.unwrap_err();
        assert!(
            matches!(error, Error::NotExecutable { address } if address == buffer.base() + buffer.page_size * 2)
        );
        assert_eq!(buffer.read(outside, 3), ORIGINAL[..3]);
    }

    #[test]
    fn unlock_failure_on_second_page_relocks_the_first() {
        let buffer = Buffer::new();
        let address = buffer.straddling();
        fake::inject(Faults {
            protect_call: Some(1),
            flush_call: None,
        });
        let error = unsafe { replace(&fake::MEMORY, address, &ORIGINAL, &PATCH) }.unwrap_err();
        assert!(matches!(error, Error::ProtectFailed { .. }));
        assert_eq!(buffer.read(address, 6), ORIGINAL);
        assert_eq!(
            fake::protection(buffer.base()),
            Some(Protection::READ_EXECUTE)
        );
    }

    #[test]
    fn flush_failure_rolls_back() {
        let buffer = Buffer::new();
        let address = buffer.straddling();
        fake::inject(Faults {
            protect_call: None,
            flush_call: Some(0),
        });
        let error = unsafe { replace(&fake::MEMORY, address, &ORIGINAL, &PATCH) }.unwrap_err();
        assert!(matches!(error, Error::FlushFailed { .. }));
        assert_eq!(buffer.read(address, 6), ORIGINAL);
        for page in memory::pages(address, 6, buffer.page_size) {
            assert_eq!(fake::protection(page), Some(Protection::READ_EXECUTE));
        }
    }

    #[test]
    fn relock_failure_rolls_back_and_relocks_every_page() {
        let buffer = Buffer::new();
        let address = buffer.straddling();
        fake::inject(Faults {
            protect_call: Some(2),
            flush_call: None,
        });
        let error = unsafe { replace(&fake::MEMORY, address, &ORIGINAL, &PATCH) }.unwrap_err();
        assert!(matches!(error, Error::ProtectFailed { .. }));
        assert_eq!(buffer.read(address, 6), ORIGINAL);
        for page in memory::pages(address, 6, buffer.page_size) {
            assert_eq!(fake::protection(page), Some(Protection::READ_EXECUTE));
        }
    }
}
