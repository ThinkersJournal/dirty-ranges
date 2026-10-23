// SPDX-License-Identifier: MIT OR Apache-2.0
//! Conformance cases that must run in their own process: they end in a signal, install other fault handlers,
//! or overflow the stack. Ported from the SDK lane's spike (`spike/trap-conformance`, PR #3).
#![cfg(any(unix, windows))]

mod common;

use common::*;
use dirty_ranges::Range;
#[cfg(unix)]
use std::cell::UnsafeCell;
use std::io::Write as _;
#[cfg(unix)]
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(90);

#[cfg(unix)]
fn block_fault_signals() {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGSEGV);
        libc::sigaddset(&mut set, libc::SIGBUS);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// A store with SIGSEGV/SIGBUS blocked cannot be handled: the kernel kills the process (Linux with SIGSEGV,
/// macOS with SIGKILL). That is why `Region::arm` refuses such a thread, and why the docs say so.
#[cfg(unix)]
#[test]
fn a_write_from_a_thread_that_blocks_the_fault_signals_is_fatal() {
    let name = "a_write_from_a_thread_that_blocks_the_fault_signals_is_fatal";
    let Some(outcome) = in_child(name, name, LIMIT, || {
        let m = Arc::new(Mapped::new(4));
        let _region = m.arm(16);
        let m2 = m.clone();
        std::thread::spawn(move || {
            block_fault_signals();
            m2.store(0, 1);
        })
        .join()
        .ok();
    }) else {
        return;
    };
    eprintln!("blocked-signal write outcome: {outcome:?}");
    // On the GitHub macos-26-arm64 runner image the store hangs instead of being killed (run 39182659564);
    // the spike saw a kill elsewhere. A hang is worse than fatal, so it is accepted on macOS only, and the
    // outcome is printed above so it is never a silent pass. Documented as a limit, not fixed.
    if cfg!(target_os = "macos") && matches!(outcome, Outcome::Hung { .. }) {
        return;
    }
    assert!(
        outcome.died_by_fault()
            || matches!(
                outcome,
                Outcome::Died {
                    signal: Some(9),
                    ..
                }
            ),
        "expected a fatal signal: {outcome:?}"
    );
}

// ---- another fault handler in the chain

static OTHER_BASE: AtomicUsize = AtomicUsize::new(0);
static OTHER_HITS: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
struct Previous(UnsafeCell<[MaybeUninit<libc::sigaction>; 2]>);
// SAFETY: written once by `install_other_handler` before the handler can run, read-only after.
#[cfg(unix)]
unsafe impl Sync for Previous {}
#[cfg(unix)]
static OTHER_PREVIOUS: Previous = Previous(UnsafeCell::new([
    MaybeUninit::uninit(),
    MaybeUninit::uninit(),
]));

#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe fn fault_address(info: *mut libc::siginfo_t) -> usize {
    unsafe { (*info).si_addr() as usize }
}
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
unsafe fn fault_address(info: *mut libc::siginfo_t) -> usize {
    unsafe { (*info).si_addr as usize }
}

/// A stand-in for another runtime's handler (a JVM, a sanitizer): it handles faults in its own page and
/// passes everything else to whoever was installed before it.
#[cfg(unix)]
unsafe extern "C" fn other_handler(
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ctx: *mut libc::c_void,
) {
    unsafe {
        let addr = fault_address(info);
        let base = OTHER_BASE.load(Ordering::Acquire);
        let page = page_size();
        if addr >= base && addr < base + page {
            let raised = libc::mprotect(
                base as *mut libc::c_void,
                page,
                libc::PROT_READ | libc::PROT_WRITE,
            );
            if raised != 0 {
                // Returning now would re-run the store and fault forever; say so before that happens.
                let msg = b"OTHER-MPROTECT-FAILED
";
                libc::write(1, msg.as_ptr().cast(), msg.len());
            }
            OTHER_HITS.fetch_add(1, Ordering::SeqCst);
            return;
        }
        let msg = b"OTHER-SAW-UNRELATED\n";
        libc::write(1, msg.as_ptr().cast(), msg.len());
        let old = (*OTHER_PREVIOUS.0.get())[usize::from(sig != libc::SIGSEGV)].assume_init_ref();
        let h = old.sa_sigaction;
        if h == libc::SIG_DFL || h == libc::SIG_IGN {
            libc::signal(sig, libc::SIG_DFL);
        } else if old.sa_flags & libc::SA_SIGINFO != 0 {
            let f: unsafe extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                std::mem::transmute(h);
            f(sig, info, ctx);
        } else {
            let f: unsafe extern "C" fn(libc::c_int) = std::mem::transmute(h);
            f(sig);
        }
    }
}

#[cfg(unix)]
fn install_other_handler() {
    unsafe {
        let previous = OTHER_PREVIOUS
            .0
            .get()
            .cast::<MaybeUninit<libc::sigaction>>();
        for (i, sig) in [libc::SIGSEGV, libc::SIGBUS].into_iter().enumerate() {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = other_handler as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER | libc::SA_ONSTACK;
            libc::sigemptyset(&mut action.sa_mask);
            assert_eq!(
                libc::sigaction(sig, &action, (*previous.add(i)).as_mut_ptr()),
                0
            );
        }
    }
}

#[cfg(unix)]
fn other_page() {
    unsafe {
        // Mapped readable and writable, then made inaccessible: on NetBSD a mapping created PROT_NONE may not be
        // raised again, which is not what these cases are about.
        let p = libc::mmap(
            std::ptr::null_mut(),
            page_size(),
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        assert_ne!(p, libc::MAP_FAILED);
        assert_eq!(libc::mprotect(p, page_size(), libc::PROT_NONE), 0);
        OTHER_BASE.store(p as usize, Ordering::Release);
    }
}

#[cfg(windows)]
unsafe extern "system" fn other_veh(
    info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    unsafe {
        let record = (*info).ExceptionRecord;
        if (*record).ExceptionCode != 0xC000_0005_u32 as i32 {
            return 0;
        }
        let addr = (*record).ExceptionInformation[1];
        let base = OTHER_BASE.load(Ordering::Acquire);
        let page = page_size();
        if addr >= base && addr < base + page {
            let mut old = 0;
            VirtualProtect(base as *const _, page, PAGE_READWRITE, &mut old);
            OTHER_HITS.fetch_add(1, Ordering::SeqCst);
            return -1;
        }
        let _ = std::io::stdout().write_all(
            b"OTHER-SAW-UNRELATED
",
        );
        let _ = std::io::stdout().flush();
        0
    }
}

#[cfg(windows)]
fn install_other_handler() {
    use windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler;
    unsafe {
        assert!(!AddVectoredExceptionHandler(1, Some(other_veh)).is_null());
    }
}

#[cfg(windows)]
fn other_page() {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_NOACCESS, VirtualAlloc,
    };
    unsafe {
        let p = VirtualAlloc(
            std::ptr::null(),
            page_size(),
            MEM_COMMIT | MEM_RESERVE,
            PAGE_NOACCESS,
        );
        assert!(!p.is_null());
        OTHER_BASE.store(p as usize, Ordering::Release);
    }
}

/// Progress marker for the parent to print if this child hangs (a hung child's output is all we get).
fn step(what: &str) {
    println!("STEP {what}");
    let _ = std::io::stdout().flush();
}

/// Whether the OS lets `mprotect` raise a mapping that was created `PROT_NONE`. NetBSD does not (`EACCES`, run
/// 39378372000): the chaining cases once relied on it and hung there, so they now map read/write first. The
/// outcome is printed on every OS; only NetBSD is allowed to refuse.
#[cfg(unix)]
#[test]
fn mprotect_can_raise_a_mapping_created_prot_none() {
    unsafe {
        let page = page_size();
        let p = libc::mmap(
            std::ptr::null_mut(),
            page,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        assert_ne!(p, libc::MAP_FAILED);
        let raised = libc::mprotect(p, page, libc::PROT_READ | libc::PROT_WRITE);
        let error = std::io::Error::last_os_error();
        if raised == 0 {
            eprintln!("mprotect up from a PROT_NONE mapping: works");
        } else {
            eprintln!("mprotect up from a PROT_NONE mapping: refused: {error}");
        }
        assert!(
            raised == 0 || cfg!(target_os = "netbsd"),
            "mprotect up from PROT_NONE failed: {error}"
        );
        libc::munmap(p, page);
    }
}

fn chain_body(ours_first: bool) {
    other_page();
    step("other page mapped");
    let m = Mapped::new(4);
    step("region mapped");
    let mut region;
    if ours_first {
        region = m.arm(16);
        step("ours installed");
        install_other_handler();
        step("other installed");
    } else {
        install_other_handler();
        step("other installed");
        region = m.arm(16);
        step("ours installed");
    }
    m.store(0, 1);
    step("trapped store done");
    assert_eq!(
        region.flush(),
        vec![Range { offset: 0, len: 1 }],
        "a trapped store did not reach our handler"
    );
    step("flushed");
    unsafe { std::ptr::write_volatile(OTHER_BASE.load(Ordering::Acquire) as *mut u8, 1) };
    step("other store done");
    assert_eq!(
        OTHER_HITS.load(Ordering::SeqCst),
        1,
        "the other handler's own fault was not handled"
    );
    println!("TRAPPED-OK OTHER-OK");
    let _ = std::io::stdout().flush();
    // An unrelated fault must reach the other handler and then the default action, which kills us.
    unsafe { std::ptr::write_volatile(std::hint::black_box(16usize) as *mut u8, 1) };
}

fn assert_chain(outcome: Outcome) {
    let output = outcome.output();
    assert!(
        output.contains("TRAPPED-OK OTHER-OK")
            && output.contains("OTHER-SAW-UNRELATED")
            && outcome.died_by_fault(),
        "{outcome:?}"
    );
}

#[test]
fn chaining_with_the_other_handler_installed_after_ours() {
    let name = "chaining_with_the_other_handler_installed_after_ours";
    if let Some(o) = in_child(name, name, LIMIT, || chain_body(true)) {
        assert_chain(o);
    }
}

#[test]
fn chaining_with_the_other_handler_installed_before_ours() {
    let name = "chaining_with_the_other_handler_installed_before_ours";
    if let Some(o) = in_child(name, name, LIMIT, || chain_body(false)) {
        assert_chain(o);
    }
}

// ---- stack overflow

#[allow(
    unconditional_recursion,
    reason = "the child recurses without end on purpose, to overflow its stack"
)]
fn recurse(n: u64) -> u64 {
    let a = [n as u8; 1024];
    std::hint::black_box(&a);
    recurse(n + 1) + u64::from(a[3])
}

fn overflow_body(armed: bool, in_thread: bool) {
    let m = Arc::new(Mapped::new(8));
    let _region = armed.then(|| m.arm(32));
    let stop = Arc::new(AtomicBool::new(false));
    let writer = (armed && in_thread).then(|| {
        let (m, stop) = (m.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut s = 12345u64;
            while !stop.load(Ordering::Relaxed) {
                m.store((rnd(&mut s) as usize) % m.len, 1);
            }
        })
    });
    if in_thread {
        std::thread::Builder::new()
            .name("overflow".into())
            .spawn(|| recurse(0))
            .unwrap()
            .join()
            .ok();
    } else {
        recurse(0);
    }
    stop.store(true, Ordering::SeqCst);
    drop(writer);
}

/// With the trap armed, a stack overflow must end exactly as it does without the trap: same signal, same
/// message. (Our handler sees the guard-page fault, finds it is not in a region, and passes it on.)
fn assert_overflow_like_control(name: &str, in_thread: bool) {
    let control = in_child(name, "control", LIMIT, move || {
        overflow_body(false, in_thread)
    });
    let ours = in_child(name, "ours", LIMIT, move || overflow_body(true, in_thread));
    let (Some(control), Some(ours)) = (control, ours) else {
        return;
    };
    let message = |o: &Outcome| o.output().contains("overflowed its stack");
    let shape = |o: &Outcome| match o {
        Outcome::Died { signal, code, .. } => Some((*signal, *code)),
        _ => None,
    };
    assert!(
        shape(&control).is_some(),
        "the control did not die: {control:?}"
    );
    assert_eq!(
        shape(&ours),
        shape(&control),
        "control: {control:?}\nours: {ours:?}"
    );
    assert_eq!(
        message(&ours),
        message(&control),
        "control: {control:?}\nours: {ours:?}"
    );
}

#[test]
fn stack_overflow_on_the_main_thread_behaves_as_without_the_trap() {
    assert_overflow_like_control(
        "stack_overflow_on_the_main_thread_behaves_as_without_the_trap",
        false,
    );
}

#[test]
fn stack_overflow_on_a_thread_while_others_write_behaves_as_without_the_trap() {
    assert_overflow_like_control(
        "stack_overflow_on_a_thread_while_others_write_behaves_as_without_the_trap",
        true,
    );
}

#[cfg(dirty_ranges_conformance)]
mod controls {
    use super::*;
    use dirty_ranges::conformance_hooks::FAIL_PROTECT;

    /// When the handler cannot unprotect a page it must die with the fault, not loop forever on it.
    #[test]
    fn a_failed_protect_in_the_handler_kills_the_process() {
        let name = "controls::a_failed_protect_in_the_handler_kills_the_process";
        let Some(outcome) = in_child(name, name, LIMIT, || {
            let m = Mapped::new(4);
            let _region = m.arm(16);
            FAIL_PROTECT.store(true, Ordering::SeqCst);
            m.store(0, 1);
        }) else {
            return;
        };
        assert!(
            outcome.died_by_fault(),
            "expected death by the fault, not a hang or a pass: {outcome:?}"
        );
    }
}
