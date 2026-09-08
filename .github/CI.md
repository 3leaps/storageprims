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

The publish check creates and verifies the `storageprims-core`, `storageprims-ops`,
and `storageprims-s3` archives in an isolated target directory. For the initial
staged release, a command-line-only Cargo registry patch supplies the unpublished
core crate while verifying the dependent packages. The generated manifests are
checked to retain the registry version and omit the local dependency path.

All jobs use the workspace MSRV, Rust 1.94.1. The Linux quality job uses the
digest-pinned Fulmen Toolbox goneat glibc runner with writable GitHub homes;
the job asserts Rust 1.94.1 and Goneat v0.6.0 before running checks. Windows
commands use Bash. There are no cross-compiled, musl, or macOS Intel cells.

`.github/actionlint.yaml` declares the organization GitHub-hosted arm64 runner
labels for linting; it does not deploy runners.
