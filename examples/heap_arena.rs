//! A pool reports logical objects independently of its native backing memory.
//! Run: cargo run --example heap_arena -- arena.pb
use pprofessor::heap::{HeapFrame, HeapProfile, HeapValues};
fn main() -> anyhow::Result<()> {
    let mut storage = [0u8; 4096];
    for i in 0..8 {
        storage[i * 32] = i as u8;
    }
    // Six objects were released back to the pool; backing storage remains.
    let mut heap = HeapProfile::new("example:arena");
    heap.add_sample(
        vec![HeapFrame {
            function: "arena.allocate".into(),
            file: file!().into(),
            line: line!() as i64,
            ..Default::default()
        }],
        HeapValues {
            alloc_objects: 8,
            alloc_space: 256,
            inuse_objects: 2,
            inuse_space: 64,
        },
    )?;
    let profile = heap.to_pprof()?;
    std::fs::write(
        std::env::args_os()
            .nth(1)
            .unwrap_or_else(|| "arena.pb".into()),
        profile.encode(),
    )?;
    std::hint::black_box(storage);
    Ok(())
}
