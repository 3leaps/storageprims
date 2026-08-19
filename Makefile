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

.PHONY: all help bootstrap bootstrap-force tools check test test-integration-s3 test-integration-ffi fmt fmt-check lint build clean version
.PHONY: precommit prepush deny audit msrv
.PHONY: build-release build-ffi cbindgen pr-final
.PHONY: build-local-go build-local-ffi-shared go-test header-go
.PHONY: version-patch version-minor version-major version-set

# -----------------------------------------------------------------------------
# Configuration
# -----------------------------------------------------------------------------

# Version from Cargo.toml (SSOT) - extracted via cargo metadata
VERSION := $(shell cargo metadata --no-deps --format-version 1 2>/dev/null | \
	grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4 || echo "dev")

# Tool installation directory
# Bootstrap installs sfetch to repo-local bin/
BIN_DIR := $(CURDIR)/bin

# Pinned tool versions for reproducibility
SFETCH_VERSION := v0.4.11
GONEAT_VERSION ?= v0.5.16
GONEAT_FORMAT_FAIL_ON ?= medium
NEXTEST_VERSION ?= 0.9.128
CARGO_EDIT_VERSION ?= 0.13.10

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
	@echo "Uniform cloud storage access across S3, GCS, Azure, and local filesystem."
	@echo ""
	@echo "Development:"
	@echo "  help            Show this help message"
	@echo "  bootstrap       Install tools (sfetch -> goneat)"
	@echo "  build           Build all crates (debug)"
	@echo "  build-release   Build all crates (release)"
	@echo "  build-ffi       Build FFI library with C header"
	@echo "  clean           Remove build artifacts"
	@echo ""
	@echo "Go bindings:"
	@echo "  build-local-go      Build FFI for local Go development"
	@echo "  build-local-ffi-shared  Build shared FFI for local consumers"
	@echo "  go-test             Run Go binding tests"
	@echo "  header-go           Generate C header for Go bindings"
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
	@echo "  msrv            Verify build with MSRV (Rust $(MSRV))"
	@echo ""
	@echo "Version management:"
	@echo "  version         Print current version"
	@echo "  version-patch   Bump patch version (0.1.0 -> 0.1.1)"
	@echo "  version-minor   Bump minor version (0.1.0 -> 0.2.0)"
	@echo "  version-major   Bump major version (0.1.0 -> 1.0.0)"
	@echo "  version-set     Set explicit version (V=X.Y.Z)"
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
	@# Step 3: Install Rust tools via cargo (cargo-deny, cargo-audit, cargo-edit, cargo-nextest)
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
	@echo ""
	@echo "[ok] Bootstrap complete"
	@echo ""
	@echo "Ensure $(BIN_DIR) is in your PATH, or tools will be found automatically."

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

test: ## Run test suite
	@echo "Running tests..."
	$(CARGO) test --workspace
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
		rustup run $(MSRV) cargo build --workspace && \
		rustup run $(MSRV) cargo test --workspace; \
	else \
		echo "[!!] Rust $(MSRV) not installed. Install with:"; \
		echo "  rustup install $(MSRV)"; \
		exit 1; \
	fi
	@echo "[ok] MSRV check passed"

precommit: fmt lint ## Pre-commit checks (fast: fmt, clippy)
	@echo "[ok] Pre-commit checks passed"

prepush: fmt-check lint test deny ## Pre-push checks (thorough)
	@echo "[ok] Pre-push checks passed"

pr-final: fmt cbindgen fmt-check lint test deny test-integration-s3 test-integration-ffi ## Final PR gate before push/PR update
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
	@echo "Library: target/release/libstorageprims.*"
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
# Go Bindings (placeholder — activate when ffi/bindings land)
# -----------------------------------------------------------------------------

GO_BINDINGS_DIR := bindings/go/storageprims
GO_LIB_ROOT := $(GO_BINDINGS_DIR)/lib

# Detect current platform
UNAME_S := $(shell uname -s | tr '[:upper:]' '[:lower:]')
UNAME_M := $(shell uname -m)

# Normalize architecture names for Go
ifeq ($(UNAME_M),x86_64)
    GO_ARCH := amd64
endif
ifeq ($(UNAME_M),aarch64)
    GO_ARCH := arm64
endif
ifeq ($(UNAME_M),arm64)
    GO_ARCH := arm64
endif

# Normalize OS names
ifeq ($(UNAME_S),darwin)
    GO_OS := darwin
    GO_LIB_EXT := .a
    GO_SHARED_EXT := .dylib
    GO_LIB_PREFIX := lib
endif
ifeq ($(UNAME_S),linux)
    GO_OS := linux
    GO_LIB_EXT := .a
    GO_SHARED_EXT := .so
    GO_LIB_PREFIX := lib
endif

build-local-go: ## Build FFI for local Go development
	@echo "Building FFI static library for local Go development..."
	$(CARGO) build --package storageprims-ffi --release
	@mkdir -p $(GO_LIB_ROOT)/local/$(GO_OS)-$(GO_ARCH)
	@cp target/release/$(GO_LIB_PREFIX)storageprims$(GO_LIB_EXT) \
		$(GO_LIB_ROOT)/local/$(GO_OS)-$(GO_ARCH)/
	@echo "[ok] Static library copied to $(GO_LIB_ROOT)/local/$(GO_OS)-$(GO_ARCH)/"

build-local-ffi-shared: ## Build shared FFI library for local consumers
	@echo "Building FFI shared library..."
	$(CARGO) build --package storageprims-ffi --release
	@echo "[ok] Shared library: target/release/$(GO_LIB_PREFIX)storageprims$(GO_SHARED_EXT)"

go-test: ## Run Go binding tests
	@echo "Running Go binding tests..."
	@cd $(GO_BINDINGS_DIR) && go test ./... -v
	@echo "[ok] Go tests passed"

header-go: cbindgen ## Generate C header for Go bindings
	@mkdir -p $(GO_BINDINGS_DIR)/include
	@cp ffi/storageprims-ffi/storageprims.h $(GO_BINDINGS_DIR)/include/
	@echo "[ok] Header copied to $(GO_BINDINGS_DIR)/include/storageprims.h"

# -----------------------------------------------------------------------------
# Version Management
# -----------------------------------------------------------------------------

version: ## Print current version
	@echo "$(VERSION)"

version-patch: ## Bump patch version
	@cargo set-version --workspace --bump patch
	@echo "[ok] Bumped to $$(cargo metadata --no-deps --format-version 1 | grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4)"

version-minor: ## Bump minor version
	@cargo set-version --workspace --bump minor
	@echo "[ok] Bumped to $$(cargo metadata --no-deps --format-version 1 | grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4)"

version-major: ## Bump major version
	@cargo set-version --workspace --bump major
	@echo "[ok] Bumped to $$(cargo metadata --no-deps --format-version 1 | grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4)"

version-set: ## Set explicit version (V=X.Y.Z)
	@if [ -z "$(V)" ]; then echo "[!!] Usage: make version-set V=X.Y.Z"; exit 1; fi
	@cargo set-version --workspace $(V)
	@echo "[ok] Set version to $(V)"
