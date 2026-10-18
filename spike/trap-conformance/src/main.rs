//! Conformance harness for the page-fault write trap. `trap-conformance` runs every case in its own
//! child process (handlers are process-global; some cases are expected to die) and prints one RESULT line per case.
mod trap;

use memmap2::MmapMut;
use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use trap::*;

// ------------------------------------------------------------------ allocator that faults inside its own critical section
struct Hammer;
static HAMMER_ON: AtomicBool = AtomicBool::new(false);
static HAMMER_MOD: AtomicU64 = AtomicU64::new(32);
static HAMMER_LOCK: AtomicBool = AtomicBool::new(false);
static HAMMER_N: AtomicU64 = AtomicU64::new(0);
static HAMMER_WRITES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Hammer {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if HAMMER_ON.load(Ordering::Relaxed) {
            let n = HAMMER_N.fetch_add(1, Ordering::Relaxed);
            if n % HAMMER_MOD.load(Ordering::Relaxed) == 0 {
                while HAMMER_LOCK.swap(true, Ordering::Acquire) {
                    std::hint::spin_loop();
                }
                // fault INSIDE the allocator's critical section
                let base = BASE.load(Ordering::Acquire);
                let armed = ARMED.load(Ordering::Acquire);
                let page = PAGE.load(Ordering::Acquire);
                if base != 0 && armed != 0 {
                    let p = (n as usize / 32) % armed;
                    let off = p * page + (n as usize * 7) % page;
                    std::ptr::write_volatile((base + off) as *mut u8, (n as u8) | 1);
                    HAMMER_WRITES.fetch_add(1, Ordering::Relaxed);
                }
                HAMMER_LOCK.store(false, Ordering::Release);
            }
        }
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
}
#[global_allocator]
static A: Hammer = Hammer;

// ------------------------------------------------------------------ helpers
struct Env {
    _mmap: MmapMut,
    path: PathBuf,
    base: *mut u8,
    len: usize,
    page: usize,
    armed: usize,
}
unsafe impl Send for Env {}
unsafe impl Sync for Env {}
impl Drop for Env {
    fn drop(&mut self) {
        disarm_all();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn setup(npages: usize, pool_pages: usize) -> Env {
    let page = page_size();
    let len = npages * page;
    let path = std::env::temp_dir().join(format!(
        "trapconf-{}-{}.mem",
        std::process::id(),
        HAMMER_N.load(Ordering::Relaxed)
    ));
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    f.set_len(len as u64).unwrap();
    let mut m = unsafe { MmapMut::map_mut(&f).unwrap() };
    let base = m.as_mut_ptr();
    let armed = arm(base, len, pool_pages);
    Env {
        _mmap: m,
        path,
        base,
        len,
        page,
        armed,
    }
}

fn apply(mirror: &mut [u8], ranges: &[Range]) {
    for r in ranges {
        mirror[r.off..r.off + r.bytes.len()].copy_from_slice(&r.bytes);
    }
}
fn live(e: &Env) -> &[u8] {
    unsafe { std::slice::from_raw_parts(e.base, e.len) }
}
fn rnd(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}
fn ok<T: Into<String>>(s: T) -> Result<String, String> {
    Ok(s.into())
}
fn check(c: bool, m: impl Into<String>) -> Result<(), String> {
    if c {
        Ok(())
    } else {
        Err(m.into())
    }
}

fn hammer_run(neg: bool) -> Result<String, String> {
    let e = Arc::new(setup(64, 256));
    let mut mirror = vec![0u8; e.len];
    let mut fl = Flusher::new();
    HAMMER_MOD.store(if neg { 2 } else { 32 }, Ordering::Relaxed);
    if neg {
        H_ALLOC_IN_HANDLER.store(true, Ordering::Relaxed);
    }
    HAMMER_ON.store(true, Ordering::SeqCst);
    let stop = Arc::new(AtomicBool::new(false));
    let mut hs = vec![];
    for t in 0..4u64 {
        let (e, stop) = (e.clone(), stop.clone());
        hs.push(std::thread::spawn(move || {
            let mut s = 0x9E3779B97F4A7C15 ^ (t + 1);
            while !stop.load(Ordering::Relaxed) {
                let off = (rnd(&mut s) as usize) % e.len;
                unsafe { std::ptr::write_volatile(e.base.add(off), rnd(&mut s) as u8) };
            }
        }));
    }
    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    for t in 0..4u64 {
        let (stop, shared) = (stop.clone(), shared.clone());
        hs.push(std::thread::spawn(move || {
            let mut s = 0xD1B54A32D192ED03 ^ (t + 1);
            while !stop.load(Ordering::Relaxed) {
                let v: Vec<u8> = Vec::with_capacity((rnd(&mut s) % 512) as usize + 1);
                let b = Box::new(rnd(&mut s));
                let mut g = shared.lock().unwrap();
                g.push(*b as u8);
                if g.len() > 64 {
                    g.clear();
                }
                drop(g);
                drop(v);
            }
        }));
    }
    let t0 = Instant::now();
    let mut flushes = 0u32;
    while t0.elapsed() < Duration::from_secs(2) {
        let r = fl.flush();
        apply(&mut mirror, &r);
        flushes += 1;
        std::thread::sleep(Duration::from_micros(300));
    }
    stop.store(true, Ordering::SeqCst);
    for h in hs {
        h.join().unwrap();
    }
    HAMMER_ON.store(false, Ordering::SeqCst);
    let r = fl.flush();
    apply(&mut mirror, &r);
    check(
        mirror == live(&e),
        "mirror != live after final flush (lost update)",
    )?;
    check(FAULTS.load(Ordering::Relaxed) > 0, "no faults happened")?;
    check(
        HAMMER_WRITES.load(Ordering::Relaxed) > 0,
        "no faults were injected inside the allocator",
    )?;
    ok(format!(
        "flushes={} faults={} allocator-internal-writes={} handler-calls={}",
        flushes,
        FAULTS.load(Ordering::Relaxed),
        HAMMER_WRITES.load(Ordering::Relaxed),
        HANDLER_CALLS.load(Ordering::Relaxed)
    ))
}

#[cfg(unix)]
fn libc_lock() -> Result<String, String> {
    let e = Arc::new(setup(16, 64));
    unsafe {
        let f = libc::tmpfile();
        check(!f.is_null(), "tmpfile failed")?;
        let rc = libc::setvbuf(f, e.base as *mut libc::c_char, libc::_IOFBF, 4 * e.page);
        check(rc == 0, "setvbuf into trapped region failed")?;
        let fp = f as usize;
        let mut hs = vec![];
        for t in 0..4u8 {
            hs.push(std::thread::spawn(move || {
                let rec = [t + 1; 100];
                for _ in 0..400 {
                    unsafe {
                        libc::fwrite(
                            rec.as_ptr() as *const libc::c_void,
                            1,
                            rec.len(),
                            fp as *mut libc::FILE,
                        )
                    };
                }
            }));
        }
        for h in hs {
            h.join().unwrap();
        }
        libc::fflush(f);
        let sz = libc::ftell(f);
        check(
            sz == 4 * 400 * 100,
            format!("file size {} != {}", sz, 4 * 400 * 100),
        )?;
        check(
            FAULTS.load(Ordering::Relaxed) > 0,
            "no fault occurred inside stdio",
        )?;
        // detach the stream from the trapped buffer before teardown
        disarm_all();
        libc::fclose(f);
    }
    ok(format!(
        "faults inside libc FILE lock: {} handler-calls={}",
        FAULTS.load(Ordering::Relaxed),
        HANDLER_CALLS.load(Ordering::Relaxed)
    ))
}

fn double_buffer(mutate: bool) -> Result<String, String> {
    if mutate {
        H_MUTATE_SKIP_PROTECT.store(true, Ordering::SeqCst);
    }
    let e = Arc::new(setup(8, 16));
    let mut mirror = vec![0u8; e.len];
    let mut fl = Flusher::new();
    let stop = Arc::new(AtomicBool::new(false));
    let mut hs = vec![];
    for t in 0..4u64 {
        let (e, stop) = (e.clone(), stop.clone());
        hs.push(std::thread::spawn(move || {
            let mut s = 0xA076_1D64_78BD_642F ^ (t + 1);
            while !stop.load(Ordering::Relaxed) {
                // hammer the same 3 pages so stores race the flush window
                let off = (rnd(&mut s) as usize) % (3 * e.page);
                unsafe { std::ptr::write_volatile(e.base.add(off), rnd(&mut s) as u8) };
            }
        }));
    }
    let t0 = Instant::now();
    let mut n = 0;
    while t0.elapsed() < Duration::from_secs(2) {
        let r = fl.flush();
        apply(&mut mirror, &r);
        n += 1;
    }
    stop.store(true, Ordering::SeqCst);
    for h in hs {
        h.join().unwrap();
    }
    let r = fl.flush();
    apply(&mut mirror, &r);
    check(
        mirror == live(&e),
        "mirror != live: a store racing the flush window was lost",
    )?;
    let (clean, busy, armed) = states_summary();
    check(
        clean == armed && busy == 0,
        format!(
            "pages left dirty/busy: clean={} busy={} armed={}",
            clean, busy, armed
        ),
    )?;
    ok(format!(
        "flushes={} faults={} (every flush raced live writers; mirror==live)",
        n,
        FAULTS.load(Ordering::Relaxed)
    ))
}

fn pool_exhaustion() -> Result<String, String> {
    let e = setup(16, 8); // 8 buffers => 4 armed pages
    check(e.armed == 4, format!("armed={} expected 4", e.armed))?;
    let mut mirror = vec![0u8; e.len];
    let mut fl = Flusher::new();
    let f0 = FAULTS.load(Ordering::Relaxed);
    unsafe {
        *e.base.add(10) = 1; // armed
        *e.base.add(e.page * 3 + 5) = 2; // armed
        *e.base.add(e.page * 6) = 3; // NOT armed
        *e.base.add(e.page * 15 + 1) = 4; // NOT armed
    }
    check(
        FAULTS.load(Ordering::Relaxed) - f0 == 2,
        "unarmed pages must not fault, armed pages must",
    )?;
    let r = fl.flush();
    check(
        r.len() == 2,
        format!("expected 2 ranges from armed pages, got {}", r.len()),
    )?;
    apply(&mut mirror, &r);
    check(
        mirror != live(&e),
        "uncovered-page writes unexpectedly tracked",
    )?;
    // scan fallback for the uncovered pages
    let mut fixed = 0;
    for p in e.armed..e.len / e.page {
        let (a, b) = (p * e.page, (p + 1) * e.page);
        if mirror[a..b] != live(&e)[a..b] {
            mirror[a..b].copy_from_slice(&live(&e)[a..b]);
            fixed += 1;
        }
    }
    check(
        mirror == live(&e) && fixed == 2,
        format!("scan fallback failed fixed={}", fixed),
    )?;
    // second epoch on armed pages after double-buffer swap
    unsafe { *e.base.add(11) = 9 };
    let r2 = fl.flush();
    check(
        r2.len() == 1 && r2[0].off == 11 && r2[0].bytes == [9],
        "second epoch diff wrong",
    )?;
    ok("armed=4 of 16 pages; uncovered pages untracked (reported to caller), scan fallback converged, 2nd epoch ok")
}

fn page_size_case() -> Result<String, String> {
    let page = page_size();
    check(
        page.is_power_of_two() && page >= 4096,
        format!("odd page size {}", page),
    )?;
    let e = setup(6, 16);
    check((e.base as usize) % page == 0, "mapping not page aligned")?;
    let mut fl = Flusher::new();
    unsafe {
        *e.base.add(2 * page - 1) = 0xAA;
        *e.base.add(2 * page) = 0xBB;
        *e.base.add(5 * page - 1) = 0xCC;
    }
    let r = fl.flush();
    check(r.len() == 2, format!("expected 2 ranges got {}", r.len()))?;
    check(
        r[0].off == 2 * page - 1 && r[0].bytes == [0xAA, 0xBB],
        "boundary-straddling range wrong",
    )?;
    check(
        r[1].off == 5 * page - 1 && r[1].bytes == [0xCC],
        "last-byte range wrong",
    )?;
    ok(format!(
        "page_size={} hardcoded-4096-would-be-wrong={}",
        page,
        page != 4096
    ))
}

fn errno_case() -> Result<String, String> {
    let e = setup(4, 16);
    #[cfg(unix)]
    unsafe {
        let ep = errno_ptr_pub();
        *ep = 777;
        *e.base = 1;
        check(*ep == 777, "plain fault changed errno")?;
        H_CLOBBER_ERRNO.store(true, Ordering::SeqCst);
        *ep = 777;
        *e.base.add(e.page) = 1;
        let kept = *ep;
        H_NO_ERRNO_RESTORE.store(true, Ordering::SeqCst);
        *ep = 777;
        *e.base.add(2 * e.page) = 1;
        let lost = *ep;
        check(
            kept == 777,
            format!("restore failed: errno={} after clobbering handler", kept),
        )?;
        check(
            lost != 777,
            "control: with restore disabled errno should have been clobbered",
        )?;
        return ok(format!(
            "errno preserved across fault (clobber hook on): 777; control without restore: {}",
            lost
        ));
    }
    #[cfg(windows)]
    unsafe {
        set_last_error(777);
        *e.base = 1;
        check(last_error() == 777, "plain fault changed GetLastError")?;
        H_CLOBBER_ERRNO.store(true, Ordering::SeqCst);
        set_last_error(777);
        *e.base.add(e.page) = 1;
        let kept = last_error();
        H_NO_ERRNO_RESTORE.store(true, Ordering::SeqCst);
        set_last_error(777);
        *e.base.add(2 * e.page) = 1;
        let lost = last_error();
        check(kept == 777, format!("restore failed: last_error={}", kept))?;
        check(lost != 777, "control: should be clobbered")?;
        return ok(format!(
            "GetLastError preserved across fault; control without restore: {}",
            lost
        ));
    }
}

#[cfg(unix)]
fn sigsegv_blocked_here() -> bool {
    unsafe {
        let mut cur: libc::sigset_t = std::mem::zeroed();
        libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut cur);
        libc::sigismember(&cur, libc::SIGSEGV) == 1 || libc::sigismember(&cur, libc::SIGBUS) == 1
    }
}
#[cfg(unix)]
fn block_segv() {
    unsafe {
        let mut s: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut s);
        libc::sigaddset(&mut s, libc::SIGSEGV);
        libc::sigaddset(&mut s, libc::SIGBUS);
        libc::pthread_sigmask(libc::SIG_BLOCK, &s, std::ptr::null_mut());
    }
}
#[cfg(unix)]
fn sigmask_guard() -> Result<String, String> {
    let _e = setup(4, 16);
    let normal = sigsegv_blocked_here();
    let blocked = std::thread::spawn(|| {
        block_segv();
        sigsegv_blocked_here()
    })
    .join()
    .unwrap();
    check(
        !normal && blocked,
        format!("guard wrong: normal={} blocked={}", normal, blocked),
    )?;
    ok("guard detects SIGSEGV/SIGBUS blocked in the calling thread (crate must refuse to arm/flush from such a thread and document it)")
}
#[cfg(unix)]
fn sigmask_fatal() -> Result<String, String> {
    let e = Arc::new(setup(4, 16));
    let e2 = e.clone();
    std::thread::spawn(move || {
        block_segv();
        unsafe { *e2.base = 1 }; // blocked synchronous SIGSEGV => kernel kills the process
    })
    .join()
    .ok();
    Err("survived a store with SIGSEGV blocked (unexpected)".into())
}

fn protect_fail() -> Result<String, String> {
    let e = setup(4, 16);
    H_FAIL_PROTECT.store(true, Ordering::SeqCst);
    unsafe { *e.base = 1 };
    Err("store succeeded although the handler's protect failed (unexpected)".into())
}

// ---- chaining
static L_BASE: AtomicUsize = AtomicUsize::new(0);
static L_HITS: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
mod lh {
    use super::*;
    pub static mut LOLD: [std::mem::MaybeUninit<libc::sigaction>; 2] = [
        std::mem::MaybeUninit::uninit(),
        std::mem::MaybeUninit::uninit(),
    ];
    pub unsafe extern "C" fn l_handler(
        sig: libc::c_int,
        info: *mut libc::siginfo_t,
        ctx: *mut libc::c_void,
    ) {
        let a = si_addr_pub(info);
        let lb = L_BASE.load(Ordering::Acquire);
        if a >= lb && a < lb + 4096 {
            libc::mprotect(
                lb as *mut libc::c_void,
                page_size(),
                libc::PROT_READ | libc::PROT_WRITE,
            );
            L_HITS.fetch_add(1, Ordering::SeqCst);
            return;
        }
        let m = b"L-SAW-UNRELATED\n";
        libc::write(1, m.as_ptr() as *const libc::c_void, m.len());
        let i = if sig == libc::SIGSEGV { 0 } else { 1 };
        #[allow(static_mut_refs)]
        trap_chain(LOLD[i].assume_init_ref(), sig, info, ctx);
    }
    pub fn l_install() {
        unsafe {
            for (i, s) in SIGS_PUB.iter().enumerate() {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = l_handler as *const () as usize;
                sa.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER | libc::SA_ONSTACK;
                libc::sigemptyset(&mut sa.sa_mask);
                #[allow(static_mut_refs)]
                libc::sigaction(*s, &sa, LOLD[i].as_mut_ptr());
            }
        }
    }
    pub fn l_page() {
        unsafe {
            let p = libc::mmap(
                std::ptr::null_mut(),
                page_size(),
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            );
            assert!(p != libc::MAP_FAILED);
            L_BASE.store(p as usize, Ordering::Release);
        }
    }
}
#[cfg(unix)]
use trap::chain_pub as trap_chain;

#[cfg(windows)]
mod lh {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Debug::*;
    use windows_sys::Win32::System::Memory::*;
    unsafe extern "system" fn l_veh(info: *mut EXCEPTION_POINTERS) -> i32 {
        let rec = (*info).ExceptionRecord;
        if (*rec).ExceptionCode != 0xC0000005u32 as i32 {
            return 0;
        }
        let a = (*rec).ExceptionInformation[1];
        let lb = L_BASE.load(Ordering::Acquire);
        if a >= lb && a < lb + 4096 {
            let mut old = 0;
            VirtualProtect(lb as _, 4096, PAGE_READWRITE, &mut old);
            L_HITS.fetch_add(1, Ordering::SeqCst);
            return -1;
        }
        let _ = std::io::stdout().write_all(b"L-SAW-UNRELATED\n");
        let _ = std::io::stdout().flush();
        0
    }
    pub fn l_install() {
        unsafe {
            assert!(!AddVectoredExceptionHandler(1, Some(l_veh)).is_null());
        }
    }
    pub fn l_page() {
        unsafe {
            let p = VirtualAlloc(
                std::ptr::null(),
                4096,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_NOACCESS,
            );
            assert!(!p.is_null());
            L_BASE.store(p as usize, Ordering::Release);
        }
    }
}

fn chain_child(ours_first: bool) -> Result<String, String> {
    lh::l_page();
    let e;
    if ours_first {
        e = setup(4, 16);
        lh::l_install();
    } else {
        lh::l_install();
        e = setup(4, 16);
    }
    unsafe { *e.base = 1 };
    check(
        FAULTS.load(Ordering::Relaxed) == 1,
        "trapped store did not reach our handler",
    )?;
    unsafe { std::ptr::write_volatile(L_BASE.load(Ordering::Acquire) as *mut u8, 1) };
    check(
        L_HITS.load(Ordering::SeqCst) == 1,
        "the other handler's own fault was not handled",
    )?;
    println!("TRAP_OK L_OK");
    let _ = std::io::stdout().flush();
    unsafe { std::ptr::write_volatile(std::hint::black_box(16usize) as *mut u8, 1) }; // unrelated null-ish deref
    Err("unrelated fault did not kill the process".into())
}

#[allow(unconditional_recursion)]
fn recurse(n: u64) -> u64 {
    let a = [n as u8; 1024];
    std::hint::black_box(&a);
    recurse(n + 1) + a[3] as u64
}
fn so_child(ours: bool, thread: bool) -> Result<String, String> {
    let env = if ours {
        Some(Arc::new(setup(8, 32)))
    } else {
        None
    };
    let stop = Arc::new(AtomicBool::new(false));
    let mut w = None;
    if let Some(e) = &env {
        if thread {
            let (e, stop) = (e.clone(), stop.clone());
            w = Some(std::thread::spawn(move || {
                let mut s = 12345u64;
                while !stop.load(Ordering::Relaxed) {
                    let off = (rnd(&mut s) as usize) % e.len;
                    unsafe { std::ptr::write_volatile(e.base.add(off), 1) };
                }
            }));
        } else {
            unsafe { *e.base = 1 };
        }
    }
    if thread {
        std::thread::Builder::new()
            .name("so-thread".into())
            .spawn(|| recurse(0))
            .unwrap()
            .join()
            .ok();
    } else {
        recurse(0);
    }
    stop.store(true, Ordering::SeqCst);
    drop(w);
    Err("stack overflow did not kill the process".into())
}

// ------------------------------------------------------------------ child dispatch
fn run_case(name: &str) -> Result<String, String> {
    match name {
        "alloc_hammer" => hammer_run(false),
        "alloc_hammer_negative_control" => hammer_run(true),
        #[cfg(unix)]
        "libc_lock_fault" => libc_lock(),
        "double_buffer_race" => double_buffer(false),
        "double_buffer_mutation_control" => double_buffer(true),
        "pool_exhaustion" => pool_exhaustion(),
        "page_size" => page_size_case(),
        "errno_restore" => errno_case(),
        #[cfg(unix)]
        "sigmask_guard" => sigmask_guard(),
        #[cfg(unix)]
        "sigmask_blocked_is_fatal" => sigmask_fatal(),
        "protect_failure_is_loud" => protect_fail(),
        "chain_ours_first" => chain_child(true),
        "chain_other_first" => chain_child(false),
        "so_main_control" => so_child(false, false),
        "so_main_ours" => so_child(true, false),
        "so_thread_control" => so_child(false, true),
        "so_thread_ours_trapped" => so_child(true, true),
        _ => Err(format!("unknown case {}", name)),
    }
}

// ------------------------------------------------------------------ runner
struct Out {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

fn spawn_case(name: &str, timeout: Duration) -> Out {
    let exe = std::env::current_exe().unwrap();
    let dir = std::env::temp_dir();
    let (po, pe) = (
        dir.join(format!("tc-{}-{}.out", std::process::id(), name)),
        dir.join(format!("tc-{}-{}.err", std::process::id(), name)),
    );
    let mut ch = Command::new(exe)
        .args(["--case", name])
        .stdout(Stdio::from(std::fs::File::create(&po).unwrap()))
        .stderr(Stdio::from(std::fs::File::create(&pe).unwrap()))
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(s) = ch.try_wait().unwrap() {
            break s;
        }
        if t0.elapsed() > timeout {
            timed_out = true;
            ch.kill().ok();
            break ch.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let o = Out {
        status,
        stdout: std::fs::read_to_string(&po).unwrap_or_default(),
        stderr: std::fs::read_to_string(&pe).unwrap_or_default(),
        timed_out,
    };
    let _ = std::fs::remove_file(po);
    let _ = std::fs::remove_file(pe);
    o
}

fn sig_of(s: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sg) = s.signal() {
            return format!("signal {}", sg);
        }
    }
    match s.code() {
        Some(c) => format!("exit 0x{:X}", c as u32),
        None => "?".into(),
    }
}
fn died_by_fault(s: &std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        return matches!(s.signal(), Some(11) | Some(10) | Some(7)); // SEGV, BUS(linux 7 / bsd+mac 10)
    }
    #[cfg(windows)]
    {
        return s.code().map(|c| c as u32 == 0xC0000005).unwrap_or(false);
    }
}

#[allow(dead_code)]
enum Verdict {
    Pass(String),
    Fail(String),
    Skip(String),
}

fn expect_pass(o: &Out) -> Verdict {
    if o.timed_out {
        return Verdict::Fail("TIMEOUT (deadlock/hang)".into());
    }
    if let Some(l) = o.stdout.lines().find(|l| l.starts_with("CASE_PASS")) {
        return Verdict::Pass(l.trim_start_matches("CASE_PASS").trim().to_string());
    }
    Verdict::Fail(format!(
        "{} | {} {}",
        sig_of(&o.status),
        o.stdout.trim(),
        o.stderr.trim()
    ))
}

fn run_all() -> i32 {
    let page = page_size();
    let info = format!(
        "os={} arch={} env={} page_size={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        if cfg!(target_env = "musl") {
            "musl"
        } else if cfg!(target_env = "gnu") {
            "gnu"
        } else if cfg!(target_env = "msvc") {
            "msvc"
        } else {
            "other"
        },
        page
    );
    println!("# {}", info);
    #[cfg(unix)]
    {
        if let Ok(o) = Command::new("uname").arg("-a").output() {
            println!("# uname: {}", String::from_utf8_lossy(&o.stdout).trim());
        }
    }
    let t = Duration::from_secs(90);
    let mut cases: Vec<(&str, Box<dyn Fn() -> Verdict>)> = vec![];
    macro_rules! pass_case {
        ($n:expr) => {
            cases.push(($n, Box::new(move || expect_pass(&spawn_case($n, t)))));
        };
    }
    pass_case!("alloc_hammer");
    cases.push((
        "alloc_hammer_negative_control",
        Box::new(move || {
            let o = spawn_case("alloc_hammer_negative_control", Duration::from_secs(15));
            if o.timed_out {
                Verdict::Pass("handler that allocates DID deadlock (test is sensitive)".into())
            } else {
                Verdict::Fail(format!(
                    "negative control did not hang: {} {}",
                    sig_of(&o.status),
                    o.stdout.trim()
                ))
            }
        }),
    ));
    #[cfg(unix)]
    pass_case!("libc_lock_fault");
    pass_case!("double_buffer_race");
    cases.push((
        "double_buffer_mutation_control",
        Box::new(move || {
            let o = spawn_case("double_buffer_mutation_control", t);
            if o.stdout.contains("CASE_FAIL mirror != live") {
                Verdict::Pass(
                    "flush with the protect step removed LOSES updates (test is sensitive)".into(),
                )
            } else {
                Verdict::Fail(format!(
                    "mutation not detected: {} {}",
                    sig_of(&o.status),
                    o.stdout.trim()
                ))
            }
        }),
    ));
    pass_case!("pool_exhaustion");
    pass_case!("page_size");
    pass_case!("errno_restore");
    #[cfg(unix)]
    pass_case!("sigmask_guard");
    #[cfg(unix)]
    cases.push((
        "sigmask_blocked_is_fatal",
        Box::new(move || {
            let o = spawn_case("sigmask_blocked_is_fatal", t);
            // Linux kills with SIGSEGV; macOS kills with SIGKILL. Any fatal termination is the documented property.
            if died_by_fault(&o.status) || sig_of(&o.status) == "signal 9" {
                Verdict::Pass(format!(
                    "EXPECTED FATAL ({}): blocked SIGSEGV cannot be handled; crate must guard",
                    sig_of(&o.status)
                ))
            } else {
                Verdict::Fail(format!("{} {}", sig_of(&o.status), o.stdout.trim()))
            }
        }),
    ));
    cases.push((
        "protect_failure_is_loud",
        Box::new(move || {
            let o = spawn_case("protect_failure_is_loud", t);
            if o.timed_out {
                Verdict::Fail("HANG: handler loops forever when protect fails".into())
            } else if died_by_fault(&o.status) {
                Verdict::Pass(format!(
                    "dies loudly ({}) instead of looping",
                    sig_of(&o.status)
                ))
            } else {
                Verdict::Fail(format!("{} {}", sig_of(&o.status), o.stdout.trim()))
            }
        }),
    ));
    for name in ["chain_ours_first", "chain_other_first"] {
        cases.push((
            name,
            Box::new(move || {
                let o = spawn_case(name, t);
                let marks = o.stdout.contains("TRAP_OK L_OK") && o.stdout.contains("L-SAW-UNRELATED");
                if !o.timed_out && marks && died_by_fault(&o.status) {
                    Verdict::Pass(format!("trapped fault handled, other handler's fault handled, unrelated fault reached the other handler then default ({})", sig_of(&o.status)))
                } else {
                    Verdict::Fail(format!("timed_out={} marks={} {} stdout={:?} stderr={:?}", o.timed_out, marks, sig_of(&o.status), o.stdout.trim(), o.stderr.trim()))
                }
            }),
        ));
    }
    for (name, ctl, ours) in [
        ("stack_overflow_main", "so_main_control", "so_main_ours"),
        (
            "stack_overflow_thread_trapped",
            "so_thread_control",
            "so_thread_ours_trapped",
        ),
    ] {
        cases.push((
            name,
            Box::new(move || {
                let (c, o) = (spawn_case(ctl, t), spawn_case(ours, t));
                let msg = |x: &Out| x.stderr.contains("overflowed its stack");
                let (cs, os) = (sig_of(&c.status), sig_of(&o.status));
                if c.timed_out || o.timed_out {
                    return Verdict::Fail("timeout".into());
                }
                if cs == os && msg(&c) == msg(&o) {
                    Verdict::Pass(format!(
                        "identical to control: {} message-printed={}",
                        os,
                        msg(&o)
                    ))
                } else {
                    Verdict::Fail(format!(
                        "control: {} msg={} | ours: {} msg={}",
                        cs,
                        msg(&c),
                        os,
                        msg(&o)
                    ))
                }
            }),
        ));
    }
    let mut fails = 0;
    let mut md = format!(
        "### trap-conformance ({})\n\n| case | result | detail |\n|---|---|---|\n",
        info
    );
    for (name, f) in cases {
        let (r, d) = match f() {
            Verdict::Pass(d) => ("PASS", d),
            Verdict::Fail(d) => {
                fails += 1;
                ("FAIL", d)
            }
            Verdict::Skip(d) => ("SKIP", d),
        };
        println!(
            "RESULT|{}|{}|{}|{}",
            info.replace(' ', ","),
            name,
            r,
            d.replace('\n', " ")
        );
        md.push_str(&format!(
            "| {} | {} | {} |\n",
            name,
            r,
            d.replace('|', "/").replace('\n', " ")
        ));
    }
    if let Ok(p) = std::env::var("GITHUB_STEP_SUMMARY") {
        if let Ok(mut f) = OpenOptions::new().append(true).open(p) {
            let _ = f.write_all(md.as_bytes());
        }
    }
    println!("# failures: {}", fails);
    fails
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() >= 3 && a[1] == "--case" {
        match run_case(&a[2]) {
            Ok(d) => {
                println!("CASE_PASS {}", d);
                let _ = std::io::stdout().flush();
            }
            Err(d) => {
                println!("CASE_FAIL {}", d);
                let _ = std::io::stdout().flush();
                std::process::exit(1);
            }
        }
        return;
    }
    std::process::exit(if run_all() == 0 { 0 } else { 1 });
}
