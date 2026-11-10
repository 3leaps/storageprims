# Guides

storageprims lets one code path read and write objects in an S3 bucket or a
local directory. These guides show how the crates fit together, how to use the
local provider for storage and for tests, and how to write code that does not
care where its data lives. Design rationale lives in
[`docs/decisions/`](../decisions/README.md).

| Guide                                                                 | Read it when                                                                                                           |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| [How the libraries fit together](how-the-libraries-fit.md)            | You are new to storageprims and want the shape of the crates, the provider contract, optional capabilities, and errors |
| [The local filesystem provider](local-provider.md)                    | You want to store objects in a directory with the same API you use for S3                                              |
| [Testing with the local provider](testing-with-the-local-provider.md) | You want fast, hermetic tests for code that talks to object storage                                                    |
| [Writing location-independent code](location-independent-code.md)     | You want one code path that runs against a local directory or a bucket                                                 |

Runnable examples, compiled by CI on every supported platform:

- [`crates/storageprims-local/examples/local_roundtrip.rs`](../../crates/storageprims-local/examples/local_roundtrip.rs):
  put, create-only put, get, head, flat and delimiter listing, delete.
- [`crates/storageprims-ops/examples/portable_store.rs`](../../crates/storageprims-ops/examples/portable_store.rs):
  choose a provider from a `file://` or `s3://` URI and relay an object between
  providers.
- [`crates/storageprims-s3/examples/list.rs`](../../crates/storageprims-s3/examples/list.rs):
  list an S3 prefix with the AWS default credential chain.
