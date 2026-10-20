use pprofessor::heap::compare::difference;
use pprofessor::heap::{HeapFrame, HeapProfile, HeapValues};
fn profile(domain: &str, bytes: u64, base: u64) -> pprofessor::PprofProfile {
    let mut p = HeapProfile::new(domain);
    p.add_sample(
        vec![HeapFrame {
            address: base,
            function: "cache".into(),
            file: "cache.rs".into(),
            line: 12,
            ..Default::default()
        }],
        HeapValues {
            alloc_objects: 1,
            alloc_space: bytes,
            inuse_objects: 1,
            inuse_space: bytes,
        },
    )
    .unwrap();
    p.to_pprof().unwrap()
}
#[test]
fn comparison_matches_symbolic_stacks_across_aslr_and_preserves_negative_values() {
    let p = difference(
        &profile("native", 128, 0x1000),
        &profile("native", 64, 0x2000),
    )
    .unwrap();
    assert_eq!(p.samples.len(), 1);
    assert_eq!(p.samples[0].values, vec![0, -64, 0, -64]);
    assert_eq!(pprofessor::PprofProfile::decode(&p.encode()).unwrap(), p);
}
#[test]
fn comparison_rejects_overlapping_but_different_domains() {
    assert!(difference(&profile("native", 128, 1), &profile("v8", 64, 1)).is_err());
}
#[test]
fn malformed_profile_references_fail_without_panicking() {
    let mut p = profile("native", 128, 1);
    p.functions[0].filename = u64::MAX;
    assert!(difference(&p, &profile("native", 64, 1)).is_err());
}
#[test]
fn cli_exports_an_interoperable_signed_difference() {
    let dir = std::env::temp_dir().join(format!("heap-diff-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let a = dir.join("base.pb");
    let b = dir.join("current.pb");
    let out = dir.join("diff.pb.gz");
    std::fs::write(&a, profile("native", 128, 1).encode()).unwrap();
    std::fs::write(&b, profile("native", 64, 2).encode()).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_pprofessor"))
        .arg("compare")
        .arg(&a)
        .arg(&b)
        .arg("--output")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = pprofessor::report::ProfileReport::from_pprof(
        &std::fs::read(out).unwrap(),
        Some("inuse_space"),
    )
    .unwrap();
    assert_eq!(report.total_value(), 64);
    std::fs::remove_dir_all(dir).unwrap();
}
