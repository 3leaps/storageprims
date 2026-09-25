# storageprims Makefile
# Cross-language cloud storage primitives library
#
# Compliant with docs/standards/repository-conventions.md
#
# Quick Reference:
#   make help       - Show all available targets
#   make bootstrap  - Install tools (sfetch -> goneat)
#   make check      - Run all quality checks (fmt, lint, test, deny)
#   make fmt        - Format code (cargo fmt + goneat format)
#   make build      - Build all crates

.NOTPARALLEL: release

.PHONY: all help bootstrap bootstrap-foundation bootstrap-rust-tools bootstrap-force
.PHONY: tools check test test-integration-s3 test-integration-ffi fmt fmt-check lint build clean version
.PHONY: precommit prepush deny audit dependency-scan msrv
.PHONY: build-release build-ffi cbindgen pr-final
.PHONY: install uninstall install-test
.PHONY: version-patch version-minor version-major version-set version-sync version-check
.PHONY: release-check release-preflight release-tooling-test release-guard-tag-version
.PHONY: release-clean release-download release-notes release-checksums release-sign
.PHONY: release-export-keys release-verify-checksums release-verify-signatures
.PHONY: release-verify-keys release-verify release-upload release
.PHONY: release-crates-list release-crates-dry-run release-crates-verify
.PHONY: release-tag release-push-tag release-verify-tag release-verify-remote-tag release-insert-anchors

# -----------------------------------------------------------------------------
# Configuration
# -----------------------------------------------------------------------------

VERSION_FILE := VERSION
VERSION := $(shell tr -d ' \t\r\n' < $(VERSION_FILE) 2>/dev/null || echo dev)

# Tool installation directory
# Bootstrap installs sfetch to repo-local bin/
BIN_DIR := $(CURDIR)/bin

# Pinned tool versions for reproducibility
SFETCH_VERSION := v0.4.11
GONEAT_VERSION ?= v0.6.0
GONEAT_FORMAT_FAIL_ON ?= medium
NEXTEST_VERSION ?= 0.9.128
CARGO_EDIT_VERSION ?= 0.13.10
CBINDGEN_VERSION ?= 0.29.2

RELEASE_DIR := $(CURDIR)/dist/release
INSTALL_LIBDIR ?= $(HOME)/.local/lib
INSTALL_INCLUDEDIR ?= $(HOME)/.local/include

# Tool paths
# sfetch: repo-local (trust anchor) or PATH
# goneat: user-space PATH only (like prettier, biome, ruff)
SFETCH = $(shell [ -x "$(BIN_DIR)/sfetch" ] && echo "$(BIN_DIR)/sfetch" || command -v sfetch 2>/dev/null)
GONEAT = $(shell command -v goneat 2>/dev/null)
CARGO_NEXTEST = $(shell command -v cargo-nextest 2>/dev/null)

# Rust toolchain (assumed installed - rustup is developer responsibility)
CARGO = cargo

# MSRV (Minimum Supported Rust Version)
MSRV = 1.94.1

# -----------------------------------------------------------------------------
# Default and Help
# -----------------------------------------------------------------------------

all: check

help: ## Show available targets
	@echo "storageprims - Cloud Storage Primitives"
	@echo "Provider-neutral storage contracts with S3 and a Unix/POSIX C ABI."
	@echo ""
	@echo "Development:"
	@echo "  help            Show this help message"
	@echo "  bootstrap       Install tools (sfetch -> goneat)"
	@echo "  build           Build all crates (debug)"
	@echo "  build-release   Build all crates (release)"
	@echo "  build-ffi       Build FFI library with C header"
	@echo "  install         Build and install Unix FFI libraries and header"
	@echo "  uninstall       Remove installed Unix FFI libraries and header"
	@echo "  clean           Remove build artifacts"
	@echo ""
	@echo "Quality gates:"
	@echo "  check           Run all quality checks (fmt, lint, test, deny)"
	@echo "  test            Run test suite"
	@echo "  test-integration-s3  Run S3 LocalStack integration suite (cargo nextest)"
	@echo "  test-integration-ffi Run FFI LocalStack integration suite"
	@echo "  fmt             Format code (cargo fmt + goneat format)"
	@echo "  lint            Run linting (cargo clippy + goneat lint)"
	@echo "  precommit       Pre-commit checks (fast: fmt, clippy)"
	@echo "  prepush         Pre-push checks (thorough: fmt, clippy, test, deny)"
	@echo "  pr-final        Final PR gate (fmt, header, checks, and integration lanes)"
	@echo "  deny            Run cargo-deny license and advisory checks"
	@echo "  audit           Run cargo-audit security scan"
	@echo "  dependency-scan Run Goneat license, cooling, and vulnerability checks"
	@echo "  msrv            Verify build with MSRV (Rust $(MSRV))"
	@echo ""
	@echo "Version management:"
	@echo "  version         Print current version"
	@echo "  version-check   Validate version consistency across files"
	@echo "  version-patch   Bump patch version (0.1.0 -> 0.1.1)"
	@echo "  version-minor   Bump minor version (0.1.0 -> 0.2.0)"
	@echo "  version-major   Bump major version (0.1.0 -> 1.0.0)"
	@echo "  version-set     Set explicit version (V=X.Y.Z)"
	@echo "  version-sync    Sync VERSION to Cargo.toml"
	@echo "  release-check   Validate versions and package the workspace"
	@echo "  release-preflight Verify clean-tree pre-tag requirements"
	@echo "  release         Run the local signed-release ceremony"
	@echo ""
	@echo "Current version: $(VERSION)"

# -----------------------------------------------------------------------------
# Bootstrap - Trust Anchor Chain
# -----------------------------------------------------------------------------
#
# Trust chain: curl -> sfetch -> goneat -> other tools
#
# sfetch (3leaps/sfetch) is the trust anchor - a minimal, auditable binary fetcher.
# goneat (fulmenhq/goneat) is installed via sfetch and manages additional tooling.
#
# NOTE: Rust toolchain (rustup/cargo) is a developer prerequisite, not bootstrapped.

bootstrap: ## Install required tools (sfetch -> goneat)
	@$(MAKE) bootstrap-foundation FORCE=$(FORCE)
	@$(MAKE) bootstrap-rust-tools
	@echo ""
	@echo "[ok] Bootstrap complete"
	@echo ""
	@echo "Ensure $(BIN_DIR) is in your PATH, or tools will be found automatically."

bootstrap-foundation:
	@echo "Bootstrapping storageprims development environment..."
	@echo ""
	@# Step 0: Verify prerequisites
	@if ! command -v curl >/dev/null 2>&1; then \
		echo "[!!] curl not found (required for bootstrap)"; \
		exit 1; \
	fi
	@echo "[ok] curl found"
	@if ! command -v cargo >/dev/null 2>&1; then \
		echo "[!!] cargo not found (required)"; \
		echo ""; \
		echo "Install Rust toolchain (minimum 1.94.1):"; \
		echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"; \
		exit 1; \
	fi
	@RUST_VER=$$(rustc --version 2>/dev/null | sed -n 's/rustc \([0-9]*\.[0-9]*\).*/\1/p'); \
	RUST_MIN="1.94.1"; \
	if [ -z "$$RUST_VER" ] || [ "$$(printf '%s\n%s\n' "$$RUST_MIN" "$$RUST_VER" | sort -V | head -n1)" != "$$RUST_MIN" ]; then \
		echo "[!!] Rust $$RUST_MIN+ required (found: $${RUST_VER:-unknown})"; \
		echo "  rustup install 1.94.1 && rustup default 1.94.1"; \
		exit 1; \
	fi
	@echo "[ok] cargo: $$(cargo --version)"
	@echo ""
	@# Step 1: Install sfetch (trust anchor)
	@mkdir -p "$(BIN_DIR)"
	@if [ ! -x "$(BIN_DIR)/sfetch" ] && ! command -v sfetch >/dev/null 2>&1; then \
		echo "[..] Installing sfetch $(SFETCH_VERSION) (trust anchor)..."; \
		if curl -fsSL "https://github.com/3leaps/sfetch/releases/latest/download/install-sfetch.sh" | bash -s -- --dir "$(BIN_DIR)" --tag $(SFETCH_VERSION) --yes --allow-checksum-only 2>/dev/null && [ -x "$(BIN_DIR)/sfetch" ]; then \
			echo "[ok] sfetch installed via install-sfetch.sh"; \
		else \
			echo "[..] install-sfetch.sh unavailable; using direct tarball..."; \
			SFETCH_ARCH=""; \
			case "$$(uname -s)-$$(uname -m)" in \
				Linux-x86_64|Linux-amd64) SFETCH_ARCH=linux_amd64 ;; \
				Linux-aarch64|Linux-arm64) SFETCH_ARCH=linux_arm64 ;; \
				Darwin-x86_64) SFETCH_ARCH=darwin_amd64 ;; \
				Darwin-arm64) SFETCH_ARCH=darwin_arm64 ;; \
				*) echo "[!!] Unsupported platform for sfetch bootstrap"; exit 1 ;; \
			esac; \
			curl -fsSL "https://github.com/3leaps/sfetch/releases/download/$(SFETCH_VERSION)/sfetch_$${SFETCH_ARCH}.tar.gz" | tar -xz -C "$(BIN_DIR)"; \
		fi; \
	else \
		echo "[ok] sfetch already installed"; \
	fi
	@# Verify sfetch
	@SFETCH_BIN=""; \
	if [ -x "$(BIN_DIR)/sfetch" ]; then SFETCH_BIN="$(BIN_DIR)/sfetch"; \
	elif command -v sfetch >/dev/null 2>&1; then SFETCH_BIN="$$(command -v sfetch)"; fi; \
	if [ -z "$$SFETCH_BIN" ]; then echo "[!!] sfetch installation failed"; exit 1; fi; \
	echo "[ok] sfetch: $$SFETCH_BIN"
	@echo ""
	@# Step 2: Install goneat via sfetch
	@SFETCH_BIN=""; \
	if [ -x "$(BIN_DIR)/sfetch" ]; then SFETCH_BIN="$(BIN_DIR)/sfetch"; \
	elif command -v sfetch >/dev/null 2>&1; then SFETCH_BIN="$$(command -v sfetch)"; fi; \
	if [ "$(FORCE)" = "1" ] || ! command -v goneat >/dev/null 2>&1; then \
		echo "[..] Installing goneat $(GONEAT_VERSION) via sfetch (user-space)..."; \
		$$SFETCH_BIN --repo fulmenhq/goneat --tag $(GONEAT_VERSION) --install; \
	else \
		echo "[ok] goneat already installed"; \
	fi
	@# Verify goneat (user-space only, not repo-local)
	@if command -v goneat >/dev/null 2>&1; then \
		echo "[ok] goneat: $$(goneat version 2>&1 | head -n1)"; \
	else \
		echo "[!!] goneat installation failed"; exit 1; \
	fi
	@echo ""

bootstrap-rust-tools:
	@# Install Rust tools used by gates and FFI packaging.
	@echo "[..] Checking Rust dev tools..."
	@if ! command -v cargo-deny >/dev/null 2>&1; then \
		echo "[..] Installing cargo-deny..."; \
		cargo install cargo-deny --locked; \
	else \
		echo "[ok] cargo-deny installed"; \
	fi
	@if ! command -v cargo-audit >/dev/null 2>&1; then \
		echo "[..] Installing cargo-audit..."; \
		cargo install cargo-audit --locked; \
	else \
		echo "[ok] cargo-audit installed"; \
	fi
	@if ! cargo set-version -V >/dev/null 2>&1; then \
		echo "[..] Installing cargo-edit $(CARGO_EDIT_VERSION)..."; \
		cargo install cargo-edit --locked --version $(CARGO_EDIT_VERSION); \
	else \
		echo "[ok] cargo-edit installed"; \
	fi
	@if ! cargo nextest --version >/dev/null 2>&1; then \
		echo "[..] Installing cargo-nextest $(NEXTEST_VERSION)..."; \
		cargo install cargo-nextest --locked --version $(NEXTEST_VERSION); \
	else \
		echo "[ok] cargo-nextest installed"; \
	fi
	@if ! command -v cbindgen >/dev/null 2>&1; then \
		echo "[..] Installing cbindgen $(CBINDGEN_VERSION)..."; \
		cargo install cbindgen --locked --version $(CBINDGEN_VERSION); \
	else \
		echo "[ok] cbindgen installed"; \
	fi

bootstrap-force: ## Force reinstall all tools
	@$(MAKE) bootstrap FORCE=1

tools: ## Verify external tools are available
	@echo "Verifying tools..."
	@# Check cargo (required)
	@if command -v cargo >/dev/null 2>&1; then \
		echo "[ok] cargo: $$(cargo --version)"; \
	else \
		echo "[!!] cargo not found (required - install rustup)"; \
	fi
	@# Check rustfmt
	@if cargo fmt --version >/dev/null 2>&1; then \
		echo "[ok] rustfmt: $$(cargo fmt --version)"; \
	else \
		echo "[!!] rustfmt not found (rustup component add rustfmt)"; \
	fi
	@# Check clippy
	@if cargo clippy --version >/dev/null 2>&1; then \
		echo "[ok] clippy: $$(cargo clippy --version)"; \
	else \
		echo "[!!] clippy not found (rustup component add clippy)"; \
	fi
	@# Check cargo-deny
	@if command -v cargo-deny >/dev/null 2>&1; then \
		echo "[ok] cargo-deny: $$(cargo-deny --version)"; \
	else \
		echo "[!!] cargo-deny not found (cargo install cargo-deny)"; \
	fi
	@# Check cargo-audit
	@if command -v cargo-audit >/dev/null 2>&1; then \
		echo "[ok] cargo-audit: $$(cargo-audit --version)"; \
	else \
		echo "[!!] cargo-audit not found (cargo install cargo-audit)"; \
	fi
	@# Check cargo-edit
	@if cargo set-version -V >/dev/null 2>&1; then \
		echo "[ok] cargo-edit: $$(cargo set-version -V)"; \
	else \
		echo "[!!] cargo-edit not found (cargo install cargo-edit)"; \
	fi
	@# Check cargo-nextest
	@if cargo nextest --version >/dev/null 2>&1; then \
		echo "[ok] cargo-nextest: $$(cargo nextest --version)"; \
	else \
		echo "[!!] cargo-nextest not found (cargo install cargo-nextest --locked --version $(NEXTEST_VERSION))"; \
	fi
	@# Check cbindgen
	@if command -v cbindgen >/dev/null 2>&1; then \
		echo "[ok] cbindgen: $$(cbindgen --version)"; \
	else \
		echo "[!!] cbindgen not found (run 'make bootstrap')"; \
	fi
	@# Check sfetch
	@if [ -x "$(BIN_DIR)/sfetch" ]; then \
		echo "[ok] sfetch: $(BIN_DIR)/sfetch"; \
	elif command -v sfetch >/dev/null 2>&1; then \
		echo "[ok] sfetch: $$(command -v sfetch)"; \
	else \
		echo "[!!] sfetch not found (run 'make bootstrap')"; \
	fi
	@# Check goneat (user-space)
	@if command -v goneat >/dev/null 2>&1; then \
		echo "[ok] goneat: $$(goneat version 2>&1 | head -n1)"; \
	else \
		echo "[!!] goneat not found (run 'make bootstrap')"; \
	fi
	@echo ""

# -----------------------------------------------------------------------------
# Quality Gates
# -----------------------------------------------------------------------------

check: fmt-check lint test deny ## Run all quality checks
	@echo "[ok] All quality checks passed"

test: ## Run locked test suite
	@echo "Running tests..."
	$(CARGO) test --workspace --locked
	@echo "[ok] Tests passed"

test-integration-s3: ## Run S3 LocalStack integration tests
	@echo "Running S3 integration tests against LocalStack..."
	@if ! cargo nextest --version >/dev/null 2>&1; then \
		echo "[!!] cargo-nextest not found (run 'make bootstrap' or 'cargo install cargo-nextest --locked --version $(NEXTEST_VERSION)')"; \
		exit 1; \
	fi
	AWS_REQUEST_CHECKSUM_CALCULATION=when_required \
	AWS_RESPONSE_CHECKSUM_VALIDATION=when_required \
	$(CARGO) nextest run -p storageprims-s3 --features integration --test integration_s3
	@echo "[ok] S3 integration tests passed"

test-integration-ffi: ## Run FFI LocalStack integration tests
	@echo "Running FFI integration tests against LocalStack..."
	$(CARGO) test -p storageprims-ffi --features integration --test integration_ffi -- --nocapture
	@echo "[ok] FFI integration tests passed"

fmt: ## Format code (cargo fmt + goneat format)
	@echo "Formatting Rust..."
	$(CARGO) fmt --all
	@if command -v goneat >/dev/null 2>&1; then \
		echo "Formatting markdown, YAML, JSON..."; \
		goneat format --quiet; \
	else \
		echo "[!!] goneat not found — skipping non-Rust formatting (run 'make bootstrap')"; \
	fi
	@echo "[ok] Formatting complete"

fmt-check: ## Check formatting without modifying
	@echo "Checking Rust formatting..."
	$(CARGO) fmt --all -- --check
	@if command -v goneat >/dev/null 2>&1; then \
		echo "Checking markdown, YAML, JSON formatting (goneat assess format)..."; \
		if goneat assess --categories format --check --fail-on $(GONEAT_FORMAT_FAIL_ON) --ci-summary --log-level warn --output /dev/null; then \
			true; \
		else \
			echo "[--] goneat assess format failed or unavailable; falling back to goneat format --check"; \
			goneat format --check --quiet; \
		fi; \
	else \
		echo "[!!] goneat not found — skipping non-Rust format check (run 'make bootstrap')"; \
	fi
	@echo "[ok] Formatting check passed"

lint: ## Run linting (cargo clippy + goneat lint)
	@echo "Linting Rust..."
	$(CARGO) clippy --workspace --all-targets --all-features --locked -- -D warnings
	@if command -v goneat >/dev/null 2>&1; then \
		echo "Linting YAML, shell, workflows..."; \
		goneat assess --categories lint --fail-on medium --ci-summary --log-level warn --output /dev/null; \
	else \
		echo "[!!] goneat not found — skipping non-Rust linting (run 'make bootstrap')"; \
	fi
	@echo "[ok] Linting passed"

deny: ## Run cargo-deny license and advisory checks
	@echo "Running cargo-deny..."
	@if command -v cargo-deny >/dev/null 2>&1; then \
		cargo-deny check; \
	else \
		echo "[!!] cargo-deny not found (run 'make bootstrap')"; \
		exit 1; \
	fi
	@echo "[ok] cargo-deny passed"

audit: ## Run cargo-audit security scan
	@echo "Running cargo-audit..."
	@if command -v cargo-audit >/dev/null 2>&1; then \
		cargo-audit audit; \
	else \
		echo "[!!] cargo-audit not found (run 'make bootstrap')"; \
		exit 1; \
	fi
	@echo "[ok] cargo-audit passed"

msrv: ## Verify build with Minimum Supported Rust Version
	@echo "Checking MSRV ($(MSRV))..."
	@if rustup run $(MSRV) cargo --version >/dev/null 2>&1; then \
		rustup run $(MSRV) cargo build --workspace --locked && \
		rustup run $(MSRV) cargo test --workspace --locked; \
	else \
		echo "[!!] Rust $(MSRV) not installed. Install with:"; \
		echo "  rustup install $(MSRV)"; \
		exit 1; \
	fi
	@echo "[ok] MSRV check passed"

precommit: fmt lint ## Pre-commit checks (fast: fmt, clippy)
	@echo "[ok] Pre-commit checks passed"

dependency-scan: ## Run Goneat dependency policy and vulnerability checks
	@./scripts/check-dependencies.sh

prepush: fmt-check lint test deny dependency-scan version-check release-tooling-test ## Pre-push checks (thorough)
	@echo "[ok] Pre-push checks passed"

pr-final: fmt cbindgen prepush test-integration-s3 test-integration-ffi ## Final PR gate before push/PR update
	@echo "[ok] PR final checks passed"

# -----------------------------------------------------------------------------
# Build
# -----------------------------------------------------------------------------

build: ## Build all crates (debug)
	@echo "Building (debug)..."
	$(CARGO) build --workspace
	@echo "[ok] Build complete"

build-release: ## Build all crates (release)
	@echo "Building (release)..."
	$(CARGO) build --workspace --release
	@echo "[ok] Release build complete"

build-ffi: cbindgen ## Build FFI library with C header
	@echo "Building FFI library..."
	$(CARGO) build --package storageprims-ffi --release
	@echo "[ok] FFI build complete"
	@echo "Library: target/release/libstorageprims_ffi.*"
	@echo "Header: ffi/storageprims-ffi/storageprims.h"

cbindgen: ## Generate C header from FFI crate
	@echo "Generating C header..."
	@if command -v cbindgen >/dev/null 2>&1; then \
		cbindgen --config cbindgen.toml --crate storageprims-ffi --output ffi/storageprims-ffi/storageprims.h; \
		echo "[ok] Generated ffi/storageprims-ffi/storageprims.h"; \
	else \
		echo "[!!] cbindgen not found (cargo install cbindgen)"; \
		exit 1; \
	fi

clean: ## Remove build artifacts
	@echo "Cleaning..."
	$(CARGO) clean
	@rm -rf bin/
	@echo "[ok] Clean complete"

# -----------------------------------------------------------------------------
# Local Unix FFI install
# -----------------------------------------------------------------------------

install: build-ffi ## Install Unix FFI libraries and header to user space
	@./scripts/install-ffi.sh install "$(CURDIR)/target/release" \
		"$(CURDIR)/ffi/storageprims-ffi/storageprims.h" \
		"$(INSTALL_LIBDIR)" "$(INSTALL_INCLUDEDIR)"

uninstall: ## Remove installed Unix FFI libraries and header
	@./scripts/install-ffi.sh uninstall "$(CURDIR)/target/release" \
		"$(CURDIR)/ffi/storageprims-ffi/storageprims.h" \
		"$(INSTALL_LIBDIR)" "$(INSTALL_INCLUDEDIR)"

install-test: ## Test unlink-first install and bounded uninstall in a temp prefix
	@./scripts/release-safety.test.sh

# -----------------------------------------------------------------------------
# Version Management
# -----------------------------------------------------------------------------

version: ## Print current version
	@echo "$(VERSION)"

version-patch: ## Bump patch version
	@current=$$(tr -d ' \t\r\n' < $(VERSION_FILE)); \
	major=$$(echo "$$current" | cut -d. -f1); \
	minor=$$(echo "$$current" | cut -d. -f2); \
	patch=$$(echo "$$current" | cut -d. -f3); \
	new_version="$$major.$$minor.$$((patch + 1))"; \
	echo "$$new_version" > $(VERSION_FILE); \
	$(MAKE) version-sync --silent; \
	echo "Version bumped: $$current -> $$new_version"

version-minor: ## Bump minor version
	@current=$$(tr -d ' \t\r\n' < $(VERSION_FILE)); \
	major=$$(echo "$$current" | cut -d. -f1); \
	minor=$$(echo "$$current" | cut -d. -f2); \
	new_version="$$major.$$((minor + 1)).0"; \
	echo "$$new_version" > $(VERSION_FILE); \
	$(MAKE) version-sync --silent; \
	echo "Version bumped: $$current -> $$new_version"

version-major: ## Bump major version
	@current=$$(tr -d ' \t\r\n' < $(VERSION_FILE)); \
	major=$$(echo "$$current" | cut -d. -f1); \
	new_version="$$((major + 1)).0.0"; \
	echo "$$new_version" > $(VERSION_FILE); \
	$(MAKE) version-sync --silent; \
	echo "Version bumped: $$current -> $$new_version"

version-set: ## Set explicit version (V=X.Y.Z)
	@if [ -z "$(V)" ]; then echo "[!!] Usage: make version-set V=X.Y.Z"; exit 1; fi
	@echo "$(V)" > $(VERSION_FILE)
	@$(MAKE) version-sync --silent
	@echo "[ok] Set version to $(V)"

version-sync: ## Sync VERSION file to Cargo.toml
	@ver=$$(tr -d ' \t\r\n' < $(VERSION_FILE)); \
	if cargo set-version -V >/dev/null 2>&1 && cargo set-version --workspace "$$ver"; then \
		echo "[ok] Synced Cargo.toml to $$ver"; \
	else \
		python3 -c "\
import pathlib, re, sys; \
ver = sys.argv[1]; \
p = pathlib.Path('Cargo.toml'); \
text = p.read_text(); \
text, n = re.subn(r'(?m)^version = \"[^\"]*\"', 'version = \"%s\"' % ver, text, count=1); \
assert n == 1, 'failed to update [workspace.package] version'; \
text = re.sub(r'(storageprims-(?:core|ops|s3|ffi) = \{ version = )\"[^\"]*\"', r'\1\"%s\"' % ver, text); \
p.write_text(text); \
" "$$ver" || exit 1; \
		echo "[ok] Synced Cargo.toml to $$ver (python fallback)"; \
	fi

version-check: ## Validate version consistency across files
	@echo "Checking version consistency..."
	@./scripts/check-version.sh

# -----------------------------------------------------------------------------
# Release
# -----------------------------------------------------------------------------

release-check: version-check ## Version consistency + package check (does not publish)
	@echo "Packaging workspace crates (does not cargo publish)..."
	@./scripts/check-packages.sh
	@echo "[ok] Package check passed; cargo publish was not run"

release-crates-list: ## Print the validated registry publication order
	@./scripts/release-crates.py list

release-crates-dry-run: ## Dry-run crates.io publishing from the guarded tag
	@./scripts/release-crates-dry-run.sh

release-crates-verify: ## Wait for the final registry version and verify each published crate
	@CRATE="$(CRATE)" ./scripts/release-crates-verify.sh

release-tooling-test: ## Run release guard, asset, cleanup, and hygiene tests
	@./scripts/release-decernor.test.sh
	@./scripts/release-tag-controls.test.sh
	@./scripts/verify-pinned-tag.test.sh
	@./scripts/release-crates.test.sh
	@./scripts/release-crates-verify.test.sh
	@./scripts/release-guard-tag-version.test.sh
	@./scripts/release-assets.test.sh
	@./scripts/release-github-state.test.sh
	@./scripts/release-safety.test.sh
	@./scripts/release-negative-controls.test.sh
	@echo "[ok] Release tooling tests passed"

release-preflight: ## Verify clean-tree pre-tag requirements
	@./scripts/validate-release-anchors.sh
	@echo "Running release preflight checks..."
	@if [ -n "$$(git status --porcelain 2>/dev/null)" ]; then \
		echo "[!!] Working tree not clean - commit or stash changes first"; \
		git status --short; \
		exit 1; \
	fi
	@$(MAKE) pr-final --silent
	@$(MAKE) version-check --silent
	@grep -Eq "^## v$(VERSION) — [0-9]{4}-[0-9]{2}-[0-9]{2}$$" RELEASE_NOTES.md || \
		{ echo "[!!] RELEASE_NOTES.md lacks the exact v$(VERSION) heading"; exit 1; }
	@test -f "docs/releases/v$(VERSION).md" || \
		{ echo "[!!] Per-cut release notes are missing"; exit 1; }
	@./scripts/check-release-notes.sh "v$(VERSION)"
	@git fetch origin main
	@test "$$(git rev-parse HEAD)" = "$$(git rev-parse origin/main)" || \
		{ echo "[!!] HEAD must equal fetched origin/main"; exit 1; }
	@test "$$(git rev-list --count HEAD..origin/main)" = 0
	@test "$$(git rev-list --count origin/main..HEAD)" = 0
	@test -z "$$(git status --porcelain)" || \
		{ echo "[!!] Preflight gates changed the working tree"; exit 1; }
	@echo "[ok] All preflight checks passed - ready to tag v$(VERSION)"

release-guard-tag-version: ## Validate the canonical release tag
	@./scripts/release-guard-tag-version.sh

release-tag: ## Create and verify a local signed version tag
	@./scripts/release-tag.sh

release-push-tag: ## Publish and verify the signed version tag
	@./scripts/release-push-tag.sh

release-verify-tag: ## Verify the tag using only the committed public pin
	@./scripts/release-verify-tag.sh

release-verify-remote-tag: ## Compare local and remote tag objects and GitHub verification
	@./scripts/release-verify-remote-tag.sh

release-insert-anchors: ## Maintainer-only: generate and review public fingerprint anchors
	@./scripts/release-insert-anchors.sh

release-clean: ## Safely empty the repository release staging directory
	@./scripts/release-clean.sh "$(RELEASE_DIR)"

release-download: ## Download exact unsigned assets from the trusted draft
	@./scripts/download-release-assets.sh "$(RELEASE_DIR)"

release-notes: ## Add the exact per-cut notes to the signable asset set
	@STORAGEPRIMS_REQUIRE_TAG=1 ./scripts/release-guard-tag-version.sh >/dev/null
	@test -f "docs/releases/$${STORAGEPRIMS_RELEASE_TAG}.md" || \
		{ echo "[!!] Exact per-cut release notes are missing"; exit 1; }
	@./scripts/validate-release-assets.sh "$(RELEASE_DIR)" base >/dev/null
	@cp "docs/releases/$${STORAGEPRIMS_RELEASE_TAG}.md" \
		"$(RELEASE_DIR)/release-notes-$${STORAGEPRIMS_RELEASE_TAG}.md"
	@./scripts/stage-release-anchors.sh "$(RELEASE_DIR)"
	@./scripts/validate-release-assets.sh "$(RELEASE_DIR)" signable >/dev/null
	@echo "[ok] Per-cut release notes added to the signed set"

release-checksums: ## Generate exact SHA256 and SHA512 manifests
	@./scripts/generate-checksums.sh "$(RELEASE_DIR)"

release-sign: ## Sign checksum manifests with local MFA-held keys
	@./scripts/sign-release-assets.sh "$(RELEASE_DIR)"

release-export-keys: ## Export and prove public verification material
	@./scripts/export-release-keys.sh "$(RELEASE_DIR)"

release-verify-checksums: ## Verify exact dual checksum manifests
	@./scripts/verify-checksums.sh "$(RELEASE_DIR)"

release-verify-signatures: ## Verify every configured signature
	@./scripts/verify-signatures.sh "$(RELEASE_DIR)"

release-verify-keys: ## Verify exported public keys against staged anchors
	@./scripts/verify-public-keys.sh "$(RELEASE_DIR)"

release-verify: release-verify-checksums release-verify-signatures release-verify-keys ## Verify signed release set
	@./scripts/validate-release-assets.sh "$(RELEASE_DIR)" signed >/dev/null
	@echo "[ok] Signed release set verified"

release-upload: ## Verify once and update the exact trusted draft release
	@./scripts/upload-release-assets.sh "$(RELEASE_DIR)"

release: release-guard-tag-version ## Run the serialized local signing ceremony
	@STORAGEPRIMS_REQUIRE_TAG=1 ./scripts/release-guard-tag-version.sh >/dev/null
	@$(MAKE) release-clean
	@$(MAKE) release-download
	@$(MAKE) release-notes
	@$(MAKE) release-checksums
	@$(MAKE) release-sign
	@$(MAKE) release-export-keys
	@$(MAKE) release-upload
	@echo "[ok] Release assets signed and uploaded; GitHub release remains draft"
