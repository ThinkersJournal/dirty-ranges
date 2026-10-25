// SPDX-License-Identifier: MIT OR Apache-2.0
//! The tracked mapping on a file system other than the usual disk one. The suite maps files under
//! `std::env::temp_dir()`, which honours `TMPDIR`, so CI runs the whole suite with `TMPDIR=/dev/shm` (tmpfs) and
//! sets `DIRTY_RANGES_EXPECT_TMPFS=1`; this file then checks that the temp directory really is tmpfs and that the
//! basic arm / dirty / flush cycle works on a file mapped from it. Without the variable the cycle still runs on
//! whatever file system the temp directory is on, and no tmpfs claim is made.
#![cfg(target_os = "linux")]

mod common;

use common::*;
use dirty_ranges::Range;

/// `TMPFS_MAGIC` from `linux/magic.h`.
const TMPFS_MAGIC: i64 = 0x0102_1994;

fn fs_magic(dir: &std::path::Path) -> i64 {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).unwrap();
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(path.as_ptr(), &mut st) };
    assert_eq!(rc, 0, "statfs {dir:?}: {}", get_os_error());
    st.f_type as i64
}

#[test]
fn the_temp_directory_is_tmpfs_when_the_job_says_so() {
    let dir = std::env::temp_dir();
    let magic = fs_magic(&dir);
    eprintln!("temp dir {dir:?}: statfs f_type {magic:#x}");
    if std::env::var_os("DIRTY_RANGES_EXPECT_TMPFS").is_some() {
        assert_eq!(magic, TMPFS_MAGIC, "{dir:?} is not tmpfs");
    }
}

#[test]
fn arm_dirty_flush_works_on_a_file_mapped_from_the_temp_directory() {
    let page = page_size();
    let m = Mapped::new(8);
    // `arm` tracks pool_pages / 2 pages, so 16 covers all 8.
    let mut region = m.arm(16);
    m.store(page + 5, 0x11);
    m.store(6 * page, 0x22);
    let r = |offset, len| Range { offset, len };
    assert_eq!(region.flush(), vec![r(page + 5, 1), r(6 * page, 1)]);
    // Flush re-protects: a second write to an already flushed page is seen again.
    m.store(page + 5, 0x33);
    assert_eq!(region.flush(), vec![r(page + 5, 1)]);
    assert!(region.flush().is_empty());
}
