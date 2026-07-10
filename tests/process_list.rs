#[cfg(target_os = "macos")]
#[test]
fn process_list_contains_the_current_process_and_only_current_user() {
    let processes = pprofessor::list_processes().unwrap();
    let current = processes
        .iter()
        .find(|process| process.pid == std::process::id())
        .expect("current process should be listed");
    assert_eq!(current.uid, unsafe { libc::geteuid() });
    assert!(processes.iter().all(|process| process.uid == current.uid));
    assert!(current.start_time_micros > 0);
}

#[test]
fn architecture_relay_is_needed_only_for_a_different_known_architecture() {
    assert_eq!(
        pprofessor::required_helper_arch("arm64", "x86_64"),
        Some("x86_64")
    );
    assert_eq!(pprofessor::required_helper_arch("arm64", "arm64"), None);
    assert_eq!(pprofessor::required_helper_arch("arm64", "unknown"), None);
}
