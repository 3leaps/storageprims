#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Refuse Cargo configuration that substitutes sources or runs commands.

Registry operations run Cargo inside a checkout of the release tag, with the
maintainer's registry credentials present. Configuration that redirects the
registry, patches a source, or names a program for Cargo to run (a compiler
wrapper, linker, runner, credential provider or alias) would let that checkout
or the operator environment substitute code or bytes, so it is refused:

- in the environment: source-selection and command-selection variables;
- in Cargo configuration files (CARGO_HOME and every ancestor .cargo/): any
  source override and any command-naming key;
- in the tagged source tree (--source-root): any .cargo/config file at all,
  and any published crate with a build script.
"""

import os
from pathlib import Path
import sys
import tomllib

# Top-level configuration tables that cannot change sources or run commands.
# Everything else is refused, including [env] (it sets variables such as PATH
# for rustc and build scripts), [target], [host], [alias] and [unstable].
SAFE_TABLES = {
    "net",
    "http",
    "term",
    "cargo-new",
    "profile",
    "future-incompat-report",
    "cache",
}
# Keys under tables that are otherwise safe but name a program.
COMMAND_KEYS = {
    ("build", "rustc"),
    ("build", "rustc-wrapper"),
    ("build", "rustc-workspace-wrapper"),
    ("build", "rustdoc"),
    ("build", "target-dir"),
    ("registry", "credential-provider"),
    ("registry", "global-credential-providers"),
}
SAFE_BUILD_KEYS = {"jobs", "incremental", "pipelining"}
SAFE_REGISTRY_KEYS = {"default", "token"}
SOURCE_TABLES = {"patch", "replace", "source", "include", "paths"}
COMMAND_ENV = {
    "RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_ENCODED_RUSTDOCFLAGS",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOC",
    "CARGO",
    "CARGO_TARGET_DIR",
    "CARGO_REGISTRY_CREDENTIAL_PROVIDER",
    "CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS",
    "CARGO_REGISTRY_INDEX",
    "CARGO_REGISTRY_DEFAULT",
}
COMMAND_ENV_PREFIXES = (
    "CARGO_BUILD_",
    "CARGO_TARGET_",
    "CARGO_ALIAS_",
    "CARGO_SOURCE_",
    "CARGO_HOST_",
    "CARGO_ENV_",
)


def refused_env(name):
    if name in COMMAND_ENV or name.startswith(COMMAND_ENV_PREFIXES):
        return True
    if name.startswith("CARGO_REGISTRIES_"):
        # Only a token for a named registry is accepted; never its index or
        # credential provider.
        return not name.endswith("_TOKEN")
    return False


def validate(config, environment):
    for name in environment:
        if refused_env(name):
            raise ValueError(
                "Cargo source or command environment overrides are forbidden"
            )
    for table, value in config.items():
        if table in SOURCE_TABLES:
            raise ValueError(
                "Cargo patch/source overrides are forbidden for registry operations"
            )
        if table == "build":
            if any(
                (table, key) in COMMAND_KEYS or key not in SAFE_BUILD_KEYS
                for key in value
            ):
                raise ValueError(
                    "Cargo build configuration may not name a program or path"
                )
        elif table == "registry":
            if any(
                (table, key) in COMMAND_KEYS or key not in SAFE_REGISTRY_KEYS
                for key in value
            ):
                raise ValueError(
                    "Cargo registry configuration may not name an index or command"
                )
        elif table == "registries":
            for registry in value.values():
                if any(key != "token" for key in registry):
                    raise ValueError(
                        "Cargo registry index or command overrides are forbidden"
                    )
        elif table not in SAFE_TABLES:
            raise ValueError(
                f"Cargo configuration table [{table}] is not allowed for registry operations"
            )


def config_files(directory):
    for name in ("config", "config.toml"):
        path = directory / name
        if path.exists() or path.is_symlink():
            if not path.is_file() or path.is_symlink():
                raise ValueError("Cargo configuration must be a regular readable file")
            yield path


def validate_source(root):
    """The tagged tree carries no Cargo configuration and no build script in crates/."""
    for dirpath, dirnames, _ in os.walk(root):
        dirnames[:] = [
            d for d in dirnames if d not in (".git", "target", "node_modules")
        ]
        directory = Path(dirpath)
        if directory.name.casefold() == ".cargo" and any(config_files(directory)):
            raise ValueError("the release source may not carry Cargo configuration")
    for manifest in root.glob("crates/*/Cargo.toml"):
        package = tomllib.loads(manifest.read_text()).get("package", {})
        if (
            package.get("build") not in (None, False)
            or (manifest.parent / "build.rs").exists()
        ):
            raise ValueError("a release crate may not have a build script")


def main():
    root = Path(__file__).resolve().parent.parent
    source_root = None
    if sys.argv[1:]:
        if len(sys.argv) != 3 or sys.argv[1] != "--source-root":
            raise ValueError("expected optional verified source root")
        source_root = Path(sys.argv[2]).resolve()
    home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    if not home.is_absolute():
        raise ValueError("Cargo home must be absolute for registry operations")
    home = home.resolve()
    tree = source_root or root
    if home == tree or tree in home.parents:
        raise ValueError("Cargo home must be outside the release source")
    validate({}, os.environ)
    manifest = tomllib.loads((tree / "Cargo.toml").read_text())
    if any(name in manifest for name in ("patch", "replace")):
        raise ValueError(
            "Cargo patch/source overrides are forbidden for registry operations"
        )
    for path in config_files(home):
        validate(tomllib.loads(path.read_text()), os.environ)
    if source_root is not None:
        validate_source(source_root)
    for directory in (path / ".cargo" for path in [tree, *tree.parents]):
        if directory == home:
            continue
        if (
            source_root is not None
            and directory != tree / ".cargo"
            and any(config_files(directory))
        ):
            # Nothing outside CARGO_HOME configures a registry step: a config
            # in any ancestor of the release source (a shared temporary
            # directory, say) could proxy or redirect the upload.
            raise ValueError(
                "Cargo configuration in an ancestor of the release source is forbidden"
            )
        for path in config_files(directory):
            validate(tomllib.loads(path.read_text()), os.environ)
    print("[ok] Cargo configuration has no source substitutions or commands")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError) as error:
        # Do not expose local configuration paths, contents or credential values.
        detail = (
            str(error)
            if isinstance(error, ValueError)
            and not isinstance(error, tomllib.TOMLDecodeError)
            else "invalid or unreadable Cargo configuration"
        )
        sys.exit("error: " + detail)
