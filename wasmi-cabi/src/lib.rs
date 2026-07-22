//! Thin C ABI shim over [wasmi](https://docs.rs/wasmi) for the Tanuki iOS WASM source engine.
//!
//! Mirrors the Android `ChicoryWasmEngine`: opaque engine/module handles, the memory ABI, and the
//! host-import bridge. Kotlin/Native reaches these via cinterop (`wasmi.def`). See
//! `docs/ios-wasmi-plan.md` for the staged buildout and `docs/wasm-abi.md` for the ABI itself.
//!
//! **Step 4 — host-import bridge:** the guest's `tanuki.log` / `tanuki.http_request` imports are bound
//! on the wasmi `Linker` and dispatched to a single C callback ([`TanukiHostCall`]) carried in the
//! store, whose `env` is an opaque pointer to a Kotlin `SourceHost`. The synchronous → suspend bridge
//! (e.g. `runBlocking` on `http_request`) lives on the Kotlin side of that callback.

use std::ffi::{c_char, c_void, CStr};

use wasmi::{Caller, Engine, Extern, Instance, Linker, Module, Store};

extern "C" {
    /// libc `free`, used to release the response buffer a [`TanukiHostCall`] allocated (with the
    /// matching system `malloc` on the Kotlin side) for an `http_request` reply.
    fn free(ptr: *mut c_void);
}

/// The single host-import callback the shim invokes when a guest calls a `tanuki.*` import.
///
/// - `env`: opaque pointer the embedder passed to [`tanuki_module_new`] (a Kotlin `SourceHost`).
/// - `kind`: `0` = http_request, `1` = log.
/// - `arg`: per-kind scalar — the log level for `log`; unused (`0`) for `http_request`.
/// - `in_ptr`/`in_len`: the input bytes (the request JSON for http; the UTF-8 message for log).
/// - `out_ptr`/`out_len`: for http, the callee sets these to a `malloc`'d response buffer the shim
///   reads then `free`s; for log they are left untouched.
///
/// Returns `0` on success, non-zero on failure.
pub type TanukiHostCall = extern "C" fn(
    env: *mut c_void,
    kind: i32,
    arg: i32,
    in_ptr: *const u8,
    in_len: i32,
    out_ptr: *mut *mut u8,
    out_len: *mut i32,
) -> i32;

/// Store data for an instantiated module: the host callback + its opaque env. Read by the linker's
/// import closures via `Caller::data`.
struct HostState {
    host_call: TanukiHostCall,
    env: *mut c_void,
}

/// Opaque handle wrapping a wasmi [`Engine`]. One per process (`Engine` is `Arc`-backed).
pub struct TanukiEngine {
    engine: Engine,
}

/// Opaque handle for one instantiated source: its [`Store`] + [`Instance`].
pub struct TanukiModule {
    store: Store<HostState>,
    instance: Instance,
}

/// A byte buffer the shim hands back across the ABI. Owned by Rust: the caller reads `ptr[..len]`, then
/// returns it to [`tanuki_buf_free`]. On error `len < 0` and `ptr` is null.
#[repr(C)]
pub struct TanukiBuf {
    ptr: *mut u8,
    len: i32,
}

impl TanukiBuf {
    fn error() -> Self {
        TanukiBuf {
            ptr: std::ptr::null_mut(),
            len: -1,
        }
    }

    fn from_vec(bytes: Vec<u8>) -> Self {
        let boxed = bytes.into_boxed_slice();
        let len = boxed.len() as i32;
        let ptr = Box::into_raw(boxed) as *mut u8;
        TanukiBuf { ptr, len }
    }
}

/// Creates a new engine. Free with [`tanuki_engine_free`]. Never null.
#[no_mangle]
pub extern "C" fn tanuki_engine_new() -> *mut TanukiEngine {
    Box::into_raw(Box::new(TanukiEngine {
        engine: Engine::default(),
    }))
}

/// Frees an engine from [`tanuki_engine_new`]. Null is a no-op.
#[no_mangle]
pub extern "C" fn tanuki_engine_free(engine: *mut TanukiEngine) {
    if !engine.is_null() {
        unsafe { drop(Box::from_raw(engine)) };
    }
}

/// Compiles `wasm[..wasm_len]` on `engine` and instantiates it, binding the `tanuki.*` imports to
/// `host_call`/`env`. Returns an opaque module handle, or null on invalid args / compile /
/// instantiation failure. Free with [`tanuki_module_free`]. `env` must outlive the module.
///
/// # Safety
/// `wasm` must point to at least `wasm_len` readable bytes; `engine` must be a live handle; `host_call`
/// must be a valid function pointer for the module's lifetime.
#[no_mangle]
pub extern "C" fn tanuki_module_new(
    engine: *mut TanukiEngine,
    wasm: *const u8,
    wasm_len: i32,
    host_call: TanukiHostCall,
    env: *mut c_void,
) -> *mut TanukiModule {
    if engine.is_null() || wasm.is_null() || wasm_len < 0 {
        return std::ptr::null_mut();
    }
    let engine = unsafe { &(*engine).engine };
    let bytes = unsafe { std::slice::from_raw_parts(wasm, wasm_len as usize) };

    let module = match Module::new(engine, bytes) {
        Ok(module) => module,
        Err(_) => return std::ptr::null_mut(),
    };
    let mut store = Store::new(engine, HostState { host_call, env });
    let mut linker = <Linker<HostState>>::new(engine);
    if bind_host_imports(&mut linker).is_err() {
        return std::ptr::null_mut();
    }
    let instance = match linker.instantiate_and_start(&mut store, &module) {
        Ok(instance) => instance,
        Err(_) => return std::ptr::null_mut(),
    };
    Box::into_raw(Box::new(TanukiModule { store, instance }))
}

/// Frees a module from [`tanuki_module_new`]. Null is a no-op.
#[no_mangle]
pub extern "C" fn tanuki_module_free(module: *mut TanukiModule) {
    if !module.is_null() {
        unsafe { drop(Box::from_raw(module)) };
    }
}

/// Calls the guest's `abi_version` export. Returns the version (>= 1), or -1 on error.
#[no_mangle]
pub extern "C" fn tanuki_abi_version(module: *mut TanukiModule) -> i32 {
    if module.is_null() {
        return -1;
    }
    let module = unsafe { &mut *module };
    let func = match module
        .instance
        .get_typed_func::<(), i32>(&module.store, "abi_version")
    {
        Ok(func) => func,
        Err(_) => return -1,
    };
    func.call(&mut module.store, ()).unwrap_or(-1)
}

/// Calls a bytes-in/bytes-out export, performing the memory ABI (alloc + write input, `export(ptr,len)
/// -> i64`, read packed result, free both guest regions). Returns a shim-owned [`TanukiBuf`] — copy it
/// out then call [`tanuki_buf_free`]; [`TanukiBuf::error`] on failure.
///
/// # Safety
/// `export` must be a valid NUL-terminated C string; `input` must point to `input_len` readable bytes
/// (or be null when `input_len == 0`); `module` must be a live handle.
#[no_mangle]
pub extern "C" fn tanuki_call(
    module: *mut TanukiModule,
    export: *const c_char,
    input: *const u8,
    input_len: i32,
) -> TanukiBuf {
    if module.is_null() || export.is_null() || input_len < 0 {
        return TanukiBuf::error();
    }
    let module = unsafe { &mut *module };
    let export = match unsafe { CStr::from_ptr(export) }.to_str() {
        Ok(name) => name,
        Err(_) => return TanukiBuf::error(),
    };
    let input: &[u8] = if input.is_null() || input_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(input, input_len as usize) }
    };

    match call_export(module, export, input) {
        Ok(out) => TanukiBuf::from_vec(out),
        Err(()) => TanukiBuf::error(),
    }
}

/// Frees a [`TanukiBuf`] returned by [`tanuki_call`]. A null/error buffer is a no-op.
///
/// # Safety
/// `buf` must be a buffer previously returned by [`tanuki_call`] and not already freed.
#[no_mangle]
pub extern "C" fn tanuki_buf_free(buf: TanukiBuf) {
    // len == 0 still owns a (zero-size) boxed slice from `from_vec`; only the error buf (len < 0) is null.
    if !buf.ptr.is_null() && buf.len >= 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(buf.ptr, buf.len as usize);
            drop(Box::from_raw(slice as *mut [u8]));
        }
    }
}

/// Registers the `tanuki.log` and `tanuki.http_request` imports, each forwarding to the store's
/// [`TanukiHostCall`]. Extra definitions are harmless for guests that import nothing.
fn bind_host_imports(linker: &mut Linker<HostState>) -> Result<(), ()> {
    linker
        .func_wrap(
            "tanuki",
            "log",
            |caller: Caller<HostState>, level: i32, ptr: i32, len: i32| {
                let Some(memory) = caller.get_export("memory").and_then(Extern::into_memory) else {
                    return;
                };
                let mut msg = vec![0u8; len.max(0) as usize];
                if memory.read(&caller, ptr as usize, &mut msg).is_err() {
                    return;
                }
                let (host_call, env) = {
                    let state = caller.data();
                    (state.host_call, state.env)
                };
                let mut out_ptr: *mut u8 = std::ptr::null_mut();
                let mut out_len: i32 = 0;
                host_call(env, 1, level, msg.as_ptr(), msg.len() as i32, &mut out_ptr, &mut out_len);
                // log has no reply; release anything the callee allocated defensively.
                if !out_ptr.is_null() {
                    unsafe { free(out_ptr as *mut c_void) };
                }
            },
        )
        .map_err(|_| ())?;

    linker
        .func_wrap(
            "tanuki",
            "http_request",
            |mut caller: Caller<HostState>, req_ptr: i32, req_len: i32| -> i64 {
                let Some(memory) = caller.get_export("memory").and_then(Extern::into_memory) else {
                    return 0;
                };
                let mut req = vec![0u8; req_len.max(0) as usize];
                if memory.read(&caller, req_ptr as usize, &mut req).is_err() {
                    return 0;
                }
                let (host_call, env) = {
                    let state = caller.data();
                    (state.host_call, state.env)
                };
                let mut out_ptr: *mut u8 = std::ptr::null_mut();
                let mut out_len: i32 = 0;
                let rc = host_call(env, 0, 0, req.as_ptr(), req.len() as i32, &mut out_ptr, &mut out_len);
                if rc != 0 || out_ptr.is_null() || out_len < 0 {
                    return 0;
                }
                let resp = unsafe { std::slice::from_raw_parts(out_ptr, out_len as usize) }.to_vec();
                unsafe { free(out_ptr as *mut c_void) };
                write_to_guest(&mut caller, &resp).unwrap_or(0)
            },
        )
        .map_err(|_| ())?;

    Ok(())
}

/// Writes `bytes` into the guest via its `tanuki_alloc` export and returns the packed `(ptr,len)` i64.
fn write_to_guest(caller: &mut Caller<HostState>, bytes: &[u8]) -> Result<i64, ()> {
    let alloc = caller
        .get_export("tanuki_alloc")
        .and_then(Extern::into_func)
        .ok_or(())?;
    let alloc = alloc.typed::<i32, i32>(&*caller).map_err(|_| ())?;
    let ptr = alloc.call(&mut *caller, bytes.len() as i32).map_err(|_| ())?;
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or(())?;
    memory.write(&mut *caller, ptr as usize, bytes).map_err(|_| ())?;
    Ok((((ptr as u64) << 32) | (bytes.len() as u64)) as i64)
}

/// The memory ABI for an export call, in safe wasmi terms. `Err(())` on any wasmi failure.
fn call_export(module: &mut TanukiModule, export: &str, input: &[u8]) -> Result<Vec<u8>, ()> {
    let memory = module
        .instance
        .get_memory(&module.store, "memory")
        .ok_or(())?;
    let alloc = module
        .instance
        .get_typed_func::<i32, i32>(&module.store, "tanuki_alloc")
        .map_err(|_| ())?;
    let dealloc = module
        .instance
        .get_typed_func::<(i32, i32), ()>(&module.store, "tanuki_dealloc")
        .map_err(|_| ())?;
    let func = module
        .instance
        .get_typed_func::<(i32, i32), i64>(&module.store, export)
        .map_err(|_| ())?;

    let in_len = input.len() as i32;
    let in_ptr = alloc.call(&mut module.store, in_len).map_err(|_| ())?;
    memory
        .write(&mut module.store, in_ptr as usize, input)
        .map_err(|_| ())?;

    let packed = func.call(&mut module.store, (in_ptr, in_len)).map_err(|_| ())? as u64;
    let out_ptr = (packed >> 32) as usize;
    let out_len = (packed & 0xFFFF_FFFF) as usize;

    let mut out = vec![0u8; out_len];
    memory.read(&module.store, out_ptr, &mut out).map_err(|_| ())?;

    let _ = dealloc.call(&mut module.store, (in_ptr, in_len));
    let _ = dealloc.call(&mut module.store, (out_ptr as i32, out_len as i32));

    Ok(out)
}
