# storageprims-s3

AWS S3 and S3-compatible object-storage provider for `storageprims`.

See the [storageprims repository](https://github.com/3leaps/storageprims) for
configuration, credential, and integration-test documentation.

The default provider probe performs `HeadBucket` against the configured
bucket. Success establishes configured-bucket reachability only; it does not
prove authorization for list, get, put, or other object operations.

`CredentialsFile` configuration is rejected during provider construction.
Supply at most one non-empty root source: either the URI path or
`TargetConfig.root_prefix`.

Licensed under MIT OR Apache-2.0.
