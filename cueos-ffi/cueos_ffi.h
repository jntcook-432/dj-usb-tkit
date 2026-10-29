/* C FFI surface of the dj-usb-tkit backend for CueOS (macOS + iPadOS).
 *
 * All returned strings are heap-allocated by Rust and MUST be freed with
 * cueos_string_free. Every JSON response matches the tkit wire contract:
 *   {"ok": true, "data": {...}} | {"ok": false, "error": {"code", "message"}}
 * Payload JSON mirrors the camelCase request structs in backend/src/models.rs.
 * Note: the tkit workspace builds with panic=abort in release; callers must
 * treat a process abort during a call as a fatal engine failure.
 */
#ifndef CUEOS_FFI_H
#define CUEOS_FFI_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Initialize the engine with a data directory (created if missing).
 * Returns a JSON string; caller frees with cueos_string_free.
 * Re-initializing replaces the previous engine. */
char *cueos_engine_init(const char *data_dir);

/* Dispatch a command by name. `payload_json` may be NULL for commands that
 * take no request. Returns a JSON ApiResponse string (never NULL unless
 * allocation fails). Caller frees with cueos_string_free. */
char *cueos_command(const char *name, const char *payload_json);

/* Free a string returned by this library. Safe to call with NULL. */
void cueos_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* CUEOS_FFI_H */
