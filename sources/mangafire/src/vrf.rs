//! VRF request signing.
//!
//! MangaFire's `/api/*` endpoints reject any request without a valid `vrf` query parameter. The token is
//! a keyed transform of the request's **canonical signed input**, produced by a three-stage substitution
//! cipher and emitted as URL-safe base64 (no padding). Faithful port of the keiyoushi extension's
//! `VrfSigner.kt` — keep the two in sync if MangaFire rotates the constants.
//!
//! # Signed input (built by the caller, see `canonical_signing_input` in lib.rs)
//! The `/api`-stripped request path, then — if any — the query parameters **sorted by key** and joined
//! with `&`, where a key ending in `[]` is rewritten to `[0]`, `[1]`, … per repeat, using the params'
//! **decoded** values. E.g. `GET /api/titles?page=1&limit=50&order[views_30d]=desc` signs
//! `"/titles?limit=50&order[views_30d]=desc&page=1"`.
//!
//! # Cipher (per stage)
//! ```text
//! prev = IV
//! for i in 0..len:
//!     prev = TABLE[(data[i] ^ KEY[i % KEY.len()] ^ prev) & 0xFF]
//!     out[i] = prev
//! ```
//! The three stages run in order; the result is URL-safe-base64-encoded into the `vrf` value.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;

/// Whether the signing constants below are real (vs. placeholder). `true` now that they're transcribed
/// from `VrfSigner.kt`; the source's API calls only work while this holds.
pub const CONFIGURED: bool = true;

/// One stage: its 256-byte substitution table and repeating key (both base64, decoded per call — the
/// input is short and this runs once per request), and its seed IV. Verbatim from `VrfSigner.kt`.
struct Stage {
    table_b64: &'static str,
    key_b64: &'static str,
    iv: u8,
}

const TABLE_1: &str = "yINlmUNho8VYJT+ibTIP+9ESiULpVEtMOoD6U6lRE0R/xwXo/Xp9NrUgC4cw/Lmo33vUyjUE40kUoEWIr/fxfNNcq2s79ShQ5NhNrFnJ4hXPwOu/SuXzIbuTQKGFvfm08E9jvCfqAtoDqvQq3dVWPQFmJjgvkISBeXY3BgANR+yVnjGbcxZ47d6kLNfZPIayTq3/YGySb1KuVZodWp/WGNAO5pfMcpaK53Hhs0allBszaMaxuouOwdxbwgxIw6YunSsXjI05Yi0j9j4eHKfSXR8Ifo/Od+8iamRfCXTyvm7NGRGYdcQ0ywcK/u6RXhrbcCm4t2eCtrDgQVecJGkQ+A==";
const KEY_1: &str = "0Ec58JOY3uBzJK9m3zqIOpdlF7UFiax9DmA=";
const TABLE_2: &str = "IUFltCxD3Oc2cwCgkJffthaOg9cgPUb0LgW6H/VtfcF0kc5F25t+aWj6JH9VOhOaY0rAFdUxlDnl5BLNvwEJvQtP5qcw7vdb/K+chnbwnspSHT8mz5lqwz41TezG0hkO06FTjJZhsyNuFLDpD2ZZxQj/QIRcF90zpmQ7Byu483WsQqUE0C342HL+JXngRB6fRzxRyVTaKu83h7UYTJ0QMt6ixFh6S3F8gqkKwrGTL3jHNBsD45UnifK8+RGtishQV2K3rujLKEkiZxpr2dYcudFW4oFsDKhad3CLBvuyTqsCo4B7mL5IKQ1vXo/MOOvq1I1d8ar9X6Ttu5KF4fZgiA==";
const KEY_2: &str = "AAdjb1iPY8CiDmq9H34tKTBF8a3oDQ==";
const TABLE_3: &str = "NQHlu1/wVO5EmkwQymF810qqY2xG1k2obcas4Z9mCsPEIFl9pRIjFxbJ7ybMHbBckT5Ton85E0FOeHezbh/mjlEYpmpnlXOS8dgrqeq2KfxImTh1YK9y0PeMNhzA1OQzSY9brYOJq/l2QnE/hwOeZIhPixVSKIUlDb5vLcH6RWKxkIEMuP0bDwIqQ71AJJaEaMJL7A6YtyIwoRT+L5v4aZzodN/0+3nOGsfblFjgxSfPzVDjNFeNl5P26+kEC/8AHgdrpAbt3hHz3HrRN1Y6e+JHgF7ncFWnoF0y3THL1S71WgWGCa6KtSzTCCG58n68nTyj2T3Sshk7utqCtMi/ZQ==";
const KEY_3: &str = "DELOJgPsVaCcblDtTGMdHzM=";

static STAGES: [Stage; 3] = [
    Stage { table_b64: TABLE_1, key_b64: KEY_1, iv: 0x5A },
    Stage { table_b64: TABLE_2, key_b64: KEY_2, iv: 0x35 },
    Stage { table_b64: TABLE_3, key_b64: KEY_3, iv: 0xBA },
];

/// Signs the canonical [input] into a `vrf` token, or `None` if the cipher is unconfigured or a constant
/// fails to decode (defensive — shouldn't happen) so the caller reports a clear error instead of trapping.
pub fn sign(input: &str) -> Option<String> {
    if !CONFIGURED {
        return None;
    }
    let mut data = input.as_bytes().to_vec();
    for stage in &STAGES {
        let table = STANDARD.decode(stage.table_b64).ok()?;
        let key = STANDARD.decode(stage.key_b64).ok()?;
        if key.is_empty() || table.len() < 256 {
            return None;
        }
        let mut prev = stage.iv;
        for (i, byte) in data.iter_mut().enumerate() {
            let idx = (*byte ^ key[i % key.len()] ^ prev) as usize;
            prev = table[idx];
            *byte = prev;
        }
    }
    Some(URL_SAFE_NO_PAD.encode(&data))
}
