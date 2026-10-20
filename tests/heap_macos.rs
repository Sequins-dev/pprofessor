use pprofessor::heap::macos::parse_history;

#[test]
fn apple_history_excludes_vm_backing_and_replays_lifetimes() {
    let history = "malloc_history Report Version:  2.0\n----\nVM_ALLOC 0x1000-0x1fff [size=4096]: 0x30 (app) arena\nALLOC 0x1000-0x103f [size=64]: 0x10 (app) main | 0x20 (app) make src.c:12\nFREE  0x1000: 0x10 (app) main | 0x40 (libc) free\nALLOC 0x1000-0x101f [size=32]: 0x10 (app) main | 0x50 (app) reuse\nBinary Images:\n";
    let p = parse_history(history).unwrap().to_pprof().unwrap();
    assert_eq!(p.samples.iter().map(|s| s.values[1]).sum::<i64>(), 96);
    assert_eq!(p.samples.iter().map(|s| s.values[3]).sum::<i64>(), 32);
    assert!(
        p.functions
            .iter()
            .any(|f| p.strings.strings[f.name as usize] == "make")
    );
    assert!(
        !p.functions
            .iter()
            .any(|f| p.strings.strings[f.name as usize] == "arena")
    );
}

#[test]
fn truncated_or_unrecognized_apple_events_fail_explicitly() {
    assert!(parse_history("permission denied").is_err());
    assert!(
        parse_history("malloc_history Report Version:  2.0\n----\nALLOC broken\nBinary Images:\n")
            .is_err()
    );
}

#[test]
fn unknown_event_kinds_cannot_silently_drop_heap_measurements() {
    assert!(parse_history("malloc_history Report Version:  2.0\n----\nNEW_ALLOC_EVENT 0x1000 [size=64]\nBinary Images:\n").is_err());
}
