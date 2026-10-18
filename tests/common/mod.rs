//! Shared helpers for the conformance suite (ported from the SDK lane's trap-conformance spike).
#![cfg(unix)]
#![allow(dead_code)]

use dirty_ranges::{Range, Region};
use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// A file-backed shared mapping, the shape the crate exists for.
pub struct Mapped {
    _map: MmapMut,
    path: PathBuf,
    pub base: *mut u8,
    pub len: usize,
    pub page: usize,
}

unsafe impl Send for Mapped {}
unsafe impl Sync for Mapped {}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl Mapped {
    pub fn new(pages: usize) -> Mapped {
        let page = page_size();
        let len = pages * page;
        let path = std::env::temp_dir().join(format!(
            "dirty-ranges-conf-{}-{}.mem",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.set_len(len as u64).unwrap();
        let mut map = unsafe { MmapMut::map_mut(&file).unwrap() };
        let base = map.as_mut_ptr();
        Mapped {
            _map: map,
            path,
            base,
            len,
            page,
        }
    }

    pub fn arm(&self, pool_pages: usize) -> Region {
        unsafe { Region::arm(self.base, self.len, pool_pages) }.unwrap()
    }

    pub fn live(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.base, self.len) }
    }

    pub fn store(&self, offset: usize, value: u8) {
        assert!(offset < self.len);
        unsafe { self.base.add(offset).write_volatile(value) };
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Copy the live bytes at `ranges` into `mirror`. After the writers stop and one final flush, `mirror`
/// equals the live bytes exactly when no write was missed.
pub fn apply(mirror: &mut [u8], live: &[u8], ranges: &[Range]) {
    for r in ranges {
        mirror[r.offset..r.offset + r.len].copy_from_slice(&live[r.offset..r.offset + r.len]);
    }
}

pub fn rnd(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

pub fn errno_ptr() -> *mut libc::c_int {
    unsafe {
        #[cfg(target_os = "linux")]
        return libc::__errno_location();
        #[cfg(target_os = "android")]
        return libc::__errno();
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "dragonfly"
        ))]
        return libc::__error();
        #[cfg(any(target_os = "netbsd", target_os = "openbsd"))]
        return libc::__errno();
    }
}

/// How a child-process case ended.
#[derive(Debug)]
pub enum Outcome {
    /// The body returned and printed the success marker.
    Passed { output: String },
    /// The process ended without the marker: killed by a signal, aborted, or a failed assertion.
    Died {
        signal: Option<i32>,
        code: Option<i32>,
        output: String,
    },
    /// It did not finish within the time limit and was killed.
    Hung { output: String },
}

impl Outcome {
    pub fn output(&self) -> &str {
        match self {
            Outcome::Passed { output }
            | Outcome::Died { output, .. }
            | Outcome::Hung { output } => output,
        }
    }

    /// Ended by the signal a fault raises (SIGSEGV 11, or SIGBUS: 7 on Linux, 10 on macOS and the BSDs).
    pub fn died_by_fault(&self) -> bool {
        matches!(
            self,
            Outcome::Died {
                signal: Some(11 | 10 | 7),
                ..
            }
        )
    }
}

const MARKER: &str = "CASE-BODY-FINISHED";

/// Run `body` in a child process (some cases must die, and the fault handler is process-global).
///
/// `test_name` is the calling `#[test]` function; `label` names this case within it (a test may run several).
/// In the parent this re-runs the test binary filtered to that one test, with an environment variable naming
/// the label, and reports how the child ended. In the child (the variable is set) it runs `body` only if
/// `label` matches, prints a marker when `body` returns, and yields `None`, so the caller can tell it is the
/// child and skip its assertions.
pub fn in_child(
    test_name: &str,
    label: &str,
    limit: Duration,
    body: impl FnOnce(),
) -> Option<Outcome> {
    if let Ok(wanted) = std::env::var("DIRTY_RANGES_CHILD") {
        if wanted == label {
            body();
            println!("{MARKER}");
        }
        return None;
    }
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env("DIRTY_RANGES_CHILD", label)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Drain both pipes on threads so a chatty child cannot block on a full pipe while we poll.
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut output = t_out.join().unwrap();
    output.push_str(&t_err.join().unwrap());
    Some(match status {
        None => Outcome::Hung { output },
        Some(st) if output.contains(MARKER) && st.success() => Outcome::Passed { output },
        Some(st) => {
            use std::os::unix::process::ExitStatusExt;
            Outcome::Died {
                signal: st.signal(),
                code: st.code(),
                output,
            }
        }
    })
}
