use std::path::PathBuf;
use std::process::Command;
fn compile(target: Option<&str>, source: &str, output: &std::path::Path, macos: bool) {
    let mut build = cc::Build::new();
    if let Some(target) = target {
        build.target(target);
    }
    let compiler = build.get_compiler();
    let mut command = compiler.to_command();
    command.args([
        "-fPIC",
        "-O2",
        "-g",
        "-fno-omit-frame-pointer",
        "-Wall",
        "-Wextra",
        "-Werror",
    ]);
    if macos {
        command.arg("-dynamiclib");
    } else {
        command.args(["-shared", "-Wl,-z,defs", "-ldl", "-pthread"]);
    }
    let status = command
        .arg(source)
        .arg("-o")
        .arg(output)
        .status()
        .expect("launching heap helper compiler");
    assert!(status.success(), "building native heap helper");
}
fn main() {
    println!("cargo:rerun-if-changed=src/heap/recorder.c");
    println!("cargo:rerun-if-changed=src/heap/finalizer.c");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap();
    let directory = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    if os == "macos" {
        // Each CLI architecture can launch either kind of macOS target.
        let arm = directory.join("finalizer-arm64.dylib");
        let intel = directory.join("finalizer-x86_64.dylib");
        let output = directory.join("libpprofessor_finalizer.dylib");
        compile(
            Some("aarch64-apple-darwin"),
            "src/heap/finalizer.c",
            &arm,
            true,
        );
        compile(
            Some("x86_64-apple-darwin"),
            "src/heap/finalizer.c",
            &intel,
            true,
        );
        assert!(
            Command::new("/usr/bin/lipo")
                .arg("-create")
                .arg(arm)
                .arg(intel)
                .arg("-output")
                .arg(&output)
                .status()
                .expect("creating universal heap finalizer")
                .success()
        );
        assert!(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(output)
                .status()
                .expect("signing heap finalizer")
                .success()
        );
    } else if os == "linux" && env == "gnu" {
        compile(
            None,
            "src/heap/recorder.c",
            &directory.join("libpprofessor_heap.so"),
            false,
        );
    }
}
