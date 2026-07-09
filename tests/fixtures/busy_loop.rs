/// Simple CPU-bound binary used as a profiling target in integration tests.
///
/// Runs a busy loop for 5 seconds or until killed, so the profiler has
/// something to sample. Prints a marker line on startup so the test knows
/// it's running.
fn main() {
    eprintln!("busy_loop: started");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut _x: u64 = 0;
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        for i in 0u64..10_000 {
            _x = _x.wrapping_add(i.wrapping_mul(i));
        }
    }
}
