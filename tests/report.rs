use pprofessor::PprofProfile;
use pprofessor::pprof::{Function, Line, Location, ProfileEncoder, Sample, ValueType};
use pprofessor::report::{ProfileReport, RenderOptions, ReportFormat, ReportView};
use std::io::Write;
use std::process::Command;

fn encoded_profile() -> Vec<u8> {
    let mut profile = ProfileEncoder::new();
    let samples = profile.strings.intern("samples");
    let count = profile.strings.intern("count");
    let cpu = profile.strings.intern("cpu");
    let nanoseconds = profile.strings.intern("nanoseconds");
    let source = profile.strings.intern("src/work.rs");

    profile.value_types = vec![
        ValueType {
            r#type: samples,
            unit: count,
        },
        ValueType {
            r#type: cpu,
            unit: nanoseconds,
        },
    ];
    profile.duration_nanos = 2_000_000_000;
    profile.period = 10_000_000;

    for (id, name, line) in [
        (1, "main", 10),
        (2, "worker", 20),
        (3, "hot_loop", 30),
        (4, "cold_path", 40),
        (5, "other", 50),
    ] {
        let name = profile.strings.intern(name);
        profile.functions.push(Function {
            id,
            name,
            system_name: name,
            filename: source,
            start_line: line,
        });
        profile.locations.push(Location {
            id,
            mapping_id: 0,
            address: id * 0x1000,
            lines: vec![Line {
                function_id: id,
                line,
                ..Line::default()
            }],
            ..Location::default()
        });
    }

    profile.samples = vec![
        Sample {
            location_ids: vec![3, 2, 1],
            values: vec![7, 70],
            labels: vec![],
        },
        Sample {
            location_ids: vec![4, 2, 1],
            values: vec![2, 20],
            labels: vec![],
        },
        Sample {
            location_ids: vec![5, 1],
            values: vec![1, 10],
            labels: vec![],
        },
    ];
    profile.encode()
}

#[test]
fn attributes_flat_and_cumulative_costs_to_hot_frames() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();

    assert_eq!(report.sample_type(), ("cpu", "nanoseconds"));
    assert_eq!(report.total_value(), 100);
    assert_eq!(report.top_frames()[0].function, "hot_loop");
    assert_eq!(report.top_frames()[0].flat, 70);

    let worker = report
        .top_frames()
        .iter()
        .find(|frame| frame.function == "worker")
        .unwrap();
    assert_eq!(worker.flat, 0);
    assert_eq!(worker.cumulative, 90);
}

#[test]
fn analyzes_the_same_canonical_model_used_for_encoding() {
    let profile = PprofProfile::decode(&encoded_profile()).unwrap();

    let report = ProfileReport::from_profile(&profile, None).unwrap();

    assert_eq!(report.sample_type(), ("cpu", "nanoseconds"));
    assert_eq!(report.total_value(), 100);
    assert_eq!(report.top_frames()[0].function, "hot_loop");
    assert_eq!(report.top_stacks()[0].value, 70);
}

#[test]
fn ranks_complete_stacks_in_root_to_leaf_order() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();

    let hottest = &report.top_stacks()[0];
    assert_eq!(hottest.value, 70);
    assert_eq!(
        hottest
            .frames
            .iter()
            .map(|frame| frame.function.as_str())
            .collect::<Vec<_>>(),
        ["main", "worker", "hot_loop"]
    );
}

#[test]
fn renders_a_self_contained_markdown_hotspot_summary() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();
    let markdown = report.render(&RenderOptions {
        view: ReportView::Summary,
        format: ReportFormat::Markdown,
        limit: 3,
        min_percent: 0.0,
    });

    assert!(markdown.starts_with("# pprof Hotspot Report\n"));
    assert!(markdown.contains("- Duration: 2.00s"));
    assert!(markdown.contains("- Sample records: 3"));
    assert!(markdown.contains("- Available sample types: `samples`, `cpu`"));
    assert!(markdown.contains("## Key observations"));
    assert!(markdown.contains("`hot_loop` is the largest self-cost hotspot"));
    assert!(markdown.contains("## Top frames"));
    assert!(markdown.contains("| Flat | Flat % | Cumulative | Cumulative % | Function | Source |"));
    assert!(markdown.contains("## Top stacks"));
    assert!(markdown.contains("`main` → `worker` → `hot_loop`"));
}

#[test]
fn markdown_stacks_explain_direction_and_label_each_field() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();
    let stacks = report.render(&RenderOptions {
        view: ReportView::TopStacks,
        format: ReportFormat::Markdown,
        limit: 1,
        min_percent: 0.0,
    });

    assert!(stacks.starts_with("Stack direction: root caller → leaf sample.\n\n"));
    assert!(stacks.contains("### Stack 1"));
    assert!(stacks.contains("- Cost: 70ns"));
    assert!(stacks.contains("- Percent of total: 70.00%"));
    assert!(stacks.contains("- Frames: 3"));
    assert!(stacks.contains("- Path: `main` → `worker` → `hot_loop`"));
}

#[test]
fn markdown_call_tree_explains_its_rows_and_marks_limit_truncation() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();
    let tree = report.render(&RenderOptions {
        view: ReportView::CallTree,
        format: ReportFormat::Markdown,
        limit: 2,
        min_percent: 0.0,
    });

    assert!(tree.contains("- Direction: callers at the top, callees beneath them."));
    assert!(
        tree.contains("- Row format: `inclusive cost (percent of total) [self cost] function`.")
    );
    assert!(
        tree.contains("- Inclusive cost includes samples in the function and its descendants.")
    );
    assert!(tree.contains("- Self cost includes samples attributed directly to the function."));
    assert!(tree.contains("- At most 2 call-tree nodes are shown, hottest branches first."));
    assert!(tree.contains("100ns (100.00%) [self: 0ns]  all samples"));
    assert!(tree.contains("… 3 additional call-tree nodes omitted by --limit"));
}

#[test]
fn call_tree_reports_global_truncation_outside_any_single_branch() {
    let mut profile = PprofProfile::decode(&encoded_profile()).unwrap();
    profile.samples[2].location_ids = vec![5];
    let report = ProfileReport::from_profile(&profile, None).unwrap();
    let tree = report.render(&RenderOptions {
        view: ReportView::CallTree,
        format: ReportFormat::Markdown,
        limit: 2,
        min_percent: 0.0,
    });

    assert!(tree.contains("\n… 3 additional call-tree nodes omitted by --limit\n"));
    assert!(!tree.contains("   … 3 additional call-tree nodes omitted by --limit"));
}

#[test]
fn call_tree_rows_include_leaf_self_cost() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();
    let tree = report.render(&RenderOptions {
        view: ReportView::CallTree,
        format: ReportFormat::Markdown,
        limit: 10,
        min_percent: 0.0,
    });

    assert!(tree.contains("70ns (70.00%) [self: 70ns]  hot_loop"));
    assert!(tree.contains("20ns (20.00%) [self: 20ns]  cold_path"));
}

#[test]
fn renders_a_filtered_terminal_call_tree() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();
    let tree = report.render(&RenderOptions {
        view: ReportView::CallTree,
        format: ReportFormat::Terminal,
        limit: 10,
        min_percent: 15.0,
    });

    assert!(tree.contains("main"));
    assert!(tree.contains("├─"));
    assert!(tree.contains("hot_loop"));
    assert!(tree.contains("cold_path"));
    assert!(!tree.contains("other"));
}

#[test]
fn analyze_command_writes_the_requested_view_to_stdout() {
    let path = std::env::temp_dir().join(format!("pprofessor-report-{}.pb", uuid::Uuid::new_v4()));
    std::fs::write(&path, encoded_profile()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pprofessor"))
        .args([
            "analyze",
            path.to_str().unwrap(),
            "--view",
            "stacks",
            "--format",
            "markdown",
            "--limit",
            "1",
        ])
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("### Stack 1"));
    assert!(stdout.contains("- Cost: 70ns"));
    assert!(stdout.contains("- Percent of total: 70.00%"));
    assert!(stdout.contains("- Path: `main` → `worker` → `hot_loop`"));
    assert!(!stdout.contains("cold_path"));
}

#[test]
fn terminal_top_view_includes_a_proportional_hotness_bar() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();
    let top = report.render(&RenderOptions {
        view: ReportView::TopFrames,
        format: ReportFormat::Terminal,
        limit: 2,
        min_percent: 0.0,
    });

    assert!(top.contains("HOT"));
    assert!(top.contains("██████████████"));
    assert!(top.find("hot_loop").unwrap() < top.find("cold_path").unwrap());
}

#[test]
fn cumulative_view_bars_and_order_use_cumulative_cost() {
    let report = ProfileReport::from_pprof(&encoded_profile(), None).unwrap();
    let cumulative = report.render(&RenderOptions {
        view: ReportView::TopCumulative,
        format: ReportFormat::Terminal,
        limit: 2,
        min_percent: 0.0,
    });
    let main = cumulative
        .lines()
        .find(|line| line.contains("main"))
        .unwrap();

    assert!(main.starts_with("████████████████████"));
    assert!(cumulative.find("main").unwrap() < cumulative.find("worker").unwrap());
}

#[test]
fn reads_gzip_compressed_pprof_profiles() {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&encoded_profile()).unwrap();
    let gzip = gzip.finish().unwrap();

    let report = ProfileReport::from_pprof(&gzip, Some("samples")).unwrap();

    assert_eq!(report.sample_type(), ("samples", "count"));
    assert_eq!(report.total_value(), 10);
}

#[test]
fn honors_the_profiles_default_sample_type() {
    let mut profile = encoded_profile();
    profile.extend_from_slice(&[14 << 3, 1]); // default_sample_type = "samples"

    let report = ProfileReport::from_pprof(&profile, None).unwrap();

    assert_eq!(report.sample_type(), ("samples", "count"));
    assert_eq!(report.total_value(), 10);
}

#[test]
fn preserves_pprof_inline_frame_order() {
    let mut profile = ProfileEncoder::new();
    let samples = profile.strings.intern("samples");
    let count = profile.strings.intern("count");
    let source = profile.strings.intern("inline.rs");
    profile.value_types.push(ValueType {
        r#type: samples,
        unit: count,
    });
    for (id, name) in [(1, "main"), (2, "outer"), (3, "inlined_leaf")] {
        let name = profile.strings.intern(name);
        profile.functions.push(Function {
            id,
            name,
            system_name: name,
            filename: source,
            start_line: 0,
        });
    }
    profile.locations = vec![
        Location {
            id: 1,
            mapping_id: 0,
            address: 0x1000,
            lines: vec![Line {
                function_id: 1,
                line: 10,
                ..Line::default()
            }],
            ..Location::default()
        },
        Location {
            id: 2,
            mapping_id: 0,
            address: 0x2000,
            // Inline frames are innermost-to-outermost in the protobuf.
            lines: vec![
                Line {
                    function_id: 3,
                    line: 30,
                    ..Line::default()
                },
                Line {
                    function_id: 2,
                    line: 20,
                    ..Line::default()
                },
            ],
            ..Location::default()
        },
    ];
    profile.samples.push(Sample {
        location_ids: vec![2, 1],
        values: vec![10],
        labels: vec![],
    });

    let report = ProfileReport::from_pprof(&profile.encode(), None).unwrap();

    assert_eq!(
        report.top_stacks()[0]
            .frames
            .iter()
            .map(|frame| frame.function.as_str())
            .collect::<Vec<_>>(),
        ["main", "outer", "inlined_leaf"]
    );
    assert_eq!(report.top_frames()[0].function, "inlined_leaf");
}

#[test]
fn counts_a_recursive_function_once_per_sample_for_cumulative_cost() {
    let mut profile = ProfileEncoder::new();
    let samples = profile.strings.intern("samples");
    let count = profile.strings.intern("count");
    let source = profile.strings.intern("recursive.rs");
    let main = profile.strings.intern("main");
    let recurse = profile.strings.intern("recurse");
    profile.value_types.push(ValueType {
        r#type: samples,
        unit: count,
    });
    profile.functions = vec![
        Function {
            id: 1,
            name: main,
            system_name: main,
            filename: source,
            start_line: 0,
        },
        Function {
            id: 2,
            name: recurse,
            system_name: recurse,
            filename: source,
            start_line: 0,
        },
    ];
    profile.locations = vec![
        Location {
            id: 1,
            mapping_id: 0,
            address: 0x1000,
            lines: vec![Line {
                function_id: 1,
                line: 10,
                ..Line::default()
            }],
            ..Location::default()
        },
        Location {
            id: 2,
            mapping_id: 0,
            address: 0x2000,
            lines: vec![Line {
                function_id: 2,
                line: 20,
                ..Line::default()
            }],
            ..Location::default()
        },
        Location {
            id: 3,
            mapping_id: 0,
            address: 0x3000,
            lines: vec![Line {
                function_id: 2,
                line: 21,
                ..Line::default()
            }],
            ..Location::default()
        },
    ];
    profile.samples.push(Sample {
        location_ids: vec![3, 2, 1],
        values: vec![10],
        labels: vec![],
    });

    let report = ProfileReport::from_pprof(&profile.encode(), None).unwrap();
    let recurse = report
        .top_frames()
        .iter()
        .find(|frame| frame.function == "recurse")
        .unwrap();

    assert_eq!(recurse.flat, 10);
    assert_eq!(recurse.cumulative, 10);
}
