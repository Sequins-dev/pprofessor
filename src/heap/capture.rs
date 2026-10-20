//! Native capture backends. Public artifacts and callbacks are always pprof.
use crate::PprofProfile;
use anyhow::{Context, Result, ensure};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct CaptureOptions {
    pub interval: Duration,
    pub duration: Option<Duration>,
}
impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            duration: None,
        }
    }
}
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("pprofessor-heap-{}", uuid::Uuid::new_v4()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        Ok(Self(path))
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(super) struct OwnedChild(pub(super) Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
        }
        let _ = self.0.wait();
    }
}
pub(super) fn spawn(mut command: Command) -> Result<OwnedChild> {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    Ok(OwnedChild(
        command.spawn().context("launching heap profiling target")?,
    ))
}
pub(super) fn done(start: Instant, options: &CaptureOptions, stop: &AtomicBool) -> bool {
    stop.load(Ordering::Relaxed) || options.duration.is_some_and(|d| start.elapsed() >= d)
}

/// Capture a new executable with native allocation recording enabled before
/// application work. Recording starts when the platform logger initializes.
/// A duration/stop ends capture and terminates the owned target.
pub fn run(
    binary: &OsStr,
    args: &[OsString],
    options: &CaptureOptions,
    stop: &AtomicBool,
    mut checkpoint: impl FnMut(u32, &PprofProfile),
) -> Result<PprofProfile> {
    ensure!(
        !options.interval.is_zero(),
        "heap checkpoint interval must be positive"
    );
    let workspace = Workspace::new()?;
    #[cfg(target_os = "macos")]
    return run_macos(binary, args, options, stop, &workspace.0, &mut checkpoint);
    #[cfg(target_os = "linux")]
    return super::linux::run(binary, args, options, stop, &workspace.0, &mut checkpoint);
}

/// Attach to a process already recording full native allocation history.
/// Historical allocation sites cannot be recovered from an uninstrumented PID.
pub fn attach(
    pid: u32,
    options: &CaptureOptions,
    stop: &AtomicBool,
    mut checkpoint: impl FnMut(u32, &PprofProfile),
) -> Result<PprofProfile> {
    ensure!(
        !options.interval.is_zero(),
        "heap checkpoint interval must be positive"
    );
    #[cfg(target_os = "macos")]
    {
        let environment = Command::new("/bin/ps")
            .args(["eww", "-p", &pid.to_string(), "-o", "command="])
            .output()?;
        let environment = String::from_utf8_lossy(&environment.stdout);
        ensure!(
            environment
                .split_whitespace()
                .any(|s| s == "MallocStackLoggingNoCompact=1")
                && !environment
                    .split_whitespace()
                    .any(|s| s.starts_with("MallocStackLogging=")),
            "heap attach requires MallocStackLoggingNoCompact=1 and no conflicting MallocStackLogging setting from launch; compact/lite histories cannot report all allocations"
        );
        let start = Instant::now();
        let mut profile = macos_snapshot(&pid.to_string()).context(
            "heap attach requires MallocStackLoggingNoCompact=1 from launch, plus debugger access",
        )?;
        checkpoint(pid, &profile);
        while !done(start, options, stop) && crate::process_exists(pid) {
            // Interval scheduling uses monotonic time, not pprof wall time.
            let until = Instant::now() + options.interval;
            while Instant::now() < until
                && !done(start, options, stop)
                && crate::process_exists(pid)
            {
                std::thread::sleep(Duration::from_millis(25));
            }
            if crate::process_exists(pid) {
                match macos_snapshot(&pid.to_string()) {
                    Ok(next) => {
                        profile = next;
                        checkpoint(pid, &profile);
                    }
                    Err(_) if !crate::process_exists(pid) => break,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(profile)
    }
    #[cfg(target_os = "linux")]
    {
        let env = std::fs::read(format!("/proc/{pid}/environ"))
            .context("reading target recorder configuration")?;
        let path = env
            .split(|b| *b == 0)
            .find_map(|entry| entry.strip_prefix(b"PPROFESSOR_HEAP_LOG="));
        use std::os::unix::ffi::OsStrExt;
        let path=path.context("heap attach requires an existing native recorder; launch with pprofessor run --profile heap, or install the library with pprofessor heap-recorder")?;
        super::linux::attach(
            pid,
            Path::new(OsStr::from_bytes(path)),
            options,
            stop,
            &mut checkpoint,
        )
    }
}
#[cfg(target_os = "macos")]
fn macos_snapshot(target: &str) -> Result<PprofProfile> {
    use std::process::Stdio;
    let workspace = Workspace::new()?;
    let errors = workspace.0.join("tool.stderr");
    let mut child = Command::new("/usr/bin/malloc_history")
        .args([target, "-allEvents"])
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&errors)?)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .context("missing malloc_history output pipe")?;
    let parsed = super::macos::parse_reader(std::io::BufReader::new(stdout));
    if parsed.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    if let Err(error) = parsed {
        return Err(error).context(format!(
            "decoding malloc_history output: {}",
            std::fs::read_to_string(&errors).unwrap_or_default()
        ));
    }
    ensure!(
        status.success(),
        "malloc_history failed: {}",
        std::fs::read_to_string(errors).unwrap_or_default()
    );
    parsed?.to_pprof()
}
#[cfg(target_os = "macos")]
fn run_macos(
    binary: &OsStr,
    args: &[OsString],
    options: &CaptureOptions,
    stop: &AtomicBool,
    workspace: &Path,
    checkpoint: &mut impl FnMut(u32, &PprofProfile),
) -> Result<PprofProfile> {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    let library = workspace.join("libpprofessor_finalizer.dylib");
    std::fs::write(
        &library,
        include_bytes!(concat!(env!("OUT_DIR"), "/libpprofessor_finalizer.dylib")),
    )?;
    let (mut parent, child_channel) = UnixStream::pair()?;
    parent.set_nonblocking(true)?;
    let fd = child_channel.as_raw_fd();
    // The target inherits only its end. Rust's UnixStream normally uses CLOEXEC.
    unsafe {
        ensure!(
            libc::fcntl(fd, libc::F_SETFD, 0) == 0,
            "could not pass finalization channel to target"
        );
    }
    let mut command = Command::new(binary);
    ensure!(
        std::env::var_os("DYLD_INSERT_LIBRARIES").is_none(),
        "heap capture cannot safely compose with existing DYLD_INSERT_LIBRARIES"
    );
    command
        .args(args)
        .env("DYLD_INSERT_LIBRARIES", &library)
        .env("PPROFESSOR_HEAP_CHANNEL", fd.to_string())
        .env("PPROFESSOR_HEAP_PARENT", std::process::id().to_string())
        .env("MallocStackLoggingDirectory", workspace)
        .env_remove("MallocStackLogging")
        .env("MallocStackLoggingNoCompact", "1");
    let mut child = spawn(command)?;
    drop(child_channel);
    let pid = child.0.id();
    let spawned = Instant::now();
    let mut started = None;
    let mut next = Instant::now() + options.interval;
    let mut ready = false;
    let mut last: Option<PprofProfile> = None;
    loop {
        let mut event = [0u8; 1];
        match parent.read(&mut event) {
            Ok(1) if event[0] == b'S' => {
                ensure!(
                    !ready,
                    "target exec replaced the allocation recorder; profile the final executable directly to keep one complete heap epoch"
                );
                ready = true;
                started = Some(Instant::now());
                next = Instant::now() + options.interval;
            }
            Ok(1) if event[0] == b'F' => {
                let profile = macos_snapshot(&pid.to_string());
                // Always release the target, including when conversion failed.
                let _ = parent.write_all(b"A");
                let profile = profile?;
                checkpoint(pid, &profile);
                // Do not let a long-lived forked child hold completion open.
                child.0.wait()?;
                return Ok(profile);
            }
            Err(e)
                if e.kind() != std::io::ErrorKind::WouldBlock
                    && e.kind() != std::io::ErrorKind::Interrupted =>
            {
                return Err(e.into());
            }
            _ => {}
        }
        if let Some(status) = child.0.try_wait()? {
            if let Some(mut profile) = last {
                profile.comments.push(profile.strings.intern(&format!("Target exited with {status} before finalization; this is the last successfully collected checkpoint.")));
                return Ok(profile);
            }
            anyhow::bail!(
                "target exited without a final heap checkpoint ({status}); macOS must permit DYLD injection and debugger access. Use a debuggable build; protected targets cannot be captured."
            );
        }
        let capture_done = started.is_some_and(|start| done(start, options, stop));
        if ready && (Instant::now() >= next || capture_done) {
            let profile = macos_snapshot(&pid.to_string())?;
            checkpoint(pid, &profile);
            if capture_done {
                return Ok(profile);
            }
            last = Some(profile);
            next = Instant::now() + options.interval;
        }
        if !ready && (stop.load(Ordering::Relaxed) || spawned.elapsed() > Duration::from_secs(10)) {
            anyhow::bail!(
                "macOS heap finalizer did not initialize; target must permit DYLD injection"
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
