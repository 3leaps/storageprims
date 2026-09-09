#pragma once

/* Generated with cbindgen:0.29.2 */

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

enum StorageprimsErrorCode
#ifdef __cplusplus
  : int32_t
#endif // __cplusplus
 {
  Ok = 0,
  InvalidUri = 1,
  InvalidArgument = 2,
  NotFound = 3,
  ContainerNotFound = 4,
  AccessDenied = 5,
  InvalidCredentials = 6,
  Throttled = 7,
  ProviderUnavailable = 8,
  UnsupportedCapability = 9,
  Conflict = 10,
  Io = 11,
  Other = 99,
};
#ifndef __cplusplus
typedef int32_t StorageprimsErrorCode;
#endif // __cplusplus

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

const char *storageprims_version(void);

uint32_t storageprims_abi_version(void);

/**
 * Initialize a storageprims FFI runtime and return an opaque handle.
 *
 * Returns `0` on failure and records the failure detail in the thread-local
 * last-error slot available through `storageprims_last_error_code` and
 * `storageprims_last_error`.
 */
uint64_t storageprims_init(void);

StorageprimsErrorCode storageprims_shutdown(uint64_t handle);

/**
 * Free a string returned by a storageprims FFI function.
 *
 * # Safety
 *
 * `s` must either be null or a pointer previously returned by a storageprims
 * FFI function that transfers string ownership to the caller.
 */
void storageprims_free_string(char *s);

/**
 * Create a provider instance from JSON configuration and return an opaque provider id.
 *
 * # Safety
 *
 * `config_json` must be a valid, NUL-terminated UTF-8 C string.
 * `out_provider_id` must be non-null and writable for a `u64`.
 */
StorageprimsErrorCode storageprims_provider_create(uint64_t handle,
                                                   const char *config_json,
                                                   uint64_t *out_provider_id);

StorageprimsErrorCode storageprims_provider_destroy(uint64_t handle, uint64_t provider_id);

/**
 * Return the provider's callable capabilities as a JSON array.
 *
 * This query reads only the in-process provider registry and does not perform
 * credential resolution, provider probes, or network I/O.
 *
 * # Safety
 *
 * `out_capabilities_json` must be non-null and writable for a `char*` returned
 * by `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_provider_capabilities(uint64_t handle,
                                                         uint64_t provider_id,
                                                         char **out_capabilities_json);

/**
 * List objects via the JSON control plane.
 *
 * # Safety
 *
 * `request_json` must be a valid, NUL-terminated UTF-8 C string.
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_list(uint64_t handle,
                                        uint64_t provider_id,
                                        const char *request_json,
                                        char **out_result_json);

/**
 * Fetch object metadata via the JSON control plane.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * `out_metadata_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_head(uint64_t handle,
                                        uint64_t provider_id,
                                        const char *key,
                                        char **out_metadata_json);

/**
 * Delete an object via the JSON control plane.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_delete(uint64_t handle,
                                          uint64_t provider_id,
                                          const char *key,
                                          char **out_result_json);

/**
 * Copy an object via the JSON control plane.
 *
 * # Safety
 *
 * `request_json` must be a valid, NUL-terminated UTF-8 C string.
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_copy(uint64_t handle,
                                        uint64_t provider_id,
                                        const char *request_json,
                                        char **out_result_json);

/**
 * Validate provider credentials and connectivity via the JSON control plane.
 *
 * # Safety
 *
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_probe(uint64_t handle,
                                         uint64_t provider_id,
                                         char **out_result_json);

/**
 * Read the first `n` logical lines via the JSON control plane.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * If non-null, `opts_json` must be a valid, NUL-terminated UTF-8 JSON string.
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_head_lines(uint64_t handle,
                                              uint64_t provider_id,
                                              const char *key,
                                              uint64_t n,
                                              const char *opts_json,
                                              char **out_result_json);

/**
 * Read the last `n` logical lines via the JSON control plane.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * If non-null, `opts_json` must be a valid, NUL-terminated UTF-8 JSON string.
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_tail_lines(uint64_t handle,
                                              uint64_t provider_id,
                                              const char *key,
                                              uint64_t n,
                                              const char *opts_json,
                                              char **out_result_json);

/**
 * Read the first `n` logical lines after midpoint alignment via the JSON control plane.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * If non-null, `opts_json` must be a valid, NUL-terminated UTF-8 JSON string.
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_mid_lines(uint64_t handle,
                                             uint64_t provider_id,
                                             const char *key,
                                             uint64_t n,
                                             const char *opts_json,
                                             char **out_result_json);

/**
 * Count logical lines via the JSON control plane.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_count_lines(uint64_t handle,
                                               uint64_t provider_id,
                                               const char *key,
                                               char **out_result_json);

StorageprimsErrorCode storageprims_last_error_code(void);

char *storageprims_last_error(void);

void storageprims_clear_error(void);

/**
 * Open a readable stream descriptor for an object.
 *
 * The returned JSON descriptor includes both an OS file descriptor and a
 * `stream_id`. Callers must drain or close the descriptor and then call
 * `storageprims_get_finalize` with that `stream_id` to release the pending
 * read state and surface any late read failure. Skipping finalization leaks
 * the tracked read stream and treats late worker failure as unobserved.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * `out_stream_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`. The returned fd is backed by a bounded OS pipe,
 * so callers should keep draining it; otherwise the provider worker may block
 * under backpressure until the fd is drained or closed.
 */
StorageprimsErrorCode storageprims_get(uint64_t handle,
                                       uint64_t provider_id,
                                       const char *key,
                                       char **out_stream_json);

/**
 * Open a readable stream descriptor for a byte range.
 *
 * The returned JSON descriptor includes both an OS file descriptor and a
 * `stream_id`. Callers must drain or close the descriptor and then call
 * `storageprims_get_finalize` with that `stream_id` to release the pending
 * read state and surface any late read failure. Skipping finalization leaks
 * the tracked read stream and treats late worker failure as unobserved.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * `out_stream_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`. The returned fd is backed by a bounded OS pipe,
 * so callers should keep draining it; otherwise the provider worker may block
 * under backpressure until the fd is drained or closed.
 */
StorageprimsErrorCode storageprims_get_range(uint64_t handle,
                                             uint64_t provider_id,
                                             const char *key,
                                             uint64_t offset,
                                             uint64_t length,
                                             char **out_stream_json);

/**
 * Begin a streamed upload and return a writable descriptor.
 *
 * # Safety
 *
 * `key` must be a valid, NUL-terminated UTF-8 C string.
 * If non-null, `metadata_json` must be a valid, NUL-terminated UTF-8 JSON string.
 * `out_stream_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`. The returned fd is the write end of a bounded
 * OS pipe, so producer writes may block until the provider worker drains the
 * pipe or the fd is closed.
 */
StorageprimsErrorCode storageprims_put_begin(uint64_t handle,
                                             uint64_t provider_id,
                                             const char *key,
                                             const char *metadata_json,
                                             char **out_stream_json);

/**
 * Finalize a previously opened streamed upload and return the JSON put result.
 *
 * # Safety
 *
 * `out_result_json` must be non-null and writable for a `char*` returned by
 * `storageprims_free_string`.
 */
StorageprimsErrorCode storageprims_put_finalize(uint64_t handle,
                                                uint64_t stream_id,
                                                char **out_result_json);

/**
 * Finalize a previously opened read stream and surface any late read failure.
 *
 * Call this exactly once after the consumer has drained or closed the file
 * descriptor returned by `storageprims_get` or `storageprims_get_range`.
 * Finalization releases the pending read entry and reports any failure that
 * occurred while copying provider bytes into the pipe.
 */
StorageprimsErrorCode storageprims_get_finalize(uint64_t handle, uint64_t stream_id);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus
