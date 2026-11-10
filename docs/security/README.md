# Release Signing and Verification

Official storageprims releases use a GPG-signed annotated Git tag and
`SHA256SUMS` and `SHA512SUMS` manifests. Each manifest carries a minisign
signature. When the release carries OpenPGP manifest signatures, each manifest
also has an `.asc` signature by the committed signing key. These are separate
signatures, and every signature a release ships is required to verify.

Establish the expected GPG primary key and the minisign public-blob
fingerprint through an independently trusted maintainer channel before
accepting downloaded public keys. A key distributed beside a signature is not
independent authentication.

## Public pins

| File                                      | Contents                                                                          |
| ----------------------------------------- | --------------------------------------------------------------------------------- |
| `docs/security/release-signing-keys.asc`  | Public OpenPGP export of the approved primary key and its signing subkey          |
| `docs/security/storageprims-minisign.pub` | The minisign public key that signs the checksum manifests                         |
| `config/release/tagger-identity.txt`      | The exact git tagger identity                                                     |
| `keys/expected-fingerprints.txt`          | Two lines: the OpenPGP primary fingerprint, then the minisign public-blob SHA-256 |
| `keys/expected-fingerprints.ndjson`       | The checker's fingerprint records for the same two keys                           |
| `config/release/decernor-ci-sha256.txt`   | Digest and filename of the checker archive CI downloads                           |

Release staging copies the published `storageprims-minisign.pub` and
`storageprims-release-signing-key.asc` assets from the verified tagged commit
as inert blobs, not from the current checkout, an environment path, or a live
keyring export. Fingerprint verification of those assets is still required.

## Fingerprint anchors

The text and NDJSON anchors in each tagged commit are derived by
`decernor 0.1.8` or newer. The minisign fingerprint covers the decoded public
blob, not its comment line or the whole file. Verify exported public material
against both anchors with that tool, then use the approved minisign public key
to verify both manifests and check every listed digest. Require the complete
expected asset set.

## Tags

A release is one tag, `vX.Y.Z`, matching `VERSION`. It must be annotated, carry
the committed tagger identity, and be signed by a subkey of the committed
primary key. Tag verification imports only the committed public export into an
isolated keyring and requires exactly one valid signature from that key. The
forge's own "verified" flag is an additional check. Historical tags are not
retroactively signed.

Every workflow runs with read-only repository permissions and does not persist
checkout credentials. The tag workflow restores the annotated tag with the job
token, verifies it against the committed pin, and builds the FFI archives and
SBOM as workflow artifacts. It does not sign, create a GitHub release, or
publish a package. A passing tag workflow is a consistency signal; it is not
release authorization.

## Publication

The maintainer binds the tag to `main`, creates the draft release from the
verified workflow artifacts, signs the manifests locally, and publishes.
Registry upload and the GitHub release are separate steps; neither authorizes
the other.

## Not applicable

- The storageprims libraries verify no signatures, so they embed no key.
- No signing key has been retired; there are no retired pins.
