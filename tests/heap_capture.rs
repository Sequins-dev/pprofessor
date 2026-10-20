use std::io::Read;
use std::process::Command;

fn build_fixture() -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("pprofessor-heap-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let fixture = dir.join("workload");
    let compile = Command::new("cc")
        .args([
            "-g",
            "-O0",
            "-fno-omit-frame-pointer",
            "-fno-builtin",
            "-pthread",
        ])
        .arg("tests/fixtures/heap/workload.c")
        .arg("-o")
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    (dir, fixture)
}

#[test]
fn native_heap_run_captures_freed_and_retained_allocations_in_pprof() {
    assert_native_capture("normal");
}
#[test]
fn immediate_exit_preserves_the_final_heap() {
    assert_native_capture("immediate");
}

fn assert_native_capture(argument: &str) {
    let (dir, fixture) = build_fixture();
    let output = dir.join("heap.pb.gz");
    let capture = Command::new(env!("CARGO_BIN_EXE_pprofessor"))
        .args(["run", "--profile", "heap", "--no-publish", "--output"])
        .arg(&output)
        .arg(&fixture)
        .arg(argument)
        .output()
        .unwrap();
    assert!(
        capture.status.success(),
        "{}",
        String::from_utf8_lossy(&capture.stderr)
    );
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(&output).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    let profile = pprofessor::PprofProfile::decode(&bytes).unwrap();
    assert!(profile.strings.strings[profile.mappings[0].filename as usize].contains("workload"));
    let report = |metric: &str| {
        pprofessor::report::ProfileReport::from_profile(&profile, Some(metric)).unwrap()
    };
    assert!(report("alloc_space").total_value() > report("inuse_space").total_value());
    for name in [
        "heap_retained",
        "heap_churn",
        "heap_resize",
        "heap_aligned",
        "heap_cross_thread_allocate",
    ] {
        assert!(
            profile.strings.strings.iter().any(|s| s.contains(name)),
            "missing {name}"
        );
    }
    assert!(
        !profile
            .strings
            .strings
            .iter()
            .any(|s| s.contains("heap_fork_child"))
    );
    let churn = profile
        .functions
        .iter()
        .find(|f| profile.strings.strings[f.name as usize].contains("heap_churn"))
        .unwrap()
        .id;
    let locations: Vec<_> = profile
        .locations
        .iter()
        .filter(|l| l.lines.iter().any(|line| line.function_id == churn))
        .map(|l| l.id)
        .collect();
    let samples: Vec<_> = profile
        .samples
        .iter()
        .filter(|s| s.location_ids.iter().any(|id| locations.contains(id)))
        .collect();
    assert!(!samples.is_empty());
    assert!(samples.iter().map(|s| s.values[1]).sum::<i64>() >= 65536);
    assert_eq!(samples.iter().map(|s| s.values[3]).sum::<i64>(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_heap_thread_filters_instead_of_silently_ignoring_them() {
    let capture = Command::new(env!("CARGO_BIN_EXE_pprofessor"))
        .args([
            "run",
            "--profile",
            "heap",
            "--thread",
            "worker",
            "/usr/bin/true",
        ])
        .output()
        .unwrap();
    assert!(!capture.status.success());
    assert!(String::from_utf8_lossy(&capture.stderr).contains("thread filters"));
}

#[test]
fn stopping_a_capture_emits_a_heap_checkpoint_and_reaps_the_target() {
    let (dir, fixture) = build_fixture();
    let options = pprofessor::heap::capture::CaptureOptions {
        interval: std::time::Duration::from_millis(100),
        duration: Some(std::time::Duration::from_millis(200)),
    };
    let mut pid = 0;
    let profile = pprofessor::heap::capture::run(
        fixture.as_os_str(),
        &["wait".into()],
        &options,
        &std::sync::atomic::AtomicBool::new(false),
        |target, p| {
            pid = target;
            assert!(!p.value_types.is_empty());
        },
    )
    .unwrap();
    assert!(pid > 0);
    assert!(profile.samples.iter().any(|s| s.values[3] > 0));
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exec_does_not_silently_mix_heap_epochs() {
    let (dir, fixture) = build_fixture();
    let result = Command::new(env!("CARGO_BIN_EXE_pprofessor"))
        .args(["run", "--profile", "heap", "--no-publish", "--output"])
        .arg(dir.join("exec.pb.gz"))
        .arg(&fixture)
        .arg("exec")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("exec"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn allocation_observation_preserves_glibc_api_semantics() {
    let (dir, fixture) = build_fixture();
    let baseline = Command::new(&fixture).arg("semantics").output().unwrap();
    let capture = Command::new(env!("CARGO_BIN_EXE_pprofessor"))
        .args(["run", "--profile", "heap", "--no-publish", "--output"])
        .arg(dir.join("semantics.pb.gz"))
        .arg(&fixture)
        .arg("semantics")
        .output()
        .unwrap();
    assert!(
        capture.status.success(),
        "{}",
        String::from_utf8_lossy(&capture.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&capture.stdout),
        String::from_utf8_lossy(&baseline.stdout)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn attachment_reads_existing_history_without_terminating_the_target() {
    use std::io::BufRead;
    use std::process::Stdio;
    struct Target(std::process::Child);
    impl Drop for Target {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let (dir, fixture) = build_fixture();
    let mut command = Command::new(&fixture);
    command
        .arg("attach")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(target_os = "macos")]
    command
        .env_remove("MallocStackLogging")
        .env("MallocStackLoggingNoCompact", "1")
        .env("MallocStackLoggingDirectory", &dir);
    #[cfg(target_os = "linux")]
    {
        let library = dir.join("recorder.so");
        std::fs::write(&library, pprofessor::heap::linux::recorder_library()).unwrap();
        command
            .env("LD_PRELOAD", library)
            .env("PPROFESSOR_HEAP_LOG", dir.join("allocations"));
    }
    let mut target = Target(command.spawn().unwrap());
    let mut ready = String::new();
    std::io::BufReader::new(target.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    let output = dir.join("attached.pb.gz");
    let capture = Command::new(env!("CARGO_BIN_EXE_pprofessor"))
        .args([
            "attach",
            "--profile",
            "heap",
            "--duration",
            "0.2",
            "--heap-interval",
            "100",
            "--no-publish",
            "--output",
        ])
        .arg(&output)
        .arg(target.0.id().to_string())
        .output()
        .unwrap();
    assert!(
        capture.status.success(),
        "{}",
        String::from_utf8_lossy(&capture.stderr)
    );
    assert!(
        target.0.try_wait().unwrap().is_none(),
        "attachment must leave the target running"
    );
    let report = pprofessor::report::ProfileReport::from_pprof(
        &std::fs::read(output).unwrap(),
        Some("inuse_space"),
    )
    .unwrap();
    assert!(report.total_value() >= 1048576);
    drop(target);
    std::fs::remove_dir_all(dir).unwrap();
}
