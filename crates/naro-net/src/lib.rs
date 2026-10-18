//! Networking and URL handling. Only plain HTTP/HTTPS GET and local files.

use std::io::Read;
use std::time::Duration;

use naro_html::escape_html;

pub struct FetchedPage {
    pub url: String,
    pub html: String,
    /// Pre-fetched `<img src>` payloads, keyed by absolute URL. Images are
    /// fetched off the UI thread together with the document so `set_html`
    /// never blocks on the network.
    pub images: Vec<(String, Vec<u8>)>,
}

/// User-Agent sent with every request.
///
/// A browser-compatible prefix with NARO's own token appended. Several sites
/// (google.com among them) answer a non-browser agent with an *empty* 200
/// response for their stylesheets, which silently drops the whole cascade;
/// identifying as a mainstream engine is what makes those resources arrive.
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/141.0.0.0 Safari/537.36 NARO/0.1";

const MAX_BODY: u64 = 8 * 1024 * 1024;
/// Stylesheet fetching is bounded so one page cannot stall navigation.
const MAX_SHEETS: usize = 12;
const MAX_CSS: u64 = 512 * 1024;
const MAX_CSS_TOTAL: usize = 3 * 1024 * 1024;
/// Image budget per page: enough for a real article, small enough to bound
/// navigation latency and memory.
const MAX_IMAGES: usize = 12;
const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SCRIPTS: usize = 8;
const MAX_SCRIPT_BYTES: u64 = 512 * 1024;

pub fn fetch(url: &str) -> Result<FetchedPage, String> {
    // ureq does not pick `native-tls` up as a default backend; the connector is
    // passed explicitly so HTTPS works without pulling in a C crypto library.
    let connector = native_tls::TlsConnector::new()
        .map_err(|e| format!("não foi possível inicializar o TLS: {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .tls_connector(std::sync::Arc::new(connector))
        .build();

    let response = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("HTTP {code} em {url}"));
        }
        Err(err) => return Err(format!("{err} ({url})")),
    };

    let final_url = response.get_url().to_string();
    let content_type = response
        .header("content-type")
        .unwrap_or("")
        .to_ascii_lowercase();

    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY)
        .read_to_end(&mut body)
        .map_err(|e| format!("falha ao ler a resposta: {e}"))?;
    let text = String::from_utf8_lossy(&body).to_string();

    let is_html = content_type.is_empty()
        || content_type.contains("text/html")
        || content_type.contains("application/xhtml")
        || content_type.contains("text/xml")
        || content_type.contains("application/xml");

    let html = if is_html {
        inline_stylesheets(&agent, &final_url, &text)
    } else {
        format!("<!doctype html><html><head><title>{}</title></head><body><pre>{}</pre></body></html>",
            escape_html(&final_url),
            escape_html(&text))
    };

    let mut images = fetch_images(&agent, &final_url, &html);
    images.extend(fetch_scripts(&agent, &final_url, &html));
    Ok(FetchedPage { url: final_url, html, images })
}

/// Fetch `<link rel=stylesheet>` targets and inline them as `<style>` blocks.
///
/// Real pages put almost all of their styling in external sheets; without this
/// every custom element renders as an unstyled inline box. Bounded to
/// `MAX_SHEETS` requests and `MAX_CSS` bytes each, and failures are skipped.
fn inline_stylesheets(agent: &ureq::Agent, base: &str, html: &str) -> String {
    let doc = naro_html::Document::parse(html);
    let mut sheets: Vec<String> = Vec::new();
    let mut total = 0usize;
    for id in 0..doc.nodes.len() {
        if doc.tag(id) != Some("link") {
            continue;
        }
        let rel = doc.attr(id, "rel").unwrap_or("").to_ascii_lowercase();
        if !rel.split_whitespace().any(|r| r == "stylesheet") {
            continue;
        }
        let href = match doc.attr(id, "href") {
            Some(h) if !h.trim().is_empty() => h,
            _ => continue,
        };
        if sheets.len() >= MAX_SHEETS || total >= MAX_CSS_TOTAL {
            break;
        }
        let url = match resolve(base, href) {
            Some(u) => u,
            None => continue,
        };
        if let Ok(css) = fetch_text(agent, &url) {
            if !css.is_empty() && total + css.len() <= MAX_CSS_TOTAL {
                total += css.len();
                // `</style>` inside CSS would terminate the block early.
                sheets.push(css.replace("</style", "<\\/style"));
            }
        }
    }
    if sheets.is_empty() {
        return html.to_string();
    }

    let mut injected = String::with_capacity(html.len() + 64);
    for css in sheets {
        injected.push_str("<style>");
        injected.push_str(&css);
        injected.push_str("</style>");
    }

    match find_ci(html, "</head") {
        Some(pos) => {
            let mut out = String::with_capacity(html.len() + injected.len());
            out.push_str(&html[..pos]);
            out.push_str(&injected);
            out.push_str(&html[pos..]);
            out
        }
        None => {
            let mut out = String::with_capacity(html.len() + injected.len());
            out.push_str(&injected);
            out.push_str(html);
            out
        }
    }
}

/// Fetch every distinct `<img src>` in document order, up to `MAX_IMAGES`.
/// Failures are skipped: a broken image must not break the page.
pub fn fetch_images(agent: &ureq::Agent, base: &str, html: &str) -> Vec<(String, Vec<u8>)> {
    let doc = naro_html::Document::parse(html);
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    // `background-image: url(...)` inside inline <style> blocks.
    let mut css_urls: Vec<String> = Vec::new();
    {
        let bytes = html.as_bytes();
        let mut i = 0usize;
        while let Some(pos) = html[i..].find("url(") {
            let start = i + pos + 4;
            if let Some(end) = html[start..].find(')') {
                let raw = html[start..start + end].trim().trim_matches(|c| c == '"' || c == '\'');
                if !raw.is_empty() && !raw.starts_with("data:") && !raw.starts_with('#') {
                    if let Some(abs) = resolve(base, raw) {
                        css_urls.push(abs);
                    }
                }
                i = start + end + 1;
            } else {
                break;
            }
            let _ = bytes;
        }
    }
    for url in css_urls {
        if out.len() >= MAX_IMAGES {
            break;
        }
        if seen.iter().any(|u| u == &url) {
            continue;
        }
        seen.push(url.clone());
        if let Ok(bytes) = fetch_bytes(agent, &url, MAX_IMAGE_BYTES) {
            out.push((url, bytes));
        }
    }
    for id in 0..doc.nodes.len() {
        if out.len() >= MAX_IMAGES {
            break;
        }
        if doc.tag(id) != Some("img") {
            continue;
        }
        let src = match doc.attr(id, "src") {
            Some(src) if !src.trim().is_empty() => src,
            _ => continue,
        };
        if src.starts_with("data:") {
            continue;
        }
        let url = match resolve(base, src) {
            Some(url) => url,
            None => continue,
        };
        if seen.iter().any(|u| u == &url) {
            continue;
        }
        seen.push(url.clone());
        if let Ok(bytes) = fetch_bytes(agent, &url, MAX_IMAGE_BYTES) {
            out.push((url, bytes));
        }
    }
    out
}

/// Fetch `<script src>` payloads so the JS runtime can execute them.
fn fetch_scripts(agent: &ureq::Agent, base: &str, html: &str) -> Vec<(String, Vec<u8>)> {
    let doc = naro_html::Document::parse(html);
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for id in 0..doc.nodes.len() {
        if out.len() >= MAX_SCRIPTS {
            break;
        }
        if doc.tag(id) != Some("script") {
            continue;
        }
        let src = match doc.attr(id, "src") {
            Some(src) if !src.trim().is_empty() => src,
            _ => continue,
        };
        let url = match resolve(base, src) {
            Some(url) => url,
            None => continue,
        };
        if seen.iter().any(|u| u == &url) {
            continue;
        }
        seen.push(url.clone());
        if let Ok(bytes) = fetch_bytes(agent, &url, MAX_SCRIPT_BYTES) {
            out.push((url, bytes));
        }
    }
    out
}

/// Fetch raw bytes with a hard size cap; used for images.
pub fn fetch_bytes(agent: &ureq::Agent, url: &str, max: u64) -> Result<Vec<u8>, String> {
    let response = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code} em {url}")),
        Err(err) => return Err(format!("{err} ({url})")),
    };
    let mut body = Vec::new();
    response
        .into_reader()
        .take(max)
        .read_to_end(&mut body)
        .map_err(|e| format!("falha ao ler {url}: {e}"))?;
    Ok(body)
}

/// Build an agent with NARO's transport settings (timeouts, UA, TLS).
pub fn agent() -> Result<ureq::Agent, String> {
    let connector = native_tls::TlsConnector::new()
        .map_err(|e| format!("não foi possível inicializar o TLS: {e}"))?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .tls_connector(std::sync::Arc::new(connector))
        .build())
}

fn fetch_text(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    let response = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code} em {url}")),
        Err(err) => return Err(format!("{err} ({url})")),
    };
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_CSS)
        .read_to_end(&mut body)
        .map_err(|e| format!("falha ao ler {url}: {e}"))?;
    Ok(String::from_utf8_lossy(&body).to_string())
}

/// Case-insensitive substring search.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

pub fn resolve(base: &str, href: &str) -> Option<String> {
    let base = url::Url::parse(base).ok()?;
    let joined = base.join(href).ok()?;
    Some(joined.to_string())
}

pub fn normalize_input(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "naro://start".to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("file://")
        || lower.starts_with("naro://")
    {
        return trimmed.to_string();
    }
    let path = std::path::Path::new(trimmed);
    if path.exists() {
        if let Ok(u) = url::Url::from_file_path(path) {
            return u.to_string();
        }
    }
    format!("https://{trimmed}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_bare_hosts() {
        assert_eq!(normalize_input("example.com"), "https://example.com");
        assert_eq!(normalize_input("  https://a.com  "), "https://a.com");
        assert_eq!(normalize_input("naro://start"), "naro://start");
        assert_eq!(normalize_input(""), "naro://start");
    }

    #[test]
    fn resolves_relative_urls() {
        assert_eq!(
            resolve("https://a.com/x/y", "../z").as_deref(),
            Some("https://a.com/z")
        );
        assert_eq!(
            resolve("https://a.com/x/y", "/root").as_deref(),
            Some("https://a.com/root")
        );
        assert_eq!(
            resolve("https://a.com/x/y", "https://b.com/q").as_deref(),
            Some("https://b.com/q")
        );
    }
}
