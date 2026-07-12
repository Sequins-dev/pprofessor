.PHONY: build build-release test clean generate build-app build-app-release build-app-store run-app install app-store-check-signing app-store-check-asc app-store-archive app-store-export app-store-upload app-store-wait app-store-build-info app-store-submit

ROOT := ../..
SIGN_IDENTITY ?= -
APP_VERSION ?= 1.0
BUILD_NUMBER ?= 1
APP_STORE_ARCHIVE ?= $(CURDIR)/build/PProfessor-AppStore.xcarchive
APP_STORE_EXPORT_DIR ?= $(CURDIR)/build/app-store
APP_STORE_PACKAGE ?= $(APP_STORE_EXPORT_DIR)/PProfessor.pkg
APP_STORE_EXPORT_OPTIONS := $(CURDIR)/AppStoreExportOptions.plist
ASC_ENV = ASC_KEY_ID="$(ASC_KEY_ID)" ASC_ISSUER_ID="$(ASC_ISSUER_ID)" ASC_PRIVATE_KEY_PATH="$(ASC_PRIVATE_KEY_PATH)"

# =============================================================================
# Swift Package Manager Commands (for unit tests and CI)
# =============================================================================

build:
	xcrun swift build

build-release:
	xcrun swift build -c release

test:
	xcrun swift test

clean:
	rm -rf .build
	rm -rf *.xcodeproj

# Default target
.DEFAULT_GOAL := build

# =============================================================================
# XcodeGen Commands (for macOS app builds)
# =============================================================================
# XcodeGen generates .xcodeproj from project.yml (plain-text configuration).
# The generated .xcodeproj is gitignored — regenerate with `make generate`.

generate:
	xcodegen generate

build-app:
	$(MAKE) -C $(ROOT) cli-helper
	$(MAKE) generate
	xcodebuild -scheme PProfessor -configuration Debug build

build-app-release:
	$(MAKE) -C $(ROOT) sign-cli-helper SIGN_IDENTITY="$(SIGN_IDENTITY)"
	$(MAKE) generate
	xcodebuild -scheme PProfessor -configuration Release build

build-app-store:
	$(MAKE) generate
	xcodebuild -scheme PProfessorAppStore -configuration Release -derivedDataPath "$(TMPDIR)pprofessor-app-store-derived-data" CODE_SIGNING_ALLOWED=NO clean build
	test ! -e "$(TMPDIR)pprofessor-app-store-derived-data/Build/Products/Release/PProfessor.app/Contents/Helpers/pprofessor"
	! strings "$(TMPDIR)pprofessor-app-store-derived-data/Build/Products/Release/PProfessor.app/Contents/MacOS/PProfessor" | grep -E "AttachProcessInfo|CLIInstaller|launchAttach|task_for_pid"

app-store-check-signing:
	@test -n "$(APPLE_TEAM_ID)" || { echo "APPLE_TEAM_ID is required" >&2; exit 2; }
	@test -n "$(ASC_KEY_ID)" || { echo "ASC_KEY_ID is required" >&2; exit 2; }
	@test -n "$(ASC_ISSUER_ID)" || { echo "ASC_ISSUER_ID is required" >&2; exit 2; }
	@test -n "$(ASC_PRIVATE_KEY_PATH)" || { echo "ASC_PRIVATE_KEY_PATH is required" >&2; exit 2; }
	@test -f "$(ASC_PRIVATE_KEY_PATH)" || { echo "ASC_PRIVATE_KEY_PATH does not exist: $(ASC_PRIVATE_KEY_PATH)" >&2; exit 2; }

app-store-check-asc:
	@command -v asc >/dev/null || { echo "asc is required; install it with: brew install asc" >&2; exit 2; }
	@test -n "$(ASC_APP_ID)" || { echo "ASC_APP_ID is required" >&2; exit 2; }

app-store-archive: app-store-check-signing
	$(MAKE) generate
	mkdir -p "$(dir $(APP_STORE_ARCHIVE))"
	xcodebuild archive \
		-scheme PProfessorAppStore \
		-configuration Release \
		-archivePath "$(APP_STORE_ARCHIVE)" \
		-destination "generic/platform=macOS" \
		-allowProvisioningUpdates \
		-authenticationKeyPath "$(ASC_PRIVATE_KEY_PATH)" \
		-authenticationKeyID "$(ASC_KEY_ID)" \
		-authenticationKeyIssuerID "$(ASC_ISSUER_ID)" \
		DEVELOPMENT_TEAM="$(APPLE_TEAM_ID)" \
		MARKETING_VERSION="$(APP_VERSION)" \
		CURRENT_PROJECT_VERSION="$(BUILD_NUMBER)"

app-store-export: app-store-archive
	mkdir -p "$(APP_STORE_EXPORT_DIR)"
	xcodebuild -exportArchive \
		-archivePath "$(APP_STORE_ARCHIVE)" \
		-exportPath "$(APP_STORE_EXPORT_DIR)" \
		-exportOptionsPlist "$(APP_STORE_EXPORT_OPTIONS)" \
		-allowProvisioningUpdates \
		-authenticationKeyPath "$(ASC_PRIVATE_KEY_PATH)" \
		-authenticationKeyID "$(ASC_KEY_ID)" \
		-authenticationKeyIssuerID "$(ASC_ISSUER_ID)"
	@test -f "$(APP_STORE_PACKAGE)" || { echo "Expected exported package at $(APP_STORE_PACKAGE)" >&2; exit 2; }

app-store-upload: app-store-export app-store-check-asc
	$(ASC_ENV) asc builds upload --app "$(ASC_APP_ID)" \
		--pkg "$(APP_STORE_PACKAGE)" --version "$(APP_VERSION)" --build-number "$(BUILD_NUMBER)"

app-store-wait: app-store-check-asc
	$(ASC_ENV) asc builds wait --app "$(ASC_APP_ID)" --latest

app-store-build-info: app-store-check-asc
	$(ASC_ENV) asc builds info --app "$(ASC_APP_ID)" \
		--version "$(APP_VERSION)" --build-number "$(BUILD_NUMBER)" --platform MAC_OS

app-store-submit: app-store-check-asc
	@test -n "$(ASC_BUILD_ID)" || { echo "ASC_BUILD_ID is required; obtain it with: make app-store-build-info" >&2; exit 2; }
	$(ASC_ENV) asc review submit --app "$(ASC_APP_ID)" \
		--version "$(APP_VERSION)" --build "$(ASC_BUILD_ID)" --platform MAC_OS --confirm

run-app: build-app
	@pkill -x PProfessor 2>/dev/null || true
	@APP_PATH=$$(xcodebuild -scheme PProfessor -configuration Debug -showBuildSettings 2>/dev/null | awk -F'= ' '/ BUILT_PRODUCTS_DIR = / { print $$2; exit }')/PProfessor.app; \
	/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$$APP_PATH" 2>/dev/null || true; \
	/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f -R -trusted "$$APP_PATH"; \
	open -n "$$APP_PATH"

install: build-app-release
	@echo "Installing PProfessor.app to /Applications..."
	@APP_PATH=$$(xcodebuild -scheme PProfessor -configuration Release -showBuildSettings 2>/dev/null | grep -m1 "BUILT_PRODUCTS_DIR" | awk '{print $$3}'); \
	rm -rf /Applications/PProfessor.app; \
	cp -R "$$APP_PATH/PProfessor.app" /Applications/PProfessor.app
	@echo "Done."
