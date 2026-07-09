mod cli;

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

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
    let file = std::fs::File::create(output)
        .with_context(|| format!("creating output file {}", output.display()))?;
    let mut gz = GzEncoder::new(file, Compression::default());
    gz.write_all(data)
        .context("writing gzip-compressed profile")?;
    gz.finish().context("finalizing gzip output")?;
    Ok(())
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

    let profile = handle.start()?;

    // Wait until the child exits, the duration elapses, or we receive a signal.
    while !STOP_FLAG.load(Ordering::Relaxed) && !profile.is_stopped() {
        std::thread::sleep(Duration::from_millis(50));
    }
    profile.signal_stop();

    eprintln!("pprofessor: symbolicating...");
    let data = profile.stop()?.to_pprof();

    CHILD_PID.store(0, Ordering::Relaxed);

    write_gzip(&data, &output)?;
    eprintln!("pprofessor: profile written to {}", output.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// attach subcommand
// ---------------------------------------------------------------------------

fn attach_command(
    pid: u32,
    freq: u32,
    output: std::path::PathBuf,
    duration: Option<f64>,
    thread: Option<String>,
    thread_id: Option<u64>,
    main_thread: bool,
) -> Result<()> {
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

    let profile = handle.start()?;

    // Wait until the target exits, the duration elapses, or we receive a signal.
    while !STOP_FLAG.load(Ordering::Relaxed) && !profile.is_stopped() {
        std::thread::sleep(Duration::from_millis(50));
    }
    profile.signal_stop();

    eprintln!("pprofessor: symbolicating...");
    let data = profile.stop()?.to_pprof();

    write_gzip(&data, &output)?;
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
        ),
        Command::Attach {
            freq,
            output,
            duration,
            thread,
            thread_id,
            main_thread,
            pid,
        } => attach_command(pid, freq, output, duration, thread, thread_id, main_thread),
    };

    if let Err(e) = result {
        eprintln!("pprofessor: error: {e:#}");
        std::process::exit(1);
    }
}
