pub mod builder;
pub(crate) mod encode;
pub mod handle;
pub mod pprof;
#[cfg(target_os = "macos")]
pub mod processes;
pub mod profile;
pub mod sampler;
pub mod stream;
pub mod symbolicate;
pub mod symbolicated;

pub use builder::ProfilerBuilder;
pub use handle::ProfilerHandle;
pub use pprof::ProfileEncoder;
#[cfg(target_os = "macos")]
pub use processes::{ProcessInfo, list_processes, required_helper_arch};
pub use profile::Profile;
pub use sampler::{LoadedImage, RawProfile, RawProfileCursor, RawSampleSeries, ThreadFilter};
pub use stream::{
    DEFAULT_SESSION_PORT, FrameKind, STREAM_HEADER_LEN, SessionHello, SessionPublisher,
    StreamHeader, StreamProtocolError,
};
pub use symbolicate::{FrameInfo, NativeSymbolizer, Symbolizer, SymbolizerChain};
pub use symbolicated::{Sample, StackFrame, SymbolicatedProfile, TreeNode, Unresolved};

pub fn builder() -> ProfilerBuilder {
    ProfilerBuilder::new()
}

/// Returns false when the process with the given PID no longer exists.
///
/// Uses kill(pid, 0) which is a no-op that lets the kernel tell us whether
/// the process is present without sending an actual signal.
pub(crate) fn process_exists(pid: u32) -> bool {
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    // ESRCH means "no such process" — anything else (0, EPERM) means it exists.
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}
