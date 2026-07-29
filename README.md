# pprofessor

PProfessor is a macOS pprof toolkit:

- `pprofessor` is a Rust CLI capture tool that samples a process and writes gzip-compressed pprof protobuf files.
- `PProfessor.app` is a native SwiftUI viewer for opening and exploring those profiles.

## Requirements

- macOS 15.0+ for the viewer
- macOS 14.0+ for the CLI
- Xcode 16+
- Rust stable
- XcodeGen for app project generation

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

The CLI uses macOS `task_for_pid`, so profiling another process requires root or a trusted signature with the debugger entitlement.

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

When PProfessor.app is open, CLI `run` and `attach` sessions are discovered automatically and stream continuously symbolized pprof deltas to the app over loopback TCP at `127.0.0.1:57557`. The listener is never exposed to the network. The final gzip profile is retained by the app alongside its session metadata. Pass `--no-publish` to keep a capture CLI-only.

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
