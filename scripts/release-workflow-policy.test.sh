#!/usr/bin/env bash
# release-workflow-policy.py accepts the repository workflows and a minimal
# compliant workflow, and refuses each violation however it is spelled
# (quoted scalars, flow style, block scalars, continuations, omitted keys).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
policy="$root/scripts/release-workflow-policy.py"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
# Every workflow file GitHub runs, both extensions.
workflows=()
while IFS= read -r -d '' file; do workflows+=("$file"); done < <(find "$root/.github/workflows" -type f \( -name '*.yml' -o -name '*.yaml' \) -print0)
[[ "${#workflows[@]}" -gt 0 ]]
python3 -I "$policy" "${workflows[@]}"

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
# Round-2 bypass forms: action case, local and docker actions, constructed
# commands, secrets in any spelling, custom shells, digest outside the image.
refused 'persist-credentials: false' "$(base "    steps:
      - uses: Actions/Checkout@3d3c42e5aac5ba805825da76410c181273ba90b1")"
refused 'not pinned to a full commit SHA' "$(base "    steps:
      - uses: ./.github/actions/probe")"
refused 'forbidden release or publish command' "$(base "    env: {CMD: gh}
    steps:
      - run: \$CMD release create v1.2.3 --draft")"
refused 'forbidden release or publish command' "$(base "    steps:
      - run: gh api -X POST /repos/o/r/releases")"
refused 'forbidden release or publish command' "$(base "    steps:
      - run: p=repos/o/r; gh api -X POST \"\$p/rel\"\"eases\"")"
refused 'forbidden release or publish command' "$(base "    steps:
      - run: gh api \"\$URL\" -f tag_name=v1.2.3")"
refused 'workflow references secrets' "$(base "    env:
      GH_TOKEN: \${{ secrets['RELEASE_TOKEN'] }}
$good_steps")"
refused 'workflow references secrets' "$(base "    env:
      ALL: \${{ toJSON(secrets) }}
$good_steps")"
refused 'is not a standard shell' "$(base "    defaults: {run: {shell: 'python3 {0}'}}
$good_steps")"
refused 'is not a standard shell' "$(base "    steps:
      - shell: 'bash -c \"gh release create v1 {0}\"'
        run: echo hi")"
refused 'docker run image is not digest-pinned' "$(base "    steps:
      - run: docker run --rm alpine:latest echo example@sha256:0000000000000000000000000000000000000000000000000000000000000000")"
refused 'docker run image is not digest-pinned' "$(base "    steps:
      - run: docker run --rm -v \"\$PWD:/src\" alpine:latest")"
refused 'forbidden release or publish command' "$(base "    steps:
      - run: |
          curl -X POST -H \"Authorization: Bearer \$T\" https://api.github.com/repos/o/r/releases -d '{}'")"
# Round-3 forms: mixed-case secrets context, several docker invocations on
# one line, and an unsupported option posing as the image.
refused 'workflow references secrets' "$(base "    env:
      GH_TOKEN: \${{ SeCrEtS.RELEASE_TOKEN }}
$good_steps")"
refused 'docker run image is not digest-pinned' "$(base "    steps:
      - run: docker run --rm $image true; docker run --rm alpine:latest true")"
refused 'docker run image is not digest-pinned' "$(base "    steps:
      - run: docker run --label-file $image alpine:latest")"
refused 'docker run image is not digest-pinned' "$(base "    steps:
      - run: docker run --rm $image printf x#y; docker run --rm alpine:latest true")"
# Live devrev forms: global options, the container group, create, other tools.
for form in 'docker container run --rm ubuntu:latest true' \
	'docker --context x run --rm ubuntu:latest true' \
	'docker -H unix:///var/run/docker.sock run ubuntu:latest true' \
	'docker create ubuntu:latest' 'podman run --rm ubuntu:latest true' \
	'/usr/bin/docker run ubuntu:latest' 'docker --unknown-global x run ubuntu:latest' \
	'docker pull ubuntu:latest' 'docker build -t x .'; do
	refused 'docker run image is not digest-pinned' "$(base "    steps:
      - run: $form")"
done
# Image-free subcommands and compose stay accepted.
base "    steps:
      - run: docker compose -f docker-compose.localstack.yml up -d localstack
      - run: docker container run --rm $image true
      - run: docker ps
      - run: docker --context ci -H unix:///var/run/docker.sock run --rm $image true
      - run: podman create $image" >"$scratch/compose.yml"
python3 -I "$policy" "$scratch/compose.yml"
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
