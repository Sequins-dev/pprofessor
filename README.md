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

Build the universal release CLI helper used by app and release packaging:

```sh
make cli-helper-universal
```

## CLI Usage

```sh
pprofessor run --output profile.pb.gz ./my-app
sudo pprofessor attach --output profile.pb.gz 12345
```

The CLI uses macOS `task_for_pid`, so profiling another process requires root or a trusted signature with the debugger entitlement.

## App CLI Installation

Release app builds bundle the `pprofessor` CLI at `PProfessor.app/Contents/Helpers/pprofessor`.
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
