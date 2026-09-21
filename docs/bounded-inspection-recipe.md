# Storage access cost recipe

Choose the narrowest operation that proves what the caller needs:

| Need                                 | Operation                               | Cost boundary                                                                                                                                           |
| ------------------------------------ | --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Object metadata                      | `StorageProvider::head`                 | One metadata request; no body preview                                                                                                                   |
| First N bytes of one selected source | `storageprims_ops::preview_bytes`       | Guarded observation, selected guarded HEAD, then one guarded range request; the finite inspection budget counts helper requests and admitted body bytes |
| A few text lines                     | bounded line helper                     | Text framing shares the inspection budget and rejects oversized pending lines rather than returning a partial complete result                           |
| Enumerate keys                       | `StorageProvider::list`                 | One page per call; continuation tokens are opaque and pagination cost is caller-controlled                                                              |
| Download content                     | `StorageProvider::get` or a guarded get | Streaming data plane; consumer controls its read limit, timeout, and cancellation                                                                       |

`preview_bytes` returns a first-N prefix or a range response proven clipped at
EOF. It is not a full download and it fails if the provider cannot enforce the
selected source or prove its range response. The default inspection ledger is
8 MiB payload, 16 helper requests, 1 MiB output, 1 MiB pending-line space, and
30 seconds; callers can raise finite values only up to the documented caps.

The logical ledger counts helper calls and bytes delivered to this crate. SDK
retries and transport prefetch are not an exact network-egress accounting API.
Dropping the operation releases its owned reader; it does not start a detached
drain. `force_stream` does not decompress compressed data.
