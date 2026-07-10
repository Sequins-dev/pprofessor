use pprofessor::{FrameKind, STREAM_HEADER_LEN, SessionHello, SessionPublisher, StreamHeader};
use pprofessor::{RawProfile, RawProfileCursor, RawSampleSeries};
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::time::Instant;

#[test]
fn stream_header_round_trips_in_network_byte_order() {
    let header = StreamHeader {
        major: 1,
        minor: 0,
        kind: FrameKind::ProfileDelta,
        flags: 3,
        sequence: 42,
        payload_len: 65_536,
    };

    let encoded = header.encode();
    assert_eq!(encoded.len(), STREAM_HEADER_LEN);
    assert_eq!(&encoded[0..4], b"PPRS");
    assert_eq!(StreamHeader::decode(&encoded).unwrap(), header);
}

#[test]
fn stream_header_rejects_unknown_kind() {
    let mut encoded = StreamHeader {
        major: 1,
        minor: 0,
        kind: FrameKind::Hello,
        flags: 0,
        sequence: 0,
        payload_len: 0,
    }
    .encode();
    encoded[8..10].copy_from_slice(&999u16.to_be_bytes());
    assert!(StreamHeader::decode(&encoded).is_err());
}

#[test]
fn raw_profile_cursor_returns_only_unpublished_counts() {
    let now = Instant::now();
    let mut profile = RawProfile {
        stacks: HashMap::from([(
            (1, vec![0x10, 0x20]),
            RawSampleSeries::timed(vec![10, 20, 30]),
        )]),
        thread_names: HashMap::from([(1, "main".to_string())]),
        start_time: now,
        end_time: now,
        images: Vec::new(),
    };
    let mut cursor = RawProfileCursor::default();

    let first = cursor.delta(&profile).unwrap();
    assert_eq!(first.stacks[&(1, vec![0x10, 0x20])].count, 3);
    assert_eq!(
        first.stacks[&(1, vec![0x10, 0x20])].timestamps_nanos,
        Some(vec![10, 20, 30])
    );

    profile.stacks.insert(
        (1, vec![0x10, 0x20]),
        RawSampleSeries::timed(vec![10, 20, 30, 40, 50]),
    );
    let second = cursor.delta(&profile).unwrap();
    assert_eq!(second.stacks[&(1, vec![0x10, 0x20])].count, 2);
    assert_eq!(
        second.stacks[&(1, vec![0x10, 0x20])].timestamps_nanos,
        Some(vec![40, 50])
    );
    assert!(cursor.delta(&profile).is_none());
}

#[test]
fn raw_sample_series_supports_untimed_aggregates() {
    let series = RawSampleSeries::untimed(7);
    assert_eq!(series.count, 7);
    assert_eq!(series.timestamps_nanos, None);
}

#[test]
fn publisher_sends_hello_before_profile_data() {
    let path = std::env::temp_dir().join(format!(
        "pprofessor-stream-test-{}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let reader = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut header = [0u8; STREAM_HEADER_LEN];
        stream.read_exact(&mut header).unwrap();
        let hello = StreamHeader::decode(&header).unwrap();
        let mut payload = vec![0; hello.payload_len as usize];
        stream.read_exact(&mut payload).unwrap();
        stream.read_exact(&mut header).unwrap();
        let profile = StreamHeader::decode(&header).unwrap();
        (hello.kind, profile.kind)
    });

    let mut publisher = SessionPublisher::new(
        path.clone(),
        SessionHello::new("session-1", "attach", 42, "target", 99),
    );
    assert!(publisher.send(FrameKind::ProfileDelta, b"profile").unwrap());
    assert_eq!(
        reader.join().unwrap(),
        (FrameKind::Hello, FrameKind::ProfileDelta)
    );
    let _ = std::fs::remove_file(path);
}
