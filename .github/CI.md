# CI

The required pull-request gates are `fast`, `platforms`, `integration-s3`,
and `publish-check`. The native matrix covers portable crates on all five
desktop/server platforms on every push and pull request, including forks.
Unix cells additionally validate the POSIX file-descriptor FFI member. The
stable `platforms` job requires all five matrix cells to pass.

## Jobs

| Job              | When          | Role                                                                                            |
| ---------------- | ------------- | ----------------------------------------------------------------------------------------------- |
| `fast`           | every push/PR | digest-pinned Linux quality runner: format, lint, tests, version consistency, dependency policy |
| `platform-smoke` | every push/PR | Five-host native clippy, tests, and release builds; Windows excludes the POSIX FFI member       |
| `platforms`      | every push/PR | stable required aggregate that fails unless all five native matrix cells pass                   |
| `integration-s3` | every push/PR | required S3 behavior against LocalStack                                                         |
| `publish-check`  | every push/PR | required packaging and verification of every publishable workspace crate without publishing     |

The publish check creates and verifies an archive for every crate in
`config/release/publishable-crates.txt` (`storageprims-core`,
`storageprims-s3`, `storageprims-local`, `storageprims-ops`) in an isolated
target directory. Because the new version is not on crates.io yet, a
command-line-only Cargo registry patch supplies the earlier crates in the list
while verifying the ones that depend on them. The generated manifests are
checked to retain the registry version and omit the local dependency path.

All jobs use the workspace MSRV, Rust 1.94.1. The Linux quality job uses the
digest-pinned Fulmen Toolbox goneat glibc runner with writable GitHub homes;
the job asserts Rust 1.94.1 and Goneat v0.6.1 before running checks. Windows
commands use Bash. There are no cross-compiled, musl, or macOS Intel cells.
Local `make bootstrap` installs Goneat v0.6.1 when absent; an existing
executable on PATH is not replaced by default. Run
`make bootstrap-foundation FORCE=1` to upgrade an existing installation.

`.github/actionlint.yaml` declares the organization GitHub-hosted arm64 runner
labels for linting; it does not deploy runners.

## Release workflow

An annotated stable-version tag starts the separate `Release` workflow. It
restores the tag with the read-only job token, verifies it against the public
key committed in the tagged commit, packages the Unix FFI library and header on
Linux x86_64, Linux arm64, and macOS arm64, and generates a CycloneDX SBOM with
a digest-pinned Syft image. It uploads these and a release identity record as
workflow artifacts; it does not create or change a GitHub release.

Every job has `contents: read` only. Every checkout disables persisted
credentials, every action is commit-pinned, and CI has no signing-key inputs.
Binding the tag to `origin/main`, staging the workflow artifacts, creating the
draft, checksum generation, signing, verification, and the explicit asset
upload run on the maintainer workstation.
