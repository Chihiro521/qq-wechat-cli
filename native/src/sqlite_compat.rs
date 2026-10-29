//! PCQQ shifts SQLite's pending byte by its 1024-byte extended header.
//! Our engine executes SQLite on one thread. Scope the process-local setting to
//! QQ source operations; every index operation continues to use standard SQLite.
use std::{marker::PhantomData, rc::Rc};
pub const STANDARD: u32 = 0x40000000;
pub const PCQQ: u32 = STANDARD - 1024;
pub struct PendingByte {
    previous: u32,
    _not_send: PhantomData<Rc<()>>,
}
impl PendingByte {
    pub fn new(value: u32) -> Self {
        assert!(value == STANDARD || value == PCQQ);
        let previous = unsafe {
            rusqlite::ffi::sqlite3_test_control(rusqlite::ffi::SQLITE_TESTCTRL_PENDING_BYTE, value)
        } as u32;
        Self {
            previous,
            _not_send: PhantomData,
        }
    }
}
impl Drop for PendingByte {
    fn drop(&mut self) {
        unsafe {
            rusqlite::ffi::sqlite3_test_control(
                rusqlite::ffi::SQLITE_TESTCTRL_PENDING_BYTE,
                self.previous,
            );
        }
    }
}
pub fn standard<T>(action: impl FnOnce() -> T) -> T {
    let _guard = PendingByte::new(STANDARD);
    action()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_byte_is_scoped() {
        let read = || unsafe {
            rusqlite::ffi::sqlite3_test_control(rusqlite::ffi::SQLITE_TESTCTRL_PENDING_BYTE, 0u32)
        } as u32;
        assert_eq!(read(), STANDARD);
        {
            let _guard = PendingByte::new(PCQQ);
            assert_eq!(read(), PCQQ);
            standard(|| assert_eq!(read(), STANDARD));
            assert_eq!(read(), PCQQ);
        }
        assert_eq!(read(), STANDARD);
    }
}
