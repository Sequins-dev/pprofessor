.PHONY: build build-release test clean generate build-app build-app-release run-app install

ROOT := ../..
SIGN_IDENTITY ?= -

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
