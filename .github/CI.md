# CI

The required pull-request gates are `fast`, `platforms`, `integration-s3`,
and `publish-check`. The native matrix covers all five supported
desktop/server platforms on every push and pull request, including forks.
The stable `platforms` job requires all five matrix cells to pass.

## Jobs

| Job              | When          | Role                                                                                            |
| ---------------- | ------------- | ----------------------------------------------------------------------------------------------- |
| `fast`           | every push/PR | digest-pinned Linux quality runner: format, lint, tests, version consistency, dependency policy |
| `platform-smoke` | every push/PR | Linux x64/arm64, macOS arm64, and Windows x64/arm64 native clippy, tests, and release builds    |
| `platforms`      | every push/PR | stable required aggregate that fails unless all five native matrix cells pass                   |
| `integration-s3` | every push/PR | required S3 behavior against LocalStack                                                         |
| `publish-check`  | every push/PR | required packaging and verification of every workspace crate without publishing                 |

All jobs use the workspace MSRV, Rust 1.89.0. The Linux quality job uses the
digest-pinned Fulmen Toolbox goneat glibc runner with writable GitHub homes;
the image's newer default Rust is not used as MSRV evidence. Windows commands
use Bash. There are no cross-compiled, musl, or macOS Intel cells.

`.github/actionlint.yaml` declares the organization GitHub-hosted arm64 runner
labels for linting; it does not deploy runners.
