//! Minimal, allocation-light HTTP header handling for the `https://`
//! probe.
//!
//! We deliberately do not depend on `http::HeaderMap` in the
//! transport-agnostic layer: the scripted transport used by the
//! conformance fixtures must be able to express *malformed* header sets
//! (duplicate `ETag`, non-ASCII values, …) that a typed `HeaderMap`
//! would reject at construction time. Probe correctness is judged on
//! how it handles hostile servers, so the fixture layer has to be able
//! to build hostile inputs.

/// A case-insensitive, order-preserving, duplicate-preserving list of
/// HTTP response header fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    fields: Vec<(String, String)>,
}

impl Headers {
    /// An empty header set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a header field. Duplicates are preserved (they are
    /// diagnostically meaningful — see [`Headers::get_all`]).
    pub fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.fields.push((name.into(), value.into()));
    }

    /// Builder form of [`Headers::push`].
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.push(name, value);
        self
    }

    /// First value for `name` (case-insensitive), with surrounding
    /// optional whitespace trimmed. Returns `None` for an absent field
    /// **and** for a present-but-empty field: an empty `ETag:` header
    /// carries no freshness signal, and treating it as a signal would
    /// be exactly the invariant-#7 mistake.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.get_all(name).into_iter().next()
    }

    /// Every value for `name` (case-insensitive), trimmed, empties
    /// dropped.
    #[must_use]
    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.fields
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
            .filter(|v| !v.is_empty())
            .collect()
    }

    /// Iterate over the raw `(name, value)` pairs in wire order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Number of header fields (counting duplicates separately).
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the header set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Headers {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut headers = Self::new();
        for (k, v) in iter {
            headers.push(k, v);
        }
        headers
    }
}

/// The subset of `Cache-Control` directives that affect trust class.
///
/// Parsed leniently: unknown directives are ignored, quoted-string
/// argument forms are tolerated, and directive names are matched
/// case-insensitively (RFC 9111 §5.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // these are four independent HTTP directives, not state
pub struct CacheControl {
    /// `no-store` present.
    pub no_store: bool,
    /// `must-revalidate` (or `proxy-revalidate`) present.
    pub must_revalidate: bool,
    /// `no-cache` present.
    pub no_cache: bool,
    /// `immutable` present (RFC 8246).
    pub immutable: bool,
    /// `max-age=<delta-seconds>`, if present and parseable.
    pub max_age: Option<u64>,
}

impl CacheControl {
    /// Parse every `Cache-Control` field in `headers` (multiple fields
    /// are equivalent to one comma-joined field, RFC 9110 §5.3).
    #[must_use]
    pub fn parse(headers: &Headers) -> Self {
        let mut out = Self::default();
        for field in headers.get_all("Cache-Control") {
            for directive in field.split(',') {
                let directive = directive.trim();
                let (name, arg) = match directive.split_once('=') {
                    Some((n, a)) => (n.trim(), Some(a.trim().trim_matches('"'))),
                    None => (directive, None),
                };
                if name.eq_ignore_ascii_case("no-store") {
                    out.no_store = true;
                } else if name.eq_ignore_ascii_case("must-revalidate")
                    || name.eq_ignore_ascii_case("proxy-revalidate")
                {
                    out.must_revalidate = true;
                } else if name.eq_ignore_ascii_case("no-cache") {
                    out.no_cache = true;
                } else if name.eq_ignore_ascii_case("immutable") {
                    out.immutable = true;
                } else if name.eq_ignore_ascii_case("max-age") {
                    out.max_age = arg.and_then(|a| a.parse::<u64>().ok());
                }
            }
        }
        out
    }

    /// Does the server forbid caching assumptions outright?
    ///
    /// Per the decision matrix in `docs/research/http-validity-probes.md`,
    /// `no-store` and `must-revalidate` both force `volatile`.
    #[must_use]
    pub const fn forbids_caching(self) -> bool {
        self.no_store || self.must_revalidate
    }
}

/// Is `value` a syntactically plausible HTTP-date?
///
/// We accept the preferred IMF-fixdate form only
/// (`Sun, 06 Nov 1994 08:49:37 GMT`, RFC 9110 §5.6.7). The two obsolete
/// forms exist but a probe that *emits* `If-Modified-Since` should emit
/// the preferred form, and a recorded fingerprint that is not in the
/// preferred form is treated as malformed rather than silently
/// forwarded — a server that cannot parse our `If-Modified-Since`
/// answers `200`, which we would misread as drift.
#[must_use]
pub fn is_imf_fixdate(value: &str) -> bool {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    // Www, DD Mon YYYY HH:MM:SS GMT  == 29 octets, fixed width.
    let b = value.as_bytes();
    if b.len() != 29 || !value.is_ascii() {
        return false;
    }
    if !DAYS.contains(&&value[0..3]) {
        return false;
    }
    if &value[3..5] != ", " {
        return false;
    }
    if !value[5..7].bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if b[7] != b' ' || b[11] != b' ' {
        return false;
    }
    if !MONTHS.contains(&&value[8..11]) {
        return false;
    }
    if !value[12..16].bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if b[16] != b' ' {
        return false;
    }
    // HH:MM:SS
    let time = &value[17..25];
    if !time[0..2].bytes().all(|c| c.is_ascii_digit())
        || !time[3..5].bytes().all(|c| c.is_ascii_digit())
        || !time[6..8].bytes().all(|c| c.is_ascii_digit())
    {
        return false;
    }
    if time.as_bytes()[2] != b':' || time.as_bytes()[5] != b':' {
        return false;
    }
    &value[25..29] == " GMT"
}

/// Parsed `Retry-After` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryAfter {
    /// `Retry-After: <delta-seconds>`.
    Seconds(u64),
    /// `Retry-After: <HTTP-date>`. We do not convert to a duration:
    /// that requires a trusted clock and date arithmetic the probe does
    /// not own. The raw value is surfaced so the caller (engine
    /// scheduler) can decide.
    HttpDate(String),
}

impl RetryAfter {
    /// Parse a `Retry-After` field value. Returns `None` if absent or
    /// unparseable in either permitted form.
    #[must_use]
    pub fn parse(headers: &Headers) -> Option<Self> {
        let raw = headers.get("Retry-After")?;
        if let Ok(secs) = raw.parse::<u64>() {
            return Some(Self::Seconds(secs));
        }
        if is_imf_fixdate(raw) {
            return Some(Self::HttpDate(raw.to_string()));
        }
        None
    }
}
