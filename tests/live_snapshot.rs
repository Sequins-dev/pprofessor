use std::time::Duration;

#[test]
fn active_profile_exposes_a_cumulative_snapshot() {
    let mut handle = pprofessor::builder().freq(100).current().unwrap();
    let profile = handle.start().unwrap();
    std::thread::sleep(Duration::from_millis(50));

    let snapshot = profile.snapshot_raw();
    assert!(snapshot.end_time >= snapshot.start_time);
    profile.stop_unsymbolicated().unwrap();
}
