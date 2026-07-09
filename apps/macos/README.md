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

## Usage

```sh
# Profile a child process
pprofessor run ./my-app

# Attach to a running process
sudo pprofessor attach 12345

# View profile
go tool pprof -http=:8080 profile.pb.gz
```

## Options

- `--freq <HZ>` — sampling frequency (default: 99)
- `-o/--output <PATH>` — output file (default: profile.pb.gz)
