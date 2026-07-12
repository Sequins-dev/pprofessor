# pprofessor

A native macOS CPU sampling profiler that outputs gzip-compressed [pprof](https://github.com/google/pprof) protobuf files.

## Requirements

- macOS 14.0+
- Swift 6.0+ (Xcode 16+)
- For `attach`: root or `com.apple.security.cs.debugger` entitlement

## Build

```sh
swift build -c release
```

The repository root also provides two app variants:

```sh
make build-app-release  # GitHub build with attach UI and bundled CLI
make build-app-store    # sandboxed viewer without attach UI or bundled CLI
```

## Usage

```sh
# Profile a child process
pprofessor run ./my-app

# Attach to a running process
sudo pprofessor attach 12345

# View profile
go tool pprof -http=:8080 profile.pb.gz
```

While the app is running, CLI captures publish live profile updates to its loopback-only TCP listener at `127.0.0.1:57557`. The App Store viewer supports these incoming sessions even though capture and attachment are distributed separately.

## Options

- `--freq <HZ>` — sampling frequency (default: 99)
- `-o/--output <PATH>` — output file (default: profile.pb.gz)
