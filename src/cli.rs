use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "pprofessor",
    about = "Native process profiler with pprof output",
    long_about = "Samples CPU usage of a process and writes a gzip-compressed pprof profile.\n\n\
                  Requires no special permissions when profiling a child process spawned via 'run'.\n\
                  The 'attach' subcommand requires root or the com.apple.security.cs.debugger entitlement."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Spawn a child process and profile it until it exits (or is interrupted)
    Run {
        /// Sampling frequency in Hz
        #[arg(long, default_value_t = 99)]
        freq: u32,

        /// Output file path (gzip-compressed pprof protobuf)
        #[arg(long, short, default_value = "profile.pb.gz")]
        output: PathBuf,

        /// Stop profiling after this many seconds (decimal allowed, e.g. 0.5)
        #[arg(long)]
        duration: Option<f64>,

        /// Only sample threads whose name contains this string
        #[arg(long, conflicts_with_all = ["thread_id", "main_thread"])]
        thread: Option<String>,

        /// Only sample the thread with this numeric thread ID
        #[arg(long, conflicts_with_all = ["thread", "main_thread"])]
        thread_id: Option<u64>,

        /// Only sample the main thread of the target process
        #[arg(long, conflicts_with_all = ["thread", "thread_id"])]
        main_thread: bool,

        /// The binary to execute
        binary: OsString,

        /// Arguments passed to the child binary
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },

    /// Attach to a running process by PID and profile until interrupted
    Attach {
        /// Sampling frequency in Hz
        #[arg(long, default_value_t = 99)]
        freq: u32,

        /// Output file path (gzip-compressed pprof protobuf)
        #[arg(long, short, default_value = "profile.pb.gz")]
        output: PathBuf,

        /// Stop profiling after this many seconds (decimal allowed, e.g. 0.5)
        #[arg(long)]
        duration: Option<f64>,

        /// Only sample threads whose name contains this string
        #[arg(long, conflicts_with_all = ["thread_id", "main_thread"])]
        thread: Option<String>,

        /// Only sample the thread with this numeric thread ID
        #[arg(long, conflicts_with_all = ["thread", "main_thread"])]
        thread_id: Option<u64>,

        /// Only sample the main thread of the target process
        #[arg(long, conflicts_with_all = ["thread", "thread_id"])]
        main_thread: bool,

        /// PID of the process to profile
        pid: u32,
    },
}
