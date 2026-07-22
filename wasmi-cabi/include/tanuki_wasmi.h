#ifndef TANUKI_WASMI_H
#define TANUKI_WASMI_H

#include <stdint.h>

/* Opaque handles owned by Rust; created/freed only through the functions below. */
typedef struct TanukiEngine TanukiEngine;
typedef struct TanukiModule TanukiModule;

/* Host-import callback. kind: 0 = http_request, 1 = log. arg: log level (log) / unused (http).
   in_ptr/in_len: input bytes (request JSON / log message). For http, the callee sets out_ptr to a
   malloc'd reply buffer (freed by the shim) and out_len to its length; for log they are left alone.
   Returns 0 on success, non-zero on failure. */
typedef int32_t (*TanukiHostCall)(void *env, int32_t kind, int32_t arg,
                                  const uint8_t *in_ptr, int32_t in_len,
                                  uint8_t **out_ptr, int32_t *out_len);

/* Engine lifecycle. tanuki_engine_new never returns NULL; free with tanuki_engine_free. */
TanukiEngine *tanuki_engine_new(void);
void tanuki_engine_free(TanukiEngine *engine);

/* Compile + instantiate a module, binding the tanuki.* imports to host_call/env. Returns NULL on
   failure; free with tanuki_module_free. env must outlive the module. */
TanukiModule *tanuki_module_new(TanukiEngine *engine, const uint8_t *wasm, int32_t wasm_len,
                                TanukiHostCall host_call, void *env);
void tanuki_module_free(TanukiModule *module);

/* The ABI version handshake export. Returns the guest's version (>= 1), or -1 on error. */
int32_t tanuki_abi_version(TanukiModule *module);

/* A shim-owned byte buffer. On success len >= 0 and ptr is valid; on error len < 0 and ptr is NULL.
   Read ptr[0..len], then return it to tanuki_buf_free. */
typedef struct TanukiBuf {
  uint8_t *ptr;
  int32_t len;
} TanukiBuf;

/* Calls a bytes-in/bytes-out export, performing the memory ABI through the guest's allocator. */
TanukiBuf tanuki_call(TanukiModule *module, const char *export, const uint8_t *input, int32_t input_len);
void tanuki_buf_free(TanukiBuf buf);

#endif /* TANUKI_WASMI_H */
