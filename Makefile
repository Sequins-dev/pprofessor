.PHONY: build build-release test clean rust-build rust-release rust-test rust-lint rust-fmt swift-build swift-test generate build-app build-app-release run-app cli-helper cli-helper-universal sign-cli-helper install

CLI_NAME := pprofessor
CLI_HELPER_DIR := target/cli-helper
CLI_HELPER := $(CLI_HELPER_DIR)/$(CLI_NAME)
MACOS_DEPLOYMENT_TARGET ?= 14.0
SIGN_IDENTITY ?= -

build: rust-build swift-build

build-release: rust-release swift-build-release

test: rust-test swift-test

clean:
	cargo clean
	$(MAKE) -C apps/macos clean
	rm -rf $(CLI_HELPER_DIR)

rust-build:
	cargo build

rust-release:
	cargo build --release

rust-test:
	cargo test

rust-lint:
	cargo clippy --all-targets -- -D warnings

rust-fmt:
	cargo fmt

swift-build:
	$(MAKE) -C apps/macos build

swift-build-release:
	$(MAKE) -C apps/macos build-release

swift-test:
	$(MAKE) -C apps/macos test

generate:
	$(MAKE) -C apps/macos generate

build-app:
	$(MAKE) -C apps/macos build-app

build-app-release:
	$(MAKE) -C apps/macos build-app-release

run-app:
	$(MAKE) -C apps/macos run-app

install:
	$(MAKE) -C apps/macos install

cli-helper: rust-build
	@mkdir -p $(CLI_HELPER_DIR)
	@cp target/debug/$(CLI_NAME) $(CLI_HELPER)
	@chmod 755 $(CLI_HELPER)
	codesign --force --entitlements entitlements.plist --sign "$(SIGN_IDENTITY)" $(CLI_HELPER)
	@echo "Built $(CLI_HELPER)"

cli-helper-universal:
	@mkdir -p $(CLI_HELPER_DIR)
	MACOSX_DEPLOYMENT_TARGET=$(MACOS_DEPLOYMENT_TARGET) cargo build --release --target aarch64-apple-darwin
	MACOSX_DEPLOYMENT_TARGET=$(MACOS_DEPLOYMENT_TARGET) cargo build --release --target x86_64-apple-darwin
	lipo -create \
		target/aarch64-apple-darwin/release/$(CLI_NAME) \
		target/x86_64-apple-darwin/release/$(CLI_NAME) \
		-output $(CLI_HELPER)
	chmod 755 $(CLI_HELPER)
	@echo "Built universal $(CLI_HELPER)"

sign-cli-helper: cli-helper-universal
	codesign --force --options runtime --timestamp --entitlements entitlements.plist --sign "$(SIGN_IDENTITY)" $(CLI_HELPER)
