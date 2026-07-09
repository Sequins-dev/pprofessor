//! Integration tests: exercise both the library API and the CLI binary.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Build the busy_loop fixture binary and return its path.
fn build_fixture() -> PathBuf {
    let status = Command::new("rustc")
        .args([
            "tests/fixtures/busy_loop.rs",
            "-o",
            "target/debug/busy_loop_fixture",
        ])
        .status()
        .expect("rustc not found");
    assert!(status.success(), "failed to compile busy_loop fixture");
    PathBuf::from("target/debug/busy_loop_fixture")
}

/// Build the pprofessor binary (debug mode) and return its path.
fn build_pprofessor() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "--bin", "pprofessor"])
        .status()
        .expect("cargo not found");
    assert!(status.success(), "failed to build pprofessor");
    PathBuf::from("target/debug/pprofessor")
}

/// Returns true if we have the privileges needed for task_for_pid.
///
/// Authoritative check: spawn a trivial child and try task_for_pid on it.
/// Getting your own task port always succeeds, so self-probing is not useful.
/// Ad-hoc signing is not trusted by SIP, so only root reliably works.
fn has_task_for_pid_permission() -> bool {
    #[link(name = "System")]
    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn task_for_pid(target: u32, pid: i32, task_out: *mut u32) -> i32;
    }
    // Spawn a trivial child. We keep it running briefly so task_for_pid has a
    // valid PID to query (a completed process has no task).
    let child = match Command::new("/bin/sleep").arg("2").spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let pid = child.id() as i32;
    let mut task: u32 = 0;
    let kr = unsafe { task_for_pid(mach_task_self(), pid, &mut task) };
    unsafe { libc::kill(pid, libc::SIGKILL) };
    kr == 0 // KERN_SUCCESS
}

/// Verify the buffer is a valid gzip stream containing a non-empty pprof protobuf.
fn assert_valid_pprof_gzip(data: &[u8]) {
    assert!(!data.is_empty(), "output data is empty");
    let mut gz = flate2::read::GzDecoder::new(data);
    let mut buf = Vec::new();
    gz.read_to_end(&mut buf).expect("output is not valid gzip");
    assert!(!buf.is_empty(), "decompressed protobuf is empty");
    // First byte of a valid pprof protobuf: field 1 (sample_type), wire type 2 → 0x0a.
    assert_eq!(
        buf[0], 0x0a,
        "first protobuf byte should be 0x0a (field 1, wire 2)"
    );
}

/// Assert that `bytes` is a valid pprof protobuf (not gzip-wrapped).
fn assert_valid_pprof(bytes: &[u8]) {
    assert!(!bytes.is_empty(), "protobuf data is empty");
    assert_eq!(bytes[0], 0x0a, "first protobuf byte should be 0x0a");
}

/// Do a small amount of CPU work to ensure at least a few samples are taken.
fn do_cpu_work() {
    let mut x: u64 = 0;
    for i in 0..10_000_000u64 {
        x = x.wrapping_add(i);
    }
    let _ = x;
}

// ---------------------------------------------------------------------------
// CLI test
// ---------------------------------------------------------------------------

#[test]
fn test_profile_child_produces_valid_gzip_output() {
    let fixture = build_fixture();
    let pprofessor = build_pprofessor();

    if !has_task_for_pid_permission() {
        eprintln!(
            "SKIP: test_profile_child_produces_valid_gzip_output — no task_for_pid permission."
        );
        eprintln!("      Run as root or sign the binary: make sign");
        return;
    }

    let output_path = std::env::temp_dir().join("pprofessor_integration_test.pb.gz");
    let _ = std::fs::remove_file(&output_path);

    // Run pprofessor wrapping the fixture. The fixture runs for up to 5s,
    // and pprofessor will exit when the child does.
    let status = Command::new(&pprofessor)
        .args([
            "run",
            "--freq",
            "99",
            "--output",
            output_path.to_str().unwrap(),
            fixture.to_str().unwrap(),
        ])
        .stderr(Stdio::inherit())
        .status()
        .expect("failed to run pprofessor");

    assert!(status.success(), "pprofessor exited with {status}");
    assert!(output_path.exists(), "output file was not created");

    let raw = std::fs::read(&output_path).unwrap();
    assert_valid_pprof_gzip(&raw);

    std::fs::remove_file(&output_path).ok();
}

// ---------------------------------------------------------------------------
// Library API test — spawn
// ---------------------------------------------------------------------------

#[test]
fn test_library_spawn_produces_valid_protobuf() {
    let fixture = build_fixture();

    if !has_task_for_pid_permission() {
        eprintln!("SKIP: test_library_spawn_produces_valid_protobuf — no task_for_pid permission.");
        return;
    }

    let handle = pprofessor::builder()
        .freq(99)
        .spawn(fixture.as_os_str(), &[] as &[&str])
        .expect("failed to spawn profiler");

    let mut handle = handle;
    let profile = handle.start().expect("failed to start profile");

    // Wait for child to finish (detected automatically via try_wait).
    while !profile.is_stopped() {
        std::thread::sleep(Duration::from_millis(50));
    }

    let symbolicated = profile.stop().expect("failed to stop profile");
    assert_valid_pprof(&symbolicated.to_pprof());
}

// ---------------------------------------------------------------------------
// Library API test — current() self-profile (no permissions needed)
// ---------------------------------------------------------------------------

#[test]
fn test_library_current_produces_valid_protobuf() {
    let mut handle = pprofessor::builder()
        .freq(99)
        .current()
        .expect("failed to create self-profiler");

    let profile = handle.start().expect("failed to start profile");

    do_cpu_work();
    std::thread::sleep(Duration::from_millis(200));

    let symbolicated = profile.stop().expect("failed to stop self-profile");
    assert_valid_pprof(&symbolicated.to_pprof());
}

// ---------------------------------------------------------------------------
// SymbolicatedProfile fields
// ---------------------------------------------------------------------------

#[test]
fn test_symbolicated_profile_has_samples_and_duration() {
    let mut handle = pprofessor::builder()
        .freq(99)
        .current()
        .expect("failed to create self-profiler");

    let profile = handle.start().expect("failed to start profile");

    do_cpu_work();
    std::thread::sleep(Duration::from_millis(200));

    let symbolicated = profile.stop().expect("failed to stop profile");

    assert!(!symbolicated.samples.is_empty(), "no samples collected");
    assert!(
        symbolicated.duration.as_millis() >= 100,
        "duration too short: {:?}",
        symbolicated.duration
    );
    assert_eq!(symbolicated.freq_hz, 99);

    // Every sample should have at least one frame in the pool.
    for sample in &symbolicated.samples {
        assert!(sample.count > 0, "sample has zero count");
        assert!(!sample.stack.is_empty(), "sample has no stack");
        // At least one address should resolve to a frame.
        let frames = symbolicated.resolve_stack(sample);
        assert!(!frames.is_empty(), "no frames resolved for sample");
    }
}

// ---------------------------------------------------------------------------
// stop_unsymbolicated — frames should have hex function names
// ---------------------------------------------------------------------------

#[test]
fn test_stop_unsymbolicated_has_hex_functions() {
    let mut handle = pprofessor::builder()
        .freq(99)
        .current()
        .expect("failed to create self-profiler");

    let profile = handle.start().expect("failed to start profile");

    do_cpu_work();
    std::thread::sleep(Duration::from_millis(200));

    let symbolicated = profile
        .stop_unsymbolicated()
        .expect("failed to stop profile");

    assert!(!symbolicated.samples.is_empty(), "no samples collected");

    // Without symbolication every function name should be a hex address.
    for frame in symbolicated.frames.values() {
        assert!(
            frame.function.starts_with("0x"),
            "expected hex address, got: {}",
            frame.function
        );
    }

    // The protobuf encoding should still be valid.
    assert_valid_pprof(&symbolicated.to_pprof());
}

// ---------------------------------------------------------------------------
// Closure profiler
// ---------------------------------------------------------------------------

#[test]
fn test_closure_profiler_returns_result_and_profile() {
    let (result, symbolicated) = pprofessor::builder()
        .freq(99)
        .profile(|| {
            do_cpu_work();
            42u32
        })
        .expect("closure profiler failed");

    assert_eq!(result, 42u32);
    assert!(
        !symbolicated.samples.is_empty(),
        "no samples from closure profiler"
    );
    assert_valid_pprof(&symbolicated.to_pprof());
}

// ---------------------------------------------------------------------------
// Duration timeout
// ---------------------------------------------------------------------------

#[test]
fn test_duration_stops_sampling_automatically() {
    let mut handle = pprofessor::builder()
        .freq(99)
        .duration(Duration::from_millis(200))
        .current()
        .expect("failed to create timed self-profiler");

    let profile = handle.start().expect("failed to start profile");

    // Wait longer than the duration to confirm the sampler self-stops.
    std::thread::sleep(Duration::from_millis(500));

    assert!(
        profile.is_stopped(),
        "sampler should have stopped after the deadline"
    );

    let symbolicated = profile.stop().expect("failed to stop profile");
    // Duration should be close to the configured 200ms (allow generous slack
    // for scheduler jitter, but confirm it's not zero)
    assert!(
        symbolicated.duration.as_millis() > 50,
        "duration unexpectedly short: {:?}",
        symbolicated.duration
    );
    assert!(
        symbolicated.duration.as_millis() < 1000,
        "duration unexpectedly long: {:?}",
        symbolicated.duration
    );
}

// ---------------------------------------------------------------------------
// Thread name filter
// ---------------------------------------------------------------------------

#[test]
fn test_thread_name_filter() {
    use std::sync::{Arc, Barrier};

    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = Arc::clone(&barrier);

    // Spawn a named worker thread that does CPU work.
    let worker = std::thread::Builder::new()
        .name("pprofessor-test-worker".to_string())
        .spawn(move || {
            barrier_clone.wait(); // signal ready
            do_cpu_work();
        })
        .expect("failed to spawn worker thread");

    barrier.wait(); // wait until worker is running

    let mut handle = pprofessor::builder()
        .freq(99)
        .thread_name("pprofessor-test-worker")
        .current()
        .expect("failed to create thread-filtered profiler");

    let profile = handle.start().expect("failed to start profile");
    std::thread::sleep(Duration::from_millis(200));
    let symbolicated = profile.stop().expect("failed to stop profile");

    worker.join().ok();

    // All samples should come from the named worker thread.
    for sample in &symbolicated.samples {
        let thread_name = symbolicated
            .threads
            .get(&sample.thread_id)
            .cloned()
            .unwrap_or_default();
        assert!(
            thread_name.contains("pprofessor-test-worker"),
            "unexpected thread name: {:?}",
            thread_name
        );
    }
}

// ---------------------------------------------------------------------------
// Custom symbolizer — renames all frames
// ---------------------------------------------------------------------------

#[test]
fn test_custom_symbolizer_renames_frames() {
    use pprofessor::{FrameInfo, Symbolizer};

    struct PrefixSymbolizer;
    impl Symbolizer for PrefixSymbolizer {
        fn symbolize_frame(&self, addr: u64) -> Option<FrameInfo> {
            Some(FrameInfo {
                function: format!("custom:0x{addr:016x}"),
                file: String::new(),
                line: 0,
            })
        }
    }

    let mut handle = pprofessor::builder()
        .freq(99)
        .symbolizer(PrefixSymbolizer)
        .current()
        .expect("failed to create profiler with custom symbolizer");

    let profile = handle.start().expect("failed to start profile");
    do_cpu_work();
    std::thread::sleep(Duration::from_millis(200));
    let symbolicated = profile.stop().expect("failed to stop profile");

    assert!(
        !symbolicated.samples.is_empty(),
        "no samples collected with custom symbolizer"
    );

    // All frame function names should be prefixed with "custom:".
    for frame in symbolicated.frames.values() {
        assert!(
            frame.function.starts_with("custom:"),
            "expected custom: prefix, got: {}",
            frame.function
        );
    }
}

// ---------------------------------------------------------------------------
// Symbolizer chain — always-None first, native fallback
// ---------------------------------------------------------------------------

#[test]
fn test_symbolizer_chain_fallback() {
    use pprofessor::{FrameInfo, Symbolizer, SymbolizerChain};

    struct AlwaysNone;
    impl Symbolizer for AlwaysNone {
        fn symbolize_frame(&self, _addr: u64) -> Option<FrameInfo> {
            None
        }
    }

    // Chain: always-None first, then native. Native should resolve frames.
    let chain = SymbolizerChain::new(vec![
        Box::new(AlwaysNone),
        Box::new(pprofessor::NativeSymbolizer::new(vec![], &[])),
    ]);

    let mut handle = pprofessor::builder()
        .freq(99)
        .symbolizer(chain)
        // With an empty NativeSymbolizer (no images), all addresses fall through to native.
        // The real native fallback is still appended by the library. This tests the chain itself.
        .current()
        .expect("failed to create profiler");

    let profile = handle.start().expect("failed to start profile");
    do_cpu_work();
    std::thread::sleep(Duration::from_millis(200));
    let symbolicated = profile.stop().expect("failed to stop profile");

    // The library appends real native as fallback — frames should be resolved.
    assert!(!symbolicated.samples.is_empty(), "no samples collected");
    assert!(!symbolicated.frames.is_empty(), "no frames resolved");
}

// ---------------------------------------------------------------------------
// Tree view — roots and children
// ---------------------------------------------------------------------------

#[test]
fn test_tree_view_has_roots() {
    let mut handle = pprofessor::builder()
        .freq(99)
        .current()
        .expect("failed to create self-profiler");

    let profile = handle.start().expect("failed to start profile");
    do_cpu_work();
    std::thread::sleep(Duration::from_millis(200));
    let symbolicated = profile.stop().expect("failed to stop profile");

    if symbolicated.samples.is_empty() {
        return; // no samples, skip
    }

    let roots = symbolicated.roots();
    assert!(!roots.is_empty(), "tree should have root nodes");

    // Total count of roots should equal sum of all sample counts.
    let total_from_roots: u64 = roots.iter().map(|n| n.total_count()).sum();
    let total_from_samples: u64 = symbolicated.samples.iter().map(|s| s.count).sum();
    assert_eq!(
        total_from_roots, total_from_samples,
        "root total_count mismatch"
    );
}
