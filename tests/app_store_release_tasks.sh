#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MACOS_DIR="$ROOT/apps/macos"

assert_contains() {
    local haystack="$1"
    local needle="$2"
    if [[ "$haystack" != *"$needle"* ]]; then
        echo "Expected dry run to contain: $needle" >&2
        exit 1
    fi
}

grep -q 'PRODUCT_BUNDLE_IDENTIFIER: dev.sequins.pprofessor' "$MACOS_DIR/project.yml"

common=(
    APPLE_TEAM_ID=TEAM123
    ASC_KEY_ID=KEY123
    ASC_ISSUER_ID=00000000-0000-0000-0000-000000000000
    ASC_PRIVATE_KEY_PATH=/tmp/AuthKey_KEY123.p8
    ASC_APP_ID=123456789
    APP_VERSION=1.2.3
    BUILD_NUMBER=42
)

archive_output="$(make -n -C "$MACOS_DIR" app-store-archive "${common[@]}")"
assert_contains "$archive_output" "-scheme PProfessorAppStore"
assert_contains "$archive_output" "MARKETING_VERSION=\"1.2.3\""
assert_contains "$archive_output" "CURRENT_PROJECT_VERSION=\"42\""
assert_contains "$archive_output" "-authenticationKeyPath \"/tmp/AuthKey_KEY123.p8\""

upload_output="$(make -n -C "$MACOS_DIR" app-store-upload "${common[@]}")"
assert_contains "$upload_output" 'ASC_KEY_ID="KEY123" ASC_ISSUER_ID="00000000-0000-0000-0000-000000000000"'
assert_contains "$upload_output" 'asc builds upload --app "123456789"'
assert_contains "$upload_output" '--pkg "'
assert_contains "$upload_output" '--version "1.2.3" --build-number "42"'

submit_output="$(make -n -C "$MACOS_DIR" app-store-submit ASC_BUILD_ID=build-uuid "${common[@]}")"
assert_contains "$submit_output" 'asc review submit --app "123456789"'
assert_contains "$submit_output" '--version "1.2.3" --build "build-uuid"'
assert_contains "$submit_output" '--platform MAC_OS --confirm'
