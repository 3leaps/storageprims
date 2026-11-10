#!/usr/bin/env bash
# release-workflow-policy.py accepts the repository workflows and a minimal
# compliant workflow, and refuses each violation however it is spelled
# (quoted scalars, flow style, block scalars, continuations, omitted keys).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
policy="$root/scripts/release-workflow-policy.py"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
python3 -I "$policy" "$root"/.github/workflows/*.yml

checkout='actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1'
image='ghcr.io/fulmenhq/example@sha256:0000000000000000000000000000000000000000000000000000000000000000'
# A compliant base; each case replaces the marked job body.
base() {
	cat <<YAML
name: fixture
on: {push: {tags: ['v*']}}
permissions:
  contents: read
jobs:
  build:
    runs-on: ubuntu-24.04
$1
YAML
}
good_steps="    steps:
      - uses: $checkout
        with:
          persist-credentials: false
      - run: echo ok"
base "$good_steps" >"$scratch/good.yml"
python3 -I "$policy" "$scratch/good.yml"

count=0
refused() {
	local reason="$1" body="$2" file
	count=$((count + 1))
	file="$scratch/case-$count.yml"
	printf '%s\n' "$body" >"$file"
	if python3 -I "$policy" "$file" >"$scratch/out" 2>&1; then
		echo "error: policy accepted case $count, expected: $reason" >&2
		cat "$file" >&2
		exit 1
	fi
	grep -q -- "$reason" "$scratch/out" || {
		echo "error: case $count refused for another reason than '$reason'" >&2
		cat "$scratch/out" >&2
		exit 1
	}
}

# Permissions, in every spelling.
refused 'top-level permissions must be exactly' "$(base "$good_steps" | sed 's/^  contents: read$/  contents: write/')"
refused 'top-level permissions must be exactly' "$(base "$good_steps" | sed 's/^  contents: read$/  contents: "write"/')"
refused 'top-level permissions must be exactly' "$(base "$good_steps" | sed -e '/^permissions:$/d' -e '/^  contents: read$/d')"
refused 'top-level permissions must be exactly' "$(base "$good_steps" | sed -e 's/^permissions:$/permissions: write-all/' -e '/^  contents: read$/d')"
refused 'top-level permissions must be exactly' "$(base "$good_steps" | sed -e 's/^permissions:$/permissions: {contents: read, actions: write}/' -e '/^  contents: read$/d')"
refused 'grants more than read' "$(base "    permissions: {contents: write}
$good_steps")"
refused 'grants more than read' "$(base "    permissions:
      id-token: 'write'
$good_steps")"
refused 'grants more than read' "$(base "    permissions: {actions: Write, contents: read}
$good_steps")"
refused 'must be a mapping of read or none' "$(base "    permissions: write-all
$good_steps")"
# Checkout credentials: omitted (the action's default persists), true, quoted.
refused 'persist-credentials: false' "$(base "    steps:
      - uses: $checkout")"
refused 'persist-credentials: false' "$(base "    steps:
      - uses: $checkout
        with: {fetch-depth: 0}")"
refused 'persist-credentials: false' "$(base "    steps:
      - uses: $checkout
        with:
          persist-credentials: true")"
refused 'persist-credentials: false' "$(base "    steps:
      - uses: $checkout
        with:
          persist-credentials: 'false'")"
refused 'persist-credentials: false' "$(base "    steps:
      - {uses: '$checkout', with: {persist-credentials: \"true\"}}")"
# Action pins.
refused 'not pinned to a full commit SHA' "$(base "    steps:
      - {uses: actions/checkout@main, with: {persist-credentials: false}}")"
refused 'not pinned to a full commit SHA' "$(base "    steps:
      - uses: dtolnay/rust-toolchain@stable")"
refused 'not pinned to a full commit SHA' "$(base "    steps:
      - uses: \"docker://alpine:latest\"")"
refused 'refused Node 20 action SHA' "$(base "    steps:
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02")"
refused 'creates GitHub releases' "$(base "    steps:
      - uses: softprops/action-gh-release@0000000000000000000000000000000000000000")"
refused 'not pinned to a full commit SHA' "$(base "    uses: org/repo/.github/workflows/x.yml@v1")"
# Images.
refused 'is not digest-pinned' "$(base "    container: alpine:latest
$good_steps")"
refused 'is not digest-pinned' "$(base "    container: {image: 'ghcr.io/fulmenhq/goneat-tools-runner-glibc:v0.5.6'}
$good_steps")"
refused 'is not digest-pinned' "$(base "    services:
      db: {image: postgres:16}
$good_steps")"
refused 'docker run image is not digest-pinned' "$(base "    steps:
      - run: |
          docker run --rm \\
            anchore/syft:latest dir:.")"
# Release creation and publication from a run script.
refused 'forbidden release or publish command' "$(base "    steps:
      - run: gh release create v1.2.3 --draft")"
refused 'forbidden release or publish command' "$(base "    steps:
      - run: >-
          /usr/bin/gh
          release upload v1.2.3 x")"
refused 'forbidden release or publish command' "$(base "    steps:
      - run: gh api --method POST repos/o/r/releases -f tag_name=v1")"
refused 'forbidden release or publish command' "$(base "    steps:
      - run: |
          cargo \\
            publish -p crate")"
# A comment line or a release download URL is not a release command.
base "    steps:
      - run: |
          # gh release create is done by the maintainer
          curl -fsSL https://github.com/3leaps/decernor/releases/download/v1/x.tar.gz -o x
      - uses: $checkout
        with:
          persist-credentials: false
    container: {image: '$image'}" >"$scratch/benign.yml"
python3 -I "$policy" "$scratch/benign.yml"
echo "[ok] workflow policy: repository workflows pass; $count violations refused across quoted, flow, block-scalar, continuation and omitted forms"
