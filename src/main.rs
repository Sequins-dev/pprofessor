mod cli;

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use clap::Parser;
use flate2::Compression;
use flate2::write::GzEncoder;
use nix::sys::signal::{self, SaFlags, SigAction, SigSet, Signal};

use cli::{Cli, Command};

// ---------------------------------------------------------------------------
// Global signal state
// ---------------------------------------------------------------------------

/// Set to true when we receive SIGINT or SIGTERM.
static STOP_FLAG: AtomicBool = AtomicBool::new(false);

/// PID of the child process to forward signals to (0 = no child).
static CHILD_PID: AtomicI32 = AtomicI32::new(0);

extern "C" fn signal_handler(sig: libc::c_int) {
    STOP_FLAG.store(true, Ordering::Relaxed);

    let child = CHILD_PID.load(Ordering::Relaxed);
    if child > 0 {
        unsafe { libc::kill(child, sig) };
    }
}

fn install_signal_handlers() -> Result<()> {
    let sa = SigAction::new(
        signal::SigHandler::Handler(signal_handler),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    unsafe {
        signal::sigaction(Signal::SIGINT, &sa).context("sigaction SIGINT")?;
        signal::sigaction(Signal::SIGTERM, &sa).context("sigaction SIGTERM")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Gzip writer
// ---------------------------------------------------------------------------

fn write_gzip(data: &[u8], output: &Path) -> Result<()> {
    let compressed = gzip_data(data)?;
    std::fs::write(output, compressed)
        .with_context(|| format!("writing output file {}", output.display()))?;
    Ok(())
}

fn gzip_data(data: &[u8]) -> Result<Vec<u8>> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(data)
        .context("writing gzip-compressed profile")?;
    gz.finish().context("finalizing gzip output")
}

fn session_id(pid: u32) -> String {
    let _ = pid;
    uuid::Uuid::new_v4().to_string()
}

fn publish_update(
    profile: &pprofessor::Profile,
    cursor: &mut pprofessor::RawProfileCursor,
    symbolizer: &mut Option<pprofessor::NativeSymbolizer>,
    publisher: &mut pprofessor::SessionPublisher,
    freq: u32,
    start_wall: SystemTime,
) {
    let snapshot = profile.snapshot_raw();
    let addresses: Vec<u64> = snapshot
        .stacks
        .keys()
        .flat_map(|(_, stack)| stack.iter().copied())
        .collect();
    let symbolizer = symbolizer
        .get_or_insert_with(|| pprofessor::NativeSymbolizer::new(snapshot.images.clone(), &[]));
    symbolizer.refresh_images(snapshot.images.clone());
    symbolizer.resolve_more(&addresses);

    if !publisher.ensure_connected().unwrap_or(false) {
        return;
    }
    let mut candidate = cursor.clone();
    let Some(delta) = candidate.delta(&snapshot) else {
        return;
    };
    let live = pprofessor::SymbolicatedProfile::from_raw_with_symbolizer(
        delta,
        symbolizer,
        pprofessor::Unresolved::Hex,
        freq,
        start_wall,
    );
    if publisher
        .send(pprofessor::FrameKind::ProfileDelta, &live.to_pprof())
        .unwrap_or(false)
    {
        *cursor = candidate;
    }
}

// ---------------------------------------------------------------------------
// Shared builder helpers
// ---------------------------------------------------------------------------

fn apply_thread_filter(
    builder: pprofessor::ProfilerBuilder,
    thread: Option<String>,
    thread_id: Option<u64>,
    main_thread: bool,
) -> pprofessor::ProfilerBuilder {
    if main_thread {
        builder.main_thread()
    } else if let Some(name) = thread {
        builder.thread_name(name)
    } else if let Some(id) = thread_id {
        builder.thread_id(id)
    } else {
        builder
    }
}

fn apply_duration(
    builder: pprofessor::ProfilerBuilder,
    secs: Option<f64>,
) -> pprofessor::ProfilerBuilder {
    if let Some(s) = secs {
        builder.duration(Duration::from_secs_f64(s))
    } else {
        builder
    }
}

// ---------------------------------------------------------------------------
// run subcommand
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_command(
    binary: std::ffi::OsString,
    args: Vec<std::ffi::OsString>,
    freq: u32,
    output: std::path::PathBuf,
    duration: Option<f64>,
    thread: Option<String>,
    thread_id: Option<u64>,
    main_thread: bool,
    publish: bool,
) -> Result<()> {
    let builder = apply_duration(
        apply_thread_filter(
            pprofessor::builder().freq(freq),
            thread,
            thread_id,
            main_thread,
        ),
        duration,
    );
    let mut handle = builder
        .spawn(&binary, &args)
        .with_context(|| format!("spawning and attaching profiler for {binary:?}"))?;

    CHILD_PID.store(handle.pid() as i32, Ordering::Relaxed);
    install_signal_handlers()?;

    let start_wall = SystemTime::now();
    let profile = handle.start()?;
    let mut publisher = publish.then(|| {
        pprofessor::SessionPublisher::new(
            pprofessor::SessionPublisher::default_address(),
            pprofessor::SessionHello::new(
                session_id(handle.pid()),
                "run",
                handle.pid(),
                binary.to_string_lossy(),
                freq,
            ),
        )
    });
    let mut cursor = pprofessor::RawProfileCursor::default();
    let mut symbolizer = None;
    let mut next_publish = Instant::now();

    // Wait until the child exits, the duration elapses, or we receive a signal.
    while !STOP_FLAG.load(Ordering::Relaxed) && !profile.is_stopped() {
        std::thread::sleep(Duration::from_millis(50));
        if let Some(publisher) = publisher.as_mut()
            && Instant::now() >= next_publish
        {
            publish_update(
                &profile,
                &mut cursor,
                &mut symbolizer,
                publisher,
                freq,
                start_wall,
            );
            next_publish = Instant::now() + Duration::from_millis(500);
        }
    }
    profile.signal_stop();

    eprintln!("pprofessor: symbolicating...");
    if let Some(publisher) = publisher.as_mut() {
        let _ = publisher.send(pprofessor::FrameKind::Finalizing, &[]);
    }
    let data = profile.stop()?.to_pprof();

    CHILD_PID.store(0, Ordering::Relaxed);

    write_gzip(&data, &output)?;
    if let Some(publisher) = publisher.as_mut() {
        let compressed = gzip_data(&data)?;
        let _ = publisher.send(pprofessor::FrameKind::FinalProfile, &compressed);
    }
    eprintln!("pprofessor: profile written to {}", output.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// attach subcommand
// ---------------------------------------------------------------------------

struct AttachOptions {
    pid: u32,
    freq: u32,
    output: std::path::PathBuf,
    duration: Option<f64>,
    thread: Option<String>,
    thread_id: Option<u64>,
    main_thread: bool,
    publish: bool,
    expected_start_time: Option<u64>,
    requested_session_id: Option<String>,
}

fn validate_process_identity(pid: u32, expected: u64, actual: Option<u64>) -> Result<()> {
    if actual != Some(expected) {
        anyhow::bail!(
            "cannot attach to pid {pid}: the selected process exited or its PID was reused"
        );
    }
    Ok(())
}

fn attach_preflight_error(pid: u32, attachable: bool, reason: Option<&str>) -> Option<String> {
    (!attachable).then(|| {
        format!(
            "cannot attach to pid {pid}: the target process is protected by macOS and does not permit debugging. \
             Production-signed and system processes normally omit the get-task-allow entitlement.{}",
            reason.map(|reason| format!(" ({reason})")).unwrap_or_default()
        )
    })
}

fn attach_command(options: AttachOptions) -> Result<()> {
    let AttachOptions {
        pid,
        freq,
        output,
        duration,
        thread,
        thread_id,
        main_thread,
        publish,
        expected_start_time,
        requested_session_id,
    } = options;
    let target_process = pprofessor::list_processes()?
        .into_iter()
        .find(|process| process.pid == pid);
    if let Some(expected) = expected_start_time {
        let actual = target_process
            .as_ref()
            .map(|process| process.start_time_micros);
        validate_process_identity(pid, expected, actual)?;
    }
    if let Some(process) = target_process.as_ref()
        && let Some(error) = attach_preflight_error(
            pid,
            process.attachable,
            process.attachability_reason.as_deref(),
        )
    {
        anyhow::bail!(error);
    }
    let current_arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    if let Some(required_arch) = target_process
        .as_ref()
        .and_then(|process| pprofessor::required_helper_arch(current_arch, &process.architecture))
    {
        install_signal_handlers()?;
        let executable = std::env::current_exe().context("locating profiler executable")?;
        let mut command = std::process::Command::new("/usr/bin/arch");
        command.arg(format!("-{required_arch}")).arg(executable);
        command.args(std::env::args_os().skip(1));
        let mut child = command
            .spawn()
            .context("launching matching profiler architecture")?;
        CHILD_PID.store(child.id() as i32, Ordering::Relaxed);
        let status = child
            .wait()
            .context("waiting for matching profiler architecture")?;
        CHILD_PID.store(0, Ordering::Relaxed);
        if !status.success() {
            anyhow::bail!("matching {required_arch} profiler exited with {status}");
        }
        return Ok(());
    }
    install_signal_handlers()?;

    let builder = apply_duration(
        apply_thread_filter(
            pprofessor::builder().freq(freq),
            thread,
            thread_id,
            main_thread,
        ),
        duration,
    );
    let mut handle = builder
        .attach(pid)
        .with_context(|| format!("attaching profiler to pid {pid}"))?;

    let hint = if duration.is_some() {
        ""
    } else {
        " — press Ctrl+C to stop"
    };
    eprintln!("pprofessor: profiling pid {pid} at {freq} Hz{hint}");

    let start_wall = SystemTime::now();
    let profile = handle.start()?;
    let process_name = pprofessor::list_processes()?
        .into_iter()
        .find(|process| process.pid == pid)
        .map(|process| process.name)
        .unwrap_or_else(|| pid.to_string());
    let mut publisher = publish.then(|| {
        pprofessor::SessionPublisher::new(
            pprofessor::SessionPublisher::default_address(),
            pprofessor::SessionHello::new(
                requested_session_id.unwrap_or_else(|| session_id(pid)),
                "attach",
                pid,
                process_name,
                freq,
            ),
        )
    });
    let mut cursor = pprofessor::RawProfileCursor::default();
    let mut symbolizer = None;
    let mut next_publish = Instant::now();

    // Wait until the target exits, the duration elapses, or we receive a signal.
    while !STOP_FLAG.load(Ordering::Relaxed) && !profile.is_stopped() {
        std::thread::sleep(Duration::from_millis(50));
        if let Some(publisher) = publisher.as_mut()
            && Instant::now() >= next_publish
        {
            publish_update(
                &profile,
                &mut cursor,
                &mut symbolizer,
                publisher,
                freq,
                start_wall,
            );
            next_publish = Instant::now() + Duration::from_millis(500);
        }
    }
    profile.signal_stop();

    eprintln!("pprofessor: symbolicating...");
    if let Some(publisher) = publisher.as_mut() {
        let _ = publisher.send(pprofessor::FrameKind::Finalizing, &[]);
    }
    let data = profile.stop()?.to_pprof();

    write_gzip(&data, &output)?;
    if let Some(publisher) = publisher.as_mut() {
        let compressed = gzip_data(&data)?;
        let _ = publisher.send(pprofessor::FrameKind::FinalProfile, &compressed);
    }
    eprintln!("pprofessor: profile written to {}", output.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Run {
            publish,
            freq,
            output,
            duration,
            thread,
            thread_id,
            main_thread,
            binary,
            args,
        } => run_command(
            binary,
            args,
            freq,
            output,
            duration,
            thread,
            thread_id,
            main_thread,
            publish,
        ),
        Command::Attach {
            publish,
            expected_start_time,
            session_id,
            freq,
            output,
            duration,
            thread,
            thread_id,
            main_thread,
            pid,
        } => attach_command(AttachOptions {
            pid,
            freq,
            output,
            duration,
            thread,
            thread_id,
            main_thread,
            publish,
            expected_start_time,
            requested_session_id: session_id,
        }),
        Command::Processes { json } => {
            let processes = pprofessor::list_processes();
            match processes {
                Ok(processes) => {
                    if json {
                        println!("{}", serde_json::to_string(&processes).unwrap());
                    } else {
                        for process in processes {
                            println!(
                                "{:>7}  {:<8}  {}",
                                process.pid, process.architecture, process.name
                            );
                        }
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
    };

    if let Err(e) = result {
        eprintln!("pprofessor: error: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod attach_error_tests {
    use super::*;

    #[test]
    fn protected_target_error_explains_target_policy() {
        let error = attach_preflight_error(
            42,
            false,
            Some("Protected by macOS: this process does not allow debugging"),
        )
        .expect("protected target should produce an error");

        assert!(error.contains("target process is protected by macOS"));
        assert!(error.contains("get-task-allow"));
        assert!(!error.contains("sign the profiler"));
    }

    #[test]
    fn attachable_target_has_no_preflight_error() {
        assert!(attach_preflight_error(42, true, None).is_none());
    }

    #[test]
    fn pid_identity_error_distinguishes_exit_or_reuse() {
        let error = validate_process_identity(42, 123, Some(456)).unwrap_err();

        assert!(error.to_string().contains("exited or its PID was reused"));
    }
}
