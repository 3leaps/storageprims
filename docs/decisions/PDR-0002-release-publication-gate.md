# PDR-0002 — Release publication gate

Status: Accepted

## Decision

Starting with the first signed version tag, the tag signature authorizes the
exact commit and the release artifacts built from it. The signature is verified
against the reviewed public key in `docs/security/release-signing-keys.asc`,
using an isolated keyring. GitHub's independent Verified result and an active
tag publication ruleset are additional requirements. The tag's signed message
includes the digest of the expected ruleset; no draft release is created until
all three checks pass. Earlier unsigned annotated releases remain historical.

The maintainer authors `message.txt` outside the repository for each cut. The
local signed tag must match that text, the policy attestation, the fixed infosec
tagger and the exact commit on `origin/main`. Creating and pushing the tag are
separate actions. CI has no signing credentials.
Private signing material stays outside the working tree, as required by the
[3 Leaps OSS Sensitive Local Data Policy](https://github.com/3leaps/oss-policies/blob/main/SENSITIVE-LOCAL-DATA.md).

The public pin is loaded from the tagged commit itself. Review and merge of
the pin through a PR _before_ the tag, combined with tag protection and the
verified infosec signer, is the authorization boundary; an account-level
GitHub Verified indicator alone is insufficient.

The release checksum set includes `expected-fingerprints.txt` and
`expected-fingerprints.ndjson`. The maintainer generates these from the GPG
primary export and minisign public export with a pinned minimum decernor
binary; reviewers re-derive them. The public pin and both anchors must land in
the reviewed commit before a tag is signed. Rotating a key requires a prior
reviewed change to the pin and anchors; a key registered on a GitHub account
does not substitute for the committed pin.
The public record schema at `schemas/fingerprint-record.v0.schema.json` is a
local copy of decernor's v0 fingerprint-record contract for offline validation
at anchor generation time.

The committed GPG fingerprint identifies the primary key. The reviewed public
export carries the permitted signing subkey, selected exactly with the `!`
form. The host-local primary fingerprint is an operator selector and must
equal the committed anchor. The tagger identity is fixed by
`config/release/tagger-identity.txt` for local and CI verification.
