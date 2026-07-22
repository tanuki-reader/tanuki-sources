//! The mechanical half of the ABI: linear-memory allocation, `(ptr,len)` packing, and the two host
//! imports. This is the **only** module in the SDK containing `unsafe`, and the only one a source
//! author never has to read.
//!
//! Host counterpart: `app.tanuki.core.source.abi.SourceAbi` and the memory-ABI helpers at the bottom
//! of `ChicoryWasmEngine.kt` / the `tanuki_call` function in `native/wasmi-cabi`. Any change here is a
//! breaking ABI change — bump `ABI_VERSION` and `SourceAbi.VERSION` together.
//!
//! # Ownership rules (get these wrong and you leak or double-free)
//!
//! Calls *into* the guest (`manga_list`, …):
//!   - the **host** allocates the input buffer via [`tanuki_alloc`] and frees it after the call
//!   - the **guest** allocates the output buffer via [`tanuki_alloc`]; the **host** frees it after
//!     reading. The guest must NOT free what it returns.
//!
//! Calls *out of* the guest ([`http_request`]):
//!   - the **guest** passes a pointer into its own memory (a live `Vec`); the host only reads it
//!   - the **host** allocates the reply via [`tanuki_alloc`]; the **guest** frees it after reading.

use std::alloc::Layout;

/// ABI revision this SDK speaks. Must equal `SourceAbi.VERSION` on the host, which refuses to run a
/// guest reporting anything else.
pub const ABI_VERSION: i32 = 2;

/// Allocates `size` bytes in linear memory and returns the pointer as an `i32`.
///
/// The `#[no_mangle] tanuki_alloc` export that wraps this is emitted by [`export_source!`], **in the
/// source crate**, not here — a `#[no_mangle]` symbol in a dependency rlib isn't guaranteed to be
/// exported from the final `.wasm`, and the host needs this export unconditionally to place input
/// buffers (and `http_request` replies) into guest memory. Returns `0` for a non-positive size,
/// which callers treat as the empty buffer.
///
/// # Safety
/// The region is untracked by Rust until someone calls [`dealloc`] with the *same* size. Alignment
/// is 1: every buffer crossing this ABI is raw UTF-8 JSON bytes.
pub fn alloc(size: i32) -> i32 {
    if size <= 0 {
        return 0;
    }
    match Layout::from_size_align(size as usize, 1) {
        // SAFETY: size > 0 and align 1, so the layout is valid and non-zero-sized.
        Ok(layout) => unsafe { std::alloc::alloc(layout) as i32 },
        Err(_) => 0,
    }
}

/// Frees a region previously returned by [`alloc`]. `size` must match the original request. A null
/// pointer or non-positive size is a no-op, so double-frees of empty buffers are harmless.
///
/// Like [`alloc`], its `#[no_mangle] tanuki_dealloc` export is emitted by [`export_source!`]. This one
/// is especially prone to being dropped: a source rarely *reaches* dealloc itself (the host is the
/// caller), so without the macro's forced export the linker would strip it and every host-side free
/// would fail.
pub fn dealloc(ptr: i32, size: i32) {
    if ptr == 0 || size <= 0 {
        return;
    }
    if let Ok(layout) = Layout::from_size_align(size as usize, 1) {
        // SAFETY: caller guarantees ptr came from alloc with this exact size.
        unsafe { std::alloc::dealloc(ptr as *mut u8, layout) }
    }
}

/// Packs a `(ptr, len)` pair into the single `i64` every bytes-returning export hands back — high 32
/// bits the pointer, low 32 the length. A WASM function can only return one scalar, which is the
/// entire reason this encoding exists.
///
/// Mirrors `packPtrLen` in `ChicoryWasmEngine.kt`.
pub fn pack(ptr: i32, len: i32) -> i64 {
    ((ptr as i64) << 32) | ((len as i64) & 0xFFFF_FFFF)
}

/// Splits a packed `i64` back into `(ptr, len)`. Uses a *logical* shift so a pointer with the high
/// bit set doesn't sign-extend into a negative pointer (mirrors Kotlin's `ushr`).
pub fn unpack(packed: i64) -> (i32, i32) {
    (((packed as u64) >> 32) as i32, (packed & 0xFFFF_FFFF) as i32)
}

/// Copies `bytes` into a freshly allocated linear-memory region and returns it packed for return to
/// the host. The host frees the region after reading it — do not free it here.
pub fn bytes_to_host(bytes: &[u8]) -> i64 {
    let len = bytes.len() as i32;
    let ptr = alloc(len);
    if ptr == 0 {
        return pack(0, 0);
    }
    // SAFETY: ptr was just allocated with exactly `len` bytes and the regions cannot overlap.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len()) }
    pack(ptr, len)
}

/// Borrows a host-provided input buffer as a slice. The host owns and frees it, so the returned
/// slice is only valid for the duration of the export call — never store it.
///
/// # Safety
/// Caller must pass a `(ptr, len)` pair exactly as received from the host.
pub unsafe fn borrow_from_host<'a>(ptr: i32, len: i32) -> &'a [u8] {
    if ptr == 0 || len <= 0 {
        return &[];
    }
    std::slice::from_raw_parts(ptr as *const u8, len as usize)
}

/// Takes ownership of a host-allocated reply buffer: copies it out and frees the original. Used for
/// [`http_request`] replies, which (unlike export inputs) the guest is responsible for freeing.
///
/// # Safety
/// Caller must pass a packed value produced by the host's `http_request` import.
pub unsafe fn take_from_host(packed: i64) -> Vec<u8> {
    let (ptr, len) = unpack(packed);
    if ptr == 0 || len <= 0 {
        return Vec::new();
    }
    let owned = std::slice::from_raw_parts(ptr as *const u8, len as usize).to_vec();
    dealloc(ptr, len);
    owned
}

// --- Host imports -------------------------------------------------------------------------------
// Bound by the engine under the import module "tanuki" (SourceAbi.Imports.MODULE). These are the
// ONLY capabilities a source has: it cannot open a socket, touch the filesystem, or read a clock.

#[link(wasm_import_module = "tanuki")]
extern "C" {
    /// `http_request(ptr,len) -> packed(ptr,len)`. Input is `HttpRequest` JSON in *guest* memory;
    /// the reply is `HttpResponse` JSON in a host-allocated buffer the guest must free.
    ///
    /// Synchronous from here — the host blocks its worker thread on a suspending Ktor call.
    pub fn http_request(ptr: i32, len: i32) -> i64;

    /// `log(level, ptr, len)`. `level` is the ordinal of `LogLevel`; the message is UTF-8 in guest
    /// memory. No return value.
    pub fn log(level: i32, ptr: i32, len: i32);
}
