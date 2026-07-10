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
        /// Do not publish this session to a running PProfessor app
        #[arg(long = "no-publish", action = clap::ArgAction::SetFalse, default_value_t = true)]
        publish: bool,

        /// Sampling frequency in Hz
        #[arg(long, default_value_t = 99, value_parser = clap::value_parser!(u32).range(1..=1000))]
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
        /// Do not publish this session to a running PProfessor app
        #[arg(long = "no-publish", action = clap::ArgAction::SetFalse, default_value_t = true)]
        publish: bool,

        /// Reject the PID if its process start time no longer matches
        #[arg(long)]
        expected_start_time: Option<u64>,

        /// Stable session identifier supplied by an app launcher
        #[arg(long)]
        session_id: Option<String>,

        /// Sampling frequency in Hz
        #[arg(long, default_value_t = 99, value_parser = clap::value_parser!(u32).range(1..=1000))]
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

    /// List processes owned by the current user
    Processes {
        /// Emit machine-readable JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_process_listing() {
        let cli = Cli::try_parse_from(["pprofessor", "processes", "--json"]).unwrap();
        assert!(matches!(cli.command, Command::Processes { json: true }));
    }

    #[test]
    fn parses_attach_publishing_controls() {
        let cli = Cli::try_parse_from([
            "pprofessor",
            "attach",
            "--no-publish",
            "--expected-start-time",
            "123",
            "--session-id",
            "abc",
            "42",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Attach {
                publish: false,
                expected_start_time: Some(123),
                session_id: Some(ref value),
                pid: 42,
                ..
            } if value == "abc"
        ));
    }

    #[test]
    fn rejects_zero_sampling_frequency() {
        assert!(Cli::try_parse_from(["pprofessor", "attach", "--freq", "0", "42"]).is_err());
    }
}
