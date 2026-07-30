//! Small helpers every source ends up needing, kept here so they're written once.
//!
//! Nothing in this module talks to the host — it is pure functions over strings. The bar for adding
//! something is that a *second* source needed it: a guest is a `.wasm` bundle whose size the app
//! downloads, so the SDK earns its bytes by being shared, not by being complete.

/// Percent-encodes one URL component (RFC 3986's unreserved set passes through; everything else
/// becomes `%XX`).
///
/// Hand-rolled rather than pulled from a crate: sources need exactly this much of the URL spec, and a
/// dependency here would land in every `.wasm` bundle.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Builds a `key=value&…` query string, percent-encoding each value.
///
/// Keys are emitted verbatim — a source writes its own (`order[views_30d]`), and encoding the
/// brackets would change the parameter's meaning on sites that use PHP-style array keys.
pub fn query_string(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Crude HTML → plain text: drops tag runs, decodes the handful of entities that show up in
/// synopses, and collapses whitespace.
///
/// A guest can't afford a real HTML parser in wasm, and a description only needs readable text, not
/// structure. Sources that must *scrape* markup need something better than this; sources that merely
/// receive an HTML-formatted synopsis from a JSON API do not.
pub fn strip_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parses a `YYYY-MM-DD?HH:MM:SS` timestamp into Unix epoch **milliseconds**, or `None` if it doesn't
/// look like one.
///
/// Accepts both separators sites actually emit — RFC 3339's `T` and the SQL-ish space — because the
/// date and time fields sit at fixed offsets and the byte between them is never read. A trailing zone
/// (`Z`, `+03:00`, `-0300` is *not* accepted — the colon is required) is applied; **no zone means
/// UTC**, which is a guess a source should overrule if it knows the site's timezone. A fractional
/// second is skipped, not rounded.
///
/// Hand-rolled because a guest has no clock and no date crate worth ~100 KB of `.wasm`.
pub fn parse_datetime_millis(value: &str) -> Option<i64> {
    let field = |range: std::ops::Range<usize>| -> Option<i64> { value.get(range)?.parse::<i64>().ok() };

    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Whatever follows the seconds is the zone, possibly behind a fractional part we don't need.
    let zone = value[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let zone_offset_seconds = match zone.as_bytes().first() {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            let magnitude = hours * 3600 + minutes * 60;
            if *sign == b'-' { -magnitude } else { magnitude }
        }
        Some(_) => return None,
    };

    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some((seconds - zone_offset_seconds) * 1000)
}

/// Days from the Unix epoch to a proleptic-Gregorian date, by Howard Hinnant's `days_from_civil`.
/// Branch-free and exact for every year we'll ever see; the shifted-year trick puts the leap day last
/// so February's length never needs a special case.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The first number in a string, as an ABI chapter number — `"Capítulo 12.5"` → `12.5`.
///
/// Sites that don't carry a numeric chapter field name their chapters in prose, and the app sorts and
/// dedupes by number, so pulling one out matters. Returns `None` when there's no digit to find, which
/// the host reads as "unknown" rather than a wrong `0`.
pub fn first_number(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(u8::is_ascii_digit)?;

    let mut end = start;
    let mut seen_dot = false;
    while end < bytes.len() {
        match bytes[end] {
            b'0'..=b'9' => end += 1,
            // A single decimal point continues the number, but only with a digit behind it, so a
            // trailing "Cap. 3." doesn't swallow the dot.
            b'.' if !seen_dot && matches!(bytes.get(end + 1), Some(b'0'..=b'9')) => {
                seen_dot = true;
                end += 1;
            }
            _ => break,
        }
    }
    text[start..end].parse().ok()
}
