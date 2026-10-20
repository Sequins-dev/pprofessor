# pprofessor

PProfessor is a native pprof toolkit:

- `pprofessor` is a Rust CLI capture tool for macOS and Linux that samples a process and writes gzip-compressed pprof protobuf files.
- `PProfessor.app` is a native SwiftUI viewer for opening and exploring those profiles.

## Requirements

- CLI: Rust stable on macOS 14.0+ or Linux
- Viewer: macOS 15.0+, Xcode 16+, and XcodeGen

The viewer remains macOS-only. Linux support currently covers the CLI capture
workflow and Rust profiling APIs, not the UI.

## Build

```sh
make build
make test
```

Build and run the app:

```sh
make run-app
```

Build the sandboxed App Store viewer variant, with attachment and the bundled CLI excluded:

```sh
make build-app-store
```

Archive, export, upload, and submit the App Store build with:

```sh
export APPLE_TEAM_ID="YOUR_TEAM_ID"
export ASC_KEY_ID="YOUR_KEY_ID"
export ASC_ISSUER_ID="YOUR_ISSUER_ID"
export ASC_PRIVATE_KEY_PATH="$HOME/.asc/AuthKey_YOUR_KEY_ID.p8"
export ASC_APP_ID="YOUR_NUMERIC_APP_STORE_CONNECT_APP_ID"

make app-store-upload APP_VERSION=1.0 BUILD_NUMBER=1
make app-store-wait
make app-store-build-info APP_VERSION=1.0 BUILD_NUMBER=1
make app-store-submit APP_VERSION=1.0 BUILD_NUMBER=1 ASC_BUILD_ID="BUILD_RESOURCE_ID"
```

The upload and submission tasks require the `asc` CLI (`brew install asc`). Submission is deliberately separate and requires the processed build resource ID so uploading cannot accidentally send a version to review.

Build the universal release CLI helper used by app and release packaging:

```sh
make cli-helper-universal
```

## CLI Usage

```sh
pprofessor run --output profile.pb.gz ./my-app
sudo pprofessor attach --output profile.pb.gz 12345
pprofessor processes
pprofessor analyze profile.pb.gz
```

On macOS, the CLI uses `task_for_pid`, so profiling another process requires
root or a trusted signature with the debugger entitlement. On Linux, `run`
works with the normal parent-child ptrace permission; `attach` follows the
system ptrace policy and may require `CAP_SYS_PTRACE`, root, or a less
restrictive Yama configuration.

### Native heap profiling

Capture native heap allocations on macOS and Linux:

```sh
pprofessor run --profile heap --output heap.pb.gz ./my-app
pprofessor run --profile heap --heap-interval 2000 --duration 10 ./my-app
pprofessor analyze heap.pb.gz --sample-type inuse_space
pprofessor analyze heap.pb.gz --sample-type alloc_space
pprofessor compare before.pb.gz after.pb.gz --output growth.pb.gz
```

Heap profiles contain `alloc_objects` and `alloc_space` (cumulative allocation
count and bytes), and `inuse_objects` and `inuse_space` (outstanding allocations).
The default metric is `inuse_space`. Bytes are allocator-reported block sizes,
including allocation rounding, rather than RSS or requested payload sizes.
Freed allocations retain their cumulative cost and contribute zero live bytes.
Comparisons export signed values: current minus base, matched by allocation
stacks and native image identity rather than relocated program counters.

The app's sample selector switches between bytes and object counts. Live heap
checkpoints replace the previous heap; they are never added together. Signed
profiles have Increase/Decrease views. Save Profile exports the complete selected
pprof profile, including from a live session. Every displayed measurement is
represented in that export. Heap captures do not create RSS timelines, object
reference graphs, or a proprietary profile format.

Capture is **exact allocation tracing**, intended for diagnostics. Recording
all allocation stacks and free events adds CPU, disk, and memory overhead;
`--heap-interval` controls checkpoint collection, not allocation sampling.
Stacks are limited to the collector's available unwinding depth (64 frames on
Linux). Recording begins when the platform recorder initializes, so earlier
runtime/loader allocations may have no recorded allocation site. Coverage is
included in the pprof comments. Capture is per process; forked children are not
combined with the parent. Replacing the target with `exec` ends the recording
epoch and is rejected; profile the final executable directly. Thread filters are rejected because allocations can
be freed by a different thread.

On **macOS**, capture uses Apple's full MallocStackLogging history through
`malloc_history`, with VM backing regions excluded to avoid counting malloc
storage twice. A bundled universal finalization helper allows a final snapshot
before normal exit or `_exit`, including short-lived programs. The target must
permit dylib injection and debugger inspection; use a development build when
hardened runtime or library validation prevents loading the helper. The tool
never changes the target's signature or weakens system protections.

On **Linux**, an embedded allocation observer interposes the dynamically linked
glibc system allocator. It records malloc/calloc/realloc, aligned allocation,
and free operations, including cross-thread releases. Static executables,
secure-execution targets, other architectures, and custom allocators that
bypass glibc are outside this backend's coverage. Existing `LD_PRELOAD` or
`DYLD_INSERT_LIBRARIES` settings are rejected instead of silently composing
potentially incompatible allocator interposers. Allocator-specific and managed
profiles can use the runtime integration below.

To attach, allocation recording must already be enabled; past allocation
stacks cannot be recovered from an uninstrumented process:

```sh
# macOS: launch with full, uncompacted history (do not also set MallocStackLogging).
MallocStackLoggingNoCompact=1 ./my-app
pprofessor attach --profile heap --duration 5 --output heap.pb.gz PID

# Linux: install the observer, then launch with a new private log path.
pprofessor heap-recorder --output /absolute/path/libpprofessor_heap.so
mkdir -m 700 /absolute/path/heap-recording
PPROFESSOR_HEAP_LOG=/absolute/path/heap-recording/events \
  LD_PRELOAD=/absolute/path/libpprofessor_heap.so ./my-app
pprofessor attach --profile heap --duration 5 --output heap.pb.gz PID
```

Linux attach reads the log path from the target environment and verifies the
log's PID and permissions. Attaching does not terminate the target or disable
its recorder; remove recording configuration on the next launch when finished.
A normal `run` completes at target exit; stopping a `run` early terminates the
owned target. Fatal termination on macOS can prevent final capture; if an earlier
checkpoint exists it is exported with an explicit incomplete-finalization
comment, otherwise capture fails.

### Managed runtimes and custom allocators

A runtime can emit an ordinary heap pprof file and import it into the app, or:

```sh
pprofessor publish runtime-heap.pb.gz --name 'JavaScript heap'
cargo run --example heap_arena -- arena.pb
```

Rust integrations can use `heap::HeapProfile` for complete four-metric snapshots,
or construct `PprofProfile` directly when only some metrics are available.
`SessionPublisher::publish_heap_checkpoint` validates and publishes those
profiles through the existing local session transport. Repeated updates must
be cumulative checkpoints; the app replaces the prior checkpoint. Start a
separate session for each heap domain. An optional standard pprof sample label
named `heap` identifies the domain; the app selects one domain at a time when
an imported profile contains multiple domains. CLI analysis requires `--heap NAME`
for such profiles. Native backing storage and
managed objects can overlap and must not be summed. Do not supply zero for
unavailable metrics: omit those sample types. Sampling providers must supply
appropriately weighted estimates and preserve their sampling period in pprof.

This integration reports allocation statistics and source stacks, including
logical objects allocated inside arenas or GC heaps. It does not claim to infer
GC object liveness from native malloc events or export object-reference graphs.
Runtime adapters must use their own allocation/liveness APIs to produce the
measurements. The shipped arena example demonstrates this path without adding
a runtime dependency to the profiler.

### Terminal and Markdown analysis

`analyze` reads raw or gzip-compressed pprof protobuf files and highlights the
work that accounts for the most measured cost. The default `summary` combines
profile metadata, concise hotspot observations, flat and cumulative frame
costs, and the highest-cost complete stacks:

```sh
pprofessor analyze profile.pb.gz
```

Focused views make it easier to explore a profile without opening the app:

```sh
# Self/flat cost by function, with terminal hotness bars
pprofessor analyze profile.pb.gz --view top --limit 20

# Functions ranked by cumulative cost
pprofessor analyze profile.pb.gz --view cumulative

# Complete root-to-leaf stacks
pprofessor analyze profile.pb.gz --view stacks

# A filtered root-to-leaf call tree
pprofessor analyze profile.pb.gz --view tree --min-percent 2
```

Markdown output is structured for LLM context, issue reports, and documents:

```sh
pprofessor analyze profile.pb.gz \
  --format markdown \
  --output hotspots.md
```

Use `--sample-type NAME` (or a zero-based sample type index) to inspect a
specific metric. Without it, PProfessor follows pprof selection semantics:
`default_sample_type` when present, otherwise the last sample type. `report` is
an alias for `analyze`.

The Rust library exposes the same reversible representation used by the CLI:

```rust
let profile = pprofessor::PprofProfile::decode(&protobuf_bytes)?;
let json = serde_json::to_string_pretty(&profile)?;
let protobuf_bytes = profile.encode();
let report = pprofessor::report::ProfileReport::from_profile(&profile, None)?;
```

`PprofProfile` implements Serde's `Serialize` and `Deserialize` traits.
`ProfileEncoder` remains available as a compatibility alias.

## Rust API

On both macOS and Linux, the Rust API supports profiling the current process
and profiling a closure in addition to the CLI workflows:

```rust
let mut handle = pprofessor::builder().freq(99).current()?;
let session = handle.start()?;
run_workload();
let profile = session.stop()?;

let (result, profile) = pprofessor::builder().profile(|| run_workload())?;
```

On macOS, in-process profiling uses Mach task and thread APIs and requires no
special permissions. On Linux, it uses per-thread `perf_event_open` software
CPU-clock events. Linux profiling normally works without elevated privileges,
but follows the host's `perf_event_paranoid` and `CAP_PERFMON` policy.

When PProfessor.app is open, CLI `run` and `attach` sessions are discovered automatically and stream symbolized pprof stack deltas or heap checkpoints to the app over loopback TCP at `127.0.0.1:57557`. The listener is never exposed to the network. The final gzip profile is retained by the app alongside its session metadata. Pass `--no-publish` to keep a capture CLI-only.

The GitHub release app's Attach button lists attachable processes owned by the current user. The App Store build omits all in-app attachment controls; live sessions still appear when captured by a separately installed CLI. Live, completed, imported, failed, and interrupted captures remain in the Sessions sidebar until explicitly deleted.

## App CLI Installation

GitHub release app builds bundle the `pprofessor` CLI at `PProfessor.app/Contents/Helpers/pprofessor`. The App Store build does not bundle or install an executable helper; install the free CLI separately from Homebrew or a GitHub release to stream live profiles into it.
Use `Tools > Install CLI tools` in the app to install it to:

```sh
~/.local/bin/pprofessor
```

Add `~/.local/bin` to your shell `PATH` if needed.

## Layout

```text
.
├── src/                 # Rust CLI/library
├── tests/               # Rust tests
├── apps/macos/          # SwiftUI app and PProfessorKit
├── Cargo.toml
└── Makefile
```
