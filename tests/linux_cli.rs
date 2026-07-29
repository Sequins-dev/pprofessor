#![cfg(target_os = "linux")]

use std::io::Read;
use std::process::Command;

#[test]
fn run_profiles_a_child_process() {
    let output =
        std::env::temp_dir().join(format!("pprofessor-linux-{}.pb.gz", std::process::id()));
    let status = Command::new(env!("CARGO_BIN_EXE_pprofessor"))
        .args(["run", "--no-publish", "--duration", "0.1", "--output"])
        .arg(&output)
        .args(["/bin/sh", "-c", "while :; do :; done"])
        .status()
        .expect("failed to run pprofessor");

    assert!(status.success(), "pprofessor exited with {status}");
    let compressed = std::fs::read(&output).expect("profile was not written");
    let mut profile = Vec::new();
    flate2::read::GzDecoder::new(&compressed[..])
        .read_to_end(&mut profile)
        .expect("profile is not valid gzip");
    assert_eq!(profile.first(), Some(&0x0a), "profile is not valid pprof");
    std::fs::remove_file(output).ok();
}
