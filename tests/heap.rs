use pprofessor::heap::{HeapFrame, HeapProfile, HeapValues, NativeHeapTracker};

fn frame(name: &str) -> HeapFrame {
    HeapFrame {
        function: name.into(),
        ..Default::default()
    }
}

#[test]
fn allocation_lifetimes_keep_cumulative_cost_and_release_original_stack() {
    let mut heap = NativeHeapTracker::new("native");
    heap.allocate(1, 64, vec![frame("first")]).unwrap();
    heap.allocate(2, 128, vec![frame("second")]).unwrap();
    heap.release(1).unwrap();
    heap.allocate(1, 32, vec![frame("reused")]).unwrap();
    let profile = heap.snapshot().unwrap().to_pprof().unwrap();
    assert_eq!(
        profile
            .samples
            .iter()
            .map(|s| s.values.clone())
            .collect::<Vec<_>>(),
        vec![vec![1, 64, 0, 0], vec![1, 32, 1, 32], vec![1, 128, 1, 128]]
    );
    assert_eq!(
        profile.strings.strings[profile.default_sample_type as usize],
        "inuse_space"
    );
    assert_eq!(
        pprofessor::PprofProfile::decode(&profile.encode()).unwrap(),
        profile
    );
}

#[test]
fn duplicate_live_addresses_and_unknown_frees_are_not_silently_accepted() {
    let mut heap = NativeHeapTracker::new("native");
    heap.allocate(1, 64, vec![frame("first")]).unwrap();
    assert!(heap.allocate(1, 32, vec![frame("bad")]).is_err());
    assert!(heap.release(2).is_err());
}

#[test]
fn runtime_snapshots_use_pprof_frames_and_heap_domain_labels() {
    let mut heap = HeapProfile::new("v8:isolate-1");
    heap.add_sample(
        vec![frame("app.js:render")],
        HeapValues {
            alloc_objects: 5,
            alloc_space: 160,
            inuse_objects: 2,
            inuse_space: 64,
        },
    )
    .unwrap();
    let profile = heap.to_pprof().unwrap();
    let label = &profile.samples[0].labels[0];
    assert_eq!(profile.strings.strings[label.key as usize], "heap");
    assert_eq!(
        profile.strings.strings[label.str_index as usize],
        "v8:isolate-1"
    );
    assert_eq!(
        profile.strings.strings[profile.functions[0].name as usize],
        "app.js:render"
    );
}

#[test]
fn values_must_not_wrap_pprof_signed_integers() {
    let mut heap = HeapProfile::new("native");
    assert!(
        heap.add_sample(
            vec![frame("overflow")],
            HeapValues {
                alloc_space: u64::MAX,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn native_exports_keep_binary_mapping_identity() {
    use pprofessor::heap::HeapMapping;
    let mut heap = HeapProfile::new("native");
    heap.add_sample(
        vec![HeapFrame {
            address: 0x1120,
            function: "allocate".into(),
            mapping: Some(HeapMapping {
                memory_start: 0x1000,
                memory_limit: 0x2000,
                file_offset: 0,
                filename: "app".into(),
                build_id: "build-1".into(),
            }),
            ..Default::default()
        }],
        HeapValues {
            alloc_objects: 1,
            alloc_space: 64,
            inuse_objects: 1,
            inuse_space: 64,
        },
    )
    .unwrap();
    let p = heap.to_pprof().unwrap();
    assert_eq!(p.locations[0].mapping_id, p.mappings[0].id);
    assert_eq!(
        p.strings.strings[p.mappings[0].build_id as usize],
        "build-1"
    );
}

#[test]
fn empty_heap_keeps_its_domain_in_a_zero_valued_pprof_sample() {
    let p = HeapProfile::new("native").to_pprof().unwrap();
    assert_eq!(p.samples.len(), 1);
    assert_eq!(p.samples[0].values, vec![0; 4]);
    assert_eq!(
        p.strings.strings[p.samples[0].labels[0].str_index as usize],
        "native"
    );
}

#[test]
fn mixed_heap_domains_require_explicit_cli_selection() {
    let mut first = HeapProfile::new("native");
    first
        .add_sample(
            vec![frame("backing")],
            HeapValues {
                inuse_space: 64,
                ..Default::default()
            },
        )
        .unwrap();
    let mut p = first.to_pprof().unwrap();
    let label = p.strings.intern("v8");
    let mut sample = p.samples[0].clone();
    sample.values = vec![0, 0, 0, 40];
    sample.labels[0].str_index = label;
    p.samples.push(sample);
    assert!(pprofessor::heap::select_domain(&mut p.clone(), None).is_err());
    pprofessor::heap::select_domain(&mut p, Some("v8")).unwrap();
    assert_eq!(p.samples.len(), 1);
    assert_eq!(p.samples[0].values[3], 40);
}

#[test]
fn runtime_documentation_link_survives_pprof_export() {
    let mut p = HeapProfile::new("v8").to_pprof().unwrap();
    p.doc_url = p.strings.intern("https://v8.dev/docs");
    let decoded = pprofessor::PprofProfile::decode(&p.encode()).unwrap();
    assert_eq!(
        decoded.strings.strings[decoded.doc_url as usize],
        "https://v8.dev/docs"
    );
}
