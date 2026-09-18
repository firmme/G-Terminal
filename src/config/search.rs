//! The web search engines offered for a selection lookup.

/// The engine used when settings carry none.
pub const DEFAULT_SEARCH_ENGINE: &str = "google";

/// Web search engines offered for the terminal's selection lookup. The first
/// field is the key stored in settings; the third is the URL template, with
/// `{}` standing for the percent-encoded query.
pub const SEARCH_ENGINES: [(&str, &str, &str); 4] = [
    ("google", "Google", "https://www.google.com/search?q={}"),
    ("bing", "Bing", "https://www.bing.com/search?q={}"),
    ("duckduckgo", "DuckDuckGo", "https://duckduckgo.com/?q={}"),
    ("baidu", "百度", "https://www.baidu.com/s?wd={}"),
];

/// The display name for a stored engine key.
pub fn search_engine_label(engine: &str) -> &str {
    SEARCH_ENGINES
        .iter()
        .find(|(key, _, _)| *key == engine)
        .map(|(_, label, _)| *label)
        .unwrap_or(engine)
}

/// The search URL for `query` on `engine`. An unknown key falls back to the
/// default engine, so a hand-edited config cannot produce a broken lookup.
pub fn search_url(engine: &str, query: &str) -> String {
    let template = SEARCH_ENGINES
        .iter()
        .find(|(key, _, _)| *key == engine)
        .or_else(|| {
            SEARCH_ENGINES
                .iter()
                .find(|(key, _, _)| *key == DEFAULT_SEARCH_ENGINE)
        })
        .map(|(_, _, template)| *template)
        .unwrap_or("https://www.google.com/search?q={}");
    template.replace("{}", &percent_encode(query))
}

/// Percent-encodes a query for a URL: RFC 3986 unreserved characters stay, and
/// everything else — including the UTF-8 bytes of CJK — is escaped.
pub(super) fn percent_encode(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for byte in query.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}
