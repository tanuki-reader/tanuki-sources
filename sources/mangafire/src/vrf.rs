//! VRF request signing.
//!
//! MangaFire's `/api/*` endpoints reject any request without a valid `vrf` query parameter. The token is
//! a keyed transform of the request's signed input, produced by a **three-stage substitution cipher**
//! and emitted as URL-safe base64 (no padding). This mirrors the reference `VrfSigner.kt` in the
//! keiyoushi MangaFire extension.
//!
//! # Algorithm (per stage)
//! Each stage walks the input bytes, chaining each output into the next (a CBC-like feedback), keyed by a
//! repeating key and substituted through a 256-byte S-box, seeded by a per-stage IV:
//! ```text
//! prev = IV
//! for i in 0..len:
//!     prev = TABLE[(data[i] ^ KEY[i % KEY.len()] ^ prev) & 0xFF]
//!     out[i] = prev
//! ```
//! The three stages run in order; the final bytes are URL-safe-base64-encoded into the `vrf` value.
//!
//! # ⚠️ NOT YET CONFIGURED — see [`CONFIGURED`]
//! The three S-boxes and three keys are **large binary constants** that must be copied **verbatim** from
//! the reference `VrfSigner.kt` (each is a base64 blob decoding to the byte arrays below). They are left
//! as zeroed placeholders here because a transcription error would be undetectable — every request would
//! silently 403 — so [`sign`] is gated behind [`CONFIGURED`] and the source reports a clear, actionable
//! error until the real values (and the exact *signed input*, see [`sign`]) are filled in.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// Flip to `true` once [`STAGES`]' tables/keys are the real values transcribed from `VrfSigner.kt`.
/// While `false`, callers must not sign — [`sign`] returns `None` and the source surfaces a setup error.
pub const CONFIGURED: bool = false;

/// One stage of the cipher: its 256-byte substitution table, its repeating key, and its seed IV.
struct Stage {
    table: &'static [u8; 256],
    key: &'static [u8],
    iv: u8,
}

// --- Placeholder constants — REPLACE with the decoded bytes from VrfSigner.kt's base64 blobs. ----------
// Each `TABLE_n` is the 256-byte S-box; each `KEY_n` the stage key. The IVs (0x5A / 0x35 / 0xBA) are the
// real values from the reference and can stay.
static TABLE_1: [u8; 256] = [0; 256];
static TABLE_2: [u8; 256] = [0; 256];
static TABLE_3: [u8; 256] = [0; 256];
static KEY_1: &[u8] = &[];
static KEY_2: &[u8] = &[];
static KEY_3: &[u8] = &[];

static STAGES: [Stage; 3] = [
    Stage { table: &TABLE_1, key: KEY_1, iv: 0x5A },
    Stage { table: &TABLE_2, key: KEY_2, iv: 0x35 },
    Stage { table: &TABLE_3, key: KEY_3, iv: 0xBA },
];

/// Signs [input] into a `vrf` token, or `None` when the cipher isn't configured (see the module note) —
/// a signal for the caller to report a setup error rather than send an invalid token.
///
/// TODO(vrf): confirm the exact **signed input**. The reference sorts the request's query parameters and
/// signs that canonical string; wire that construction in at the call site so this receives the same bytes
/// the reference does. Until [`CONFIGURED`], the output here is meaningless regardless.
pub fn sign(input: &str) -> Option<String> {
    if !CONFIGURED {
        return None;
    }
    let mut data = input.as_bytes().to_vec();
    for stage in &STAGES {
        // Guard the modulo: an empty key would divide by zero, and a guest must never panic.
        if stage.key.is_empty() {
            return None;
        }
        let mut prev = stage.iv;
        for (i, byte) in data.iter_mut().enumerate() {
            // out[i] = TABLE[(in[i] ^ KEY[i % keylen] ^ prev) & 0xFF]; prev = out[i]  (per VrfSigner.kt)
            let idx = (*byte ^ stage.key[i % stage.key.len()] ^ prev) as usize;
            prev = stage.table[idx & 0xFF];
            *byte = prev;
        }
    }
    Some(URL_SAFE_NO_PAD.encode(&data))
}