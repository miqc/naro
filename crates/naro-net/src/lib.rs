//! Networking, URL handling and the HTTP state that belongs to a browser
//! session: connection reuse, a memory cache and cookies.
//!
//! Only plain HTTP/HTTPS GET and POST and local files. One `NetSession` lives
//! as long as the application (or a headless run), so keep-alive, the cache
//! and the cookie jar survive navigation — a page cannot observe another
//! page's cache, but it does observe its cookies, which is the point.

use std::io::Read;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use naro_html::escape_html;

pub struct FetchedPage {
    pub url: String,
    pub html: String,
    /// Subresources the origin policy refused, so the caller can report them
    /// instead of leaving a silently missing stylesheet.
    pub blocked: Vec<String>,
    /// The document's Content-Security-Policy, from the header or a `<meta>`.
    pub csp: Option<Csp>,
    /// Pre-fetched `<img src>` payloads, keyed by absolute URL. Images are
    /// fetched off the UI thread together with the document so `set_html`
    /// never blocks on the network.
    pub images: Vec<(String, Vec<u8>)>,
}

/// One HTTP response, with the body still as bytes.
///
/// `text()` decodes it with the charset the server declared, so a page in
/// `windows-1252` no longer arrives as replacement characters.
pub struct HttpResponse {
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub content_type: String,
    pub body: Vec<u8>,
    /// Lower-cased header names to values. Only the headers a page can
    /// observe are kept, which is what a browser exposes.
    pub headers: Vec<(String, String)>,
}

impl HttpResponse {
    pub fn text(&self) -> String {
        decode_body(&self.body, &self.content_type)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A scheme/host/port triple, the unit the same-origin policy compares.
///
/// `file://` has no host worth speaking of, so every file is its own opaque
/// origin: a local page must not read the network, and the network must not
/// read a local file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl Origin {
    /// The origin of `url`, or `None` when it cannot be parsed.
    pub fn of(url: &str) -> Option<Origin> {
        let parsed = url::Url::parse(url).ok()?;
        let scheme = parsed.scheme().to_ascii_lowercase();
        let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
        let port = parsed.port_or_known_default().unwrap_or(0);
        Some(Origin { scheme, host, port })
    }

    /// `https://host:port`, the serialization a `CORS` header uses.
    pub fn serialize(&self) -> String {
        match (self.scheme.as_str(), self.port) {
            ("http", 80) | ("https", 443) => format!("{}://{}", self.scheme, self.host),
            _ => format!("{}://{}:{}", self.scheme, self.host, self.port),
        }
    }

    /// `file:` and other local schemes are never same-origin with anything.
    pub fn is_local(&self) -> bool {
        matches!(self.scheme.as_str(), "file" | "naro" | "data" | "about")
    }

    pub fn same_origin(&self, other: &Origin) -> bool {
        !self.is_local() && self == other
    }

    /// Does `header` (an `Access-Control-Allow-Origin` value) admit this
    /// origin? `*` admits anything non-local.
    pub fn allowed_by(&self, header: &str) -> bool {
        let header = header.trim();
        if header == "*" {
            return !self.is_local();
        }
        match Origin::of(header) {
            Some(allowed) => !self.is_local() && &allowed == self,
            None => false,
        }
    }
}

/// A Content-Security-Policy, reduced to the three fetch directives that
/// matter for what a page can pull in.
///
/// `'self'`, `'none'` and explicit hosts are understood; nonces, hashes and
/// the other keyword sources are parsed and ignored, which fails closed for
/// inline script (the engine does not distinguish inline from external yet,
/// so a `script-src` without a host simply blocks every script).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Csp {
    /// Lower-cased source lists per directive.
    pub script_src: Option<Vec<String>>,
    pub style_src: Option<Vec<String>>,
    pub img_src: Option<Vec<String>>,
    /// `default-src`, the fallback for any directive not given.
    pub default_src: Option<Vec<String>>,
}

impl Csp {
    /// Parse a policy header. `None` when it declares nothing usable.
    pub fn parse(header: &str) -> Option<Csp> {
        let mut csp = Csp::default();
        let mut any = false;
        for directive in header.split(';') {
            let mut parts = directive.split_whitespace();
            let Some(name) = parts.next() else { continue };
            let sources: Vec<String> = parts.map(|s| s.to_ascii_lowercase()).collect();
            match name.to_ascii_lowercase().as_str() {
                "script-src" => {
                    csp.script_src = Some(sources);
                    any = true;
                }
                "style-src" => {
                    csp.style_src = Some(sources);
                    any = true;
                }
                "img-src" => {
                    csp.img_src = Some(sources);
                    any = true;
                }
                "default-src" => {
                    csp.default_src = Some(sources);
                    any = true;
                }
                _ => {}
            }
        }
        if any {
            Some(csp)
        } else {
            None
        }
    }

    /// May `kind` load `url` from a document at `page_url`?
    ///
    /// An absent policy allows everything, which is the only behaviour that
    /// cannot break a page that declared nothing.
    pub fn allows(&self, kind: &str, page_url: &str, url: &str) -> bool {
        let list = match kind {
            "script" => self.script_src.as_ref().or(self.default_src.as_ref()),
            "style" => self.style_src.as_ref().or(self.default_src.as_ref()),
            "img" => self.img_src.as_ref().or(self.default_src.as_ref()),
            _ => self.default_src.as_ref(),
        };
        let Some(list) = list else { return true };
        if list.iter().any(|s| s == "'none'") {
            return false;
        }
        let target = match Origin::of(url) {
            Some(origin) => origin,
            None => return true,
        };
        let page = Origin::of(page_url);
        for source in list {
            match source.as_str() {
                "'self'" => {
                    if let Some(page) = &page {
                        if &target == page {
                            return true;
                        }
                    }
                }
                "*" => return true,
                // A bare host, optionally with a scheme and a wildcard.
                _ => {
                    let cleaned = source
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .trim_start_matches("*.");
                    let host = cleaned.split('/').next().unwrap_or("");
                    if host.is_empty() {
                        continue;
                    }
                    if target.host == host || target.host.ends_with(&format!(".{host}")) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// What a cross-origin request is allowed to do, per the response headers.
pub enum CorsVerdict {
    /// Same origin, or an opaque `file://` origin talking to itself.
    Allowed,
    /// Cross-origin and permitted by `Access-Control-Allow-Origin`.
    AllowedCross,
    /// Cross-origin and not permitted: the body must not reach script.
    Blocked(String),
}

/// Decide whether a response may be handed to script running at `page`.
pub fn cors_verdict(page: Option<&Origin>, response: &HttpResponse) -> CorsVerdict {
    let Some(page) = page else {
        // No page origin (a headless load): nothing to enforce.
        return CorsVerdict::Allowed;
    };
    let Some(target) = Origin::of(&response.url) else {
        return CorsVerdict::Allowed;
    };
    if page.same_origin(&target) {
        return CorsVerdict::Allowed;
    }
    if page.is_local() || target.is_local() {
        // A local page cannot reach the network and the network cannot reach
        // a local file: this is the rule that keeps `file://` from being a
        // bypass for every other policy.
        return CorsVerdict::Blocked(format!(
            "origem local não acessa {}",
            target.serialize()
        ));
    }
    match response.header("access-control-allow-origin") {
        Some(value) if page.allowed_by(value) => CorsVerdict::AllowedCross,
        _ => CorsVerdict::Blocked(format!(
            "CORS bloqueou {}",
            target.serialize()
        )),
    }
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
/// Cache budget: a browsing session, not a proxy.
const CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;
const CACHE_MAX_ENTRIES: usize = 200;

/// Everything that outlives one navigation.
pub struct NetSession {
    agent: ureq::Agent,
    cache: std::sync::Mutex<HttpCache>,
    cookies: std::sync::Mutex<CookieJar>,
}

impl NetSession {
    /// Poisoning is not a hazard here: nothing panics while a guard is held,
    /// and a poisoned jar is still a usable jar.
    fn cache(&self) -> std::sync::MutexGuard<'_, HttpCache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cookies(&self) -> std::sync::MutexGuard<'_, CookieJar> {
        self.cookies.lock().unwrap_or_else(|e| e.into_inner())
    }
}

static SESSION: std::sync::LazyLock<Result<NetSession, String>> =
    std::sync::LazyLock::new(NetSession::new);

/// The process-wide session: keep-alive, cache and cookies shared by every
/// tab, the way a browser shares them across a profile.
///
/// One session instead of one per tab is what makes a cookie set by a page
/// visible to the next navigation, and it is why a `fetch` from script and a
/// navigation cannot disagree about the cache. Requests still run
/// concurrently: only the cache and the cookie jar are locked, and only for
/// the moment it takes to read or write an entry.
pub fn session() -> Result<&'static NetSession, String> {
    match &*SESSION {
        Ok(session) => Ok(session),
        Err(err) => Err(err.clone()),
    }
}

/// Run `f` against the shared session.
pub fn with_session<T>(f: impl FnOnce(&NetSession) -> Result<T, String>) -> Result<T, String> {
    f(session()?)
}

impl NetSession {
    pub fn new() -> Result<NetSession, String> {
        Ok(NetSession {
            agent: agent()?,
            cache: std::sync::Mutex::new(HttpCache::default()),
            cookies: std::sync::Mutex::new(CookieJar::default()),
        })
    }

    /// Fetch a document and everything it needs to render.
    pub fn fetch(&self, url: &str) -> Result<FetchedPage, String> {
        let response = self.request("GET", url, None, &[])?;
        if response.status >= 400 {
            return Err(format!("HTTP {} em {}", response.status, response.url));
        }
        let final_url = response.url.clone();
        let content_type = response.content_type.to_ascii_lowercase();
        let response_csp = response
            .header("content-security-policy")
            .map(|v| v.to_string());
        let text = response.text();

        let is_html = content_type.is_empty()
            || content_type.contains("text/html")
            || content_type.contains("application/xhtml")
            || content_type.contains("text/xml")
            || content_type.contains("application/xml");

        let mut blocked = Vec::new();
        // The policy is read once, before any subresource moves: a header
        // beats a `<meta>`, and an absent policy allows everything.
        let csp = self.policy_of(response_csp.as_deref(), &text);
        let html = if is_html {
            self.inline_stylesheets(&final_url, &text, &mut blocked, &csp)
        } else {
            format!(
                "<!doctype html><html><head><title>{}</title></head><body><pre>{}</pre></body></html>",
                escape_html(&final_url),
                escape_html(&text)
            )
        };

        let mut images = self.fetch_images(&final_url, &html, &mut blocked, &csp);
        images.extend(self.fetch_scripts(&final_url, &html, &mut blocked, &csp));
        Ok(FetchedPage {
            url: final_url,
            html,
            images,
            blocked,
            csp,
        })
    }

    /// Submit a form with `application/x-www-form-urlencoded`.
    pub fn post_form(&self, url: &str, body: &str) -> Result<FetchedPage, String> {
        let headers = [(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];
        let response = self.request("POST", url, Some(body.as_bytes()), &headers)?;
        if response.status >= 400 {
            return Err(format!("HTTP {} em {}", response.status, response.url));
        }
        let final_url = response.url.clone();
        let mut blocked = Vec::new();
        let body = response.text();
        let csp = self.policy_of(response.header("content-security-policy"), &body);
        let html = self.inline_stylesheets(&final_url, &body, &mut blocked, &csp);
        let mut images = self.fetch_images(&final_url, &html, &mut blocked, &csp);
        images.extend(self.fetch_scripts(&final_url, &html, &mut blocked, &csp));
        Ok(FetchedPage {
            url: final_url,
            html,
            images,
            blocked,
            csp,
        })
    }

    /// One request, served from the cache when it is still fresh.
    ///
    /// A non-2xx status is a *response*, not an error: `fetch` must be able to
    /// see a 404. Only transport failures return `Err`.
    pub fn request(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
        headers: &[(String, String)],
    ) -> Result<HttpResponse, String> {
        let absolute = resolve(url, "").unwrap_or_else(|| url.to_string());
        let cacheable = method.eq_ignore_ascii_case("GET");

        let mut conditional: Vec<(String, String)> = Vec::new();
        if cacheable {
            if let Some(entry) = self.cache().get(&absolute) {
                if entry.is_fresh() {
                    return Ok(entry.to_response(absolute));
                }
                if let Some(etag) = &entry.etag {
                    conditional.push(("if-none-match".to_string(), etag.clone()));
                }
                if let Some(modified) = &entry.last_modified {
                    conditional.push(("if-modified-since".to_string(), modified.clone()));
                }
            }
        }

        let mut request = self.agent.request(method, &absolute);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        for (name, value) in &conditional {
            request = request.set(name, value);
        }
        if let Some(cookie) = self.cookies().header_for(&absolute) {
            request = request.set("cookie", &cookie);
        }

        let result = match body {
            Some(bytes) => request.send_bytes(bytes),
            None => request.call(),
        };
        // A status error still carries the response; that is a valid result
        // for `fetch`, which reports the status instead of throwing.
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(err) => return Err(format!("{err} ({absolute})")),
        };

        let final_url = response.get_url().to_string();
        let status = response.status();
        let status_text = response.status_text().to_string();
        let content_type = response.header("content-type").unwrap_or("").to_string();
        let etag = response.header("etag").map(|v| v.to_string());
        let last_modified = response.header("last-modified").map(|v| v.to_string());
        let max_age = response
            .header("cache-control")
            .and_then(parse_max_age);
        let store = response
            .header("cache-control")
            .map(|v| !v.to_ascii_lowercase().contains("no-store"))
            .unwrap_or(true);
        let set_cookies: Vec<String> = response
            .all("set-cookie")
            .into_iter()
            .map(|v| v.to_string())
            .collect();
        // Only the headers a page can observe are kept: exposing `Set-Cookie`
        // or the raw connection headers would be a leak, not a feature.
        let mut observed: Vec<(String, String)> = Vec::new();
        for name in [
            "content-type",
            "cache-control",
            "etag",
            "last-modified",
            "access-control-allow-origin",
            "access-control-allow-credentials",
            "access-control-allow-methods",
            "access-control-allow-headers",
            "content-security-policy",
            "location",
        ] {
            if let Some(value) = response.header(name) {
                observed.push((name.to_string(), value.to_string()));
            }
        }

        let mut payload = Vec::new();
        response
            .into_reader()
            .take(MAX_BODY)
            .read_to_end(&mut payload)
            .map_err(|e| format!("falha ao ler a resposta: {e}"))?;

        for cookie in set_cookies {
            self.cookies().store(&final_url, &cookie);
        }

        // 304 means "your copy is still good": the body comes from the cache
        // and only its timestamp moves.
        if status == 304 {
            if let Some(entry) = self.cache().refresh(&absolute) {
                return Ok(entry.to_response(final_url));
            }
            return Err(format!("HTTP 304 sem entrada em cache: {final_url}"));
        }

        if cacheable && store && (200..300).contains(&status) {
            let has_validator = etag.is_some() || last_modified.is_some();
            if max_age.is_some() || has_validator {
                self.cache().put(
                    &absolute,
                    CacheEntry {
                        body: payload.clone(),
                        content_type: content_type.clone(),
                        stored_at: Instant::now(),
                        max_age,
                        etag,
                        last_modified,
                        headers: observed.clone(),
                    },
                );
            }
        }

        Ok(HttpResponse {
            status,
            status_text,
            url: final_url,
            content_type,
            body: payload,
            headers: observed,
        })
    }

    /// The `Cookie` header value a request to `url` should carry.
    pub fn cookie_header(&self, url: &str) -> Option<String> {
        self.cookies().header_for(url)
    }

    /// `document.cookie` read: every non-`HttpOnly` cookie visible to `url`.
    pub fn cookie_string(&self, url: &str) -> String {
        self.cookies().visible(url)
    }

    /// `document.cookie = value`: one `name=value` pair, with optional
    /// `path`/`max-age`/`expires` attributes.
    pub fn set_cookie_string(&self, url: &str, value: &str) {
        if value.trim().is_empty() {
            return;
        }
        self.cookies().store(url, value);
    }

    /// Fetch `<link rel=stylesheet>` targets and inline them as `<style>` blocks.
    ///
    /// Real pages put almost all of their styling in external sheets; without
    /// this every custom element renders as an unstyled inline box. Bounded to
    /// `MAX_SHEETS` requests and `MAX_CSS` bytes each, and failures are skipped.
    /// The policy a document declares: the header wins over `<meta>`.
    fn policy_of(&self, header: Option<&str>, html: &str) -> Option<Csp> {
        if let Some(csp) = header.and_then(Csp::parse) {
            return Some(csp);
        }
        let doc = naro_html::Document::parse(html);
        for id in 0..doc.nodes.len() {
            if doc.tag(id) != Some("meta") {
                continue;
            }
            let equiv = doc
                .attr(id, "http-equiv")
                .unwrap_or("")
                .to_ascii_lowercase();
            if equiv != "content-security-policy" {
                continue;
            }
            if let Some(content) = doc.attr(id, "content") {
                if let Some(csp) = Csp::parse(content) {
                    return Some(csp);
                }
            }
        }
        None
    }

    /// Does the policy allow `kind` from `base` at `url`?
    fn policy_allows(csp: &Option<Csp>, kind: &str, base: &str, url: &str) -> bool {
        match csp {
            Some(csp) => csp.allows(kind, base, url),
            None => true,
        }
    }

    fn inline_stylesheets(
        &self,
        base: &str,
        html: &str,
        blocked: &mut Vec<String>,
        csp: &Option<Csp>,
    ) -> String {
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
            if !self.may_load(base, &url) || !Self::policy_allows(csp, "style", base, &url) {
                blocked.push(url);
                continue;
            }
            if let Ok(css) = self.fetch_text(&url) {
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
    fn fetch_images(
        &self,
        base: &str,
        html: &str,
        blocked: &mut Vec<String>,
        csp: &Option<Csp>,
    ) -> Vec<(String, Vec<u8>)> {
        let doc = naro_html::Document::parse(html);
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        // `background-image: url(...)` inside inline <style> blocks.
        let mut css_urls: Vec<String> = Vec::new();
        {
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
            if !self.may_load(base, &url) || !Self::policy_allows(csp, "img", base, &url) {
                blocked.push(url);
                continue;
            }
            if let Ok(bytes) = self.fetch_bytes(&url, MAX_IMAGE_BYTES) {
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
            if !self.may_load(base, &url) || !Self::policy_allows(csp, "img", base, &url) {
                blocked.push(url);
                continue;
            }
            if let Ok(bytes) = self.fetch_bytes(&url, MAX_IMAGE_BYTES) {
                out.push((url, bytes));
            }
        }
        out
    }

    /// Fetch `<script src>` payloads so the JS runtime can execute them.
    fn fetch_scripts(
        &self,
        base: &str,
        html: &str,
        blocked: &mut Vec<String>,
        csp: &Option<Csp>,
    ) -> Vec<(String, Vec<u8>)> {
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
            if !self.may_load(base, &url) || !Self::policy_allows(csp, "script", base, &url) {
                blocked.push(url);
                continue;
            }
            if let Ok(bytes) = self.fetch_bytes(&url, MAX_SCRIPT_BYTES) {
                out.push((url, bytes));
            }
        }
        out
    }

    /// May a document at `page` pull in a subresource at `resource`?
    ///
    /// Two rules, both enforced before any bytes move:
    /// * mixed content — an `https` page never loads an `http` subresource,
    ///   because that resource would be trivially forgeable;
    /// * a `file://` page never reaches the network, which is what stops a
    ///   downloaded HTML file from phoning home.
    pub fn may_load(&self, page: &str, resource: &str) -> bool {
        let (Some(page_origin), Some(resource_origin)) =
            (Origin::of(page), Origin::of(resource))
        else {
            return true;
        };
        if page_origin.is_local() && !resource_origin.is_local() {
            return false;
        }
        if page_origin.scheme == "https" && resource_origin.scheme == "http" {
            return false;
        }
        true
    }

    /// Raw bytes with a hard size cap; used for images and scripts.
    fn fetch_bytes(&self, url: &str, max: u64) -> Result<Vec<u8>, String> {
        let response = self.request("GET", url, None, &[])?;
        if response.status >= 400 {
            return Err(format!("HTTP {} em {url}", response.status));
        }
        let mut body = response.body;
        body.truncate(max as usize);
        Ok(body)
    }

    fn fetch_text(&self, url: &str) -> Result<String, String> {
        let response = self.request("GET", url, None, &[])?;
        if response.status >= 400 {
            return Err(format!("HTTP {} em {url}", response.status));
        }
        let mut text = response.text();
        if text.len() > MAX_CSS as usize {
            text.truncate(MAX_CSS as usize);
        }
        Ok(text)
    }
}

/// `max-age` from a `Cache-Control` header, in seconds.
fn parse_max_age(header: &str) -> Option<Duration> {
    for directive in header.split(',') {
        let directive = directive.trim();
        if let Some(value) = directive.strip_prefix("max-age=") {
            if let Ok(seconds) = value.trim().parse::<u64>() {
                return Some(Duration::from_secs(seconds));
            }
        }
    }
    None
}

/// One stored response.
struct CacheEntry {
    body: Vec<u8>,
    content_type: String,
    stored_at: Instant,
    max_age: Option<Duration>,
    etag: Option<String>,
    last_modified: Option<String>,
    headers: Vec<(String, String)>,
}

impl CacheEntry {
    fn is_fresh(&self) -> bool {
        match self.max_age {
            Some(age) => self.stored_at.elapsed() < age,
            None => false,
        }
    }

    fn to_response(&self, url: String) -> HttpResponse {
        HttpResponse {
            status: 200,
            status_text: "OK".to_string(),
            url,
            content_type: self.content_type.clone(),
            body: self.body.clone(),
            headers: self.headers.clone(),
        }
    }
}

/// In-memory HTTP cache, FIFO eviction, no disk.
#[derive(Default)]
struct HttpCache {
    /// Insertion order is the eviction order.
    entries: Vec<(String, CacheEntry)>,
    bytes: usize,
}

impl HttpCache {
    fn get(&self, url: &str) -> Option<&CacheEntry> {
        self.entries
            .iter()
            .find(|(key, _)| key == url)
            .map(|(_, entry)| entry)
    }

    fn put(&mut self, url: &str, entry: CacheEntry) {
        if entry.body.len() > CACHE_MAX_BYTES {
            return;
        }
        if let Some(pos) = self.entries.iter().position(|(key, _)| key == url) {
            let (_, old) = self.entries.remove(pos);
            self.bytes = self.bytes.saturating_sub(old.body.len());
        }
        while (!self.entries.is_empty())
            && (self.entries.len() >= CACHE_MAX_ENTRIES
                || self.bytes + entry.body.len() > CACHE_MAX_BYTES)
        {
            let (_, evicted) = self.entries.remove(0);
            self.bytes = self.bytes.saturating_sub(evicted.body.len());
        }
        self.bytes += entry.body.len();
        self.entries.push((url.to_string(), entry));
    }

    /// Mark an entry as revalidated, keeping its body.
    fn refresh(&mut self, url: &str) -> Option<CacheEntry> {
        let entry = self
            .entries
            .iter_mut()
            .find(|(key, _)| key == url)
            .map(|(_, entry)| entry)?;
        entry.stored_at = Instant::now();
        Some(CacheEntry {
            body: entry.body.clone(),
            content_type: entry.content_type.clone(),
            stored_at: entry.stored_at,
            max_age: entry.max_age,
            etag: entry.etag.clone(),
            last_modified: entry.last_modified.clone(),
            headers: entry.headers.clone(),
        })
    }
}

/// One cookie, in the subset the web actually uses.
#[derive(Clone, Debug)]
struct Cookie {
    name: String,
    value: String,
    /// The domain the cookie is scoped to; the request host when host-only.
    domain: String,
    /// `true` for a cookie without a `Domain` attribute: it is sent only to
    /// the exact host that set it, never to a subdomain.
    host_only: bool,
    path: String,
    secure: bool,
    http_only: bool,
    /// Unix seconds; `None` is a session cookie.
    expires: Option<u64>,
}

impl Cookie {
    fn matches(&self, host: &str, path: &str, https: bool) -> bool {
        if self.secure && !https {
            return false;
        }
        let domain_ok = if self.host_only {
            host == self.domain
        } else {
            host == self.domain || host.ends_with(&format!(".{}", self.domain))
        };
        if !domain_ok {
            return false;
        }
        path_match(path, &self.path)
    }
}

/// RFC 6265 path matching: `/` matches everything, and `/a` matches `/a/b`
/// but not `/ab`.
fn path_match(request: &str, cookie: &str) -> bool {
    if cookie == "/" || request == cookie {
        return true;
    }
    if !request.starts_with(cookie) {
        return false;
    }
    cookie.ends_with('/') || request.as_bytes().get(cookie.len()) == Some(&b'/')
}

#[derive(Default)]
struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    fn store(&mut self, url: &str, set_cookie: &str) {
        let parsed = match url::Url::parse(url) {
            Ok(parsed) => parsed,
            Err(_) => return,
        };
        let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
        let mut parts = set_cookie.split(';');
        let pair = match parts.next() {
            Some(pair) => pair,
            None => return,
        };
        let (name, value) = match pair.split_once('=') {
            Some((name, value)) => (name.trim().to_string(), value.trim().to_string()),
            None => return,
        };
        if name.is_empty() {
            return;
        }
        let mut cookie = Cookie {
            name,
            value,
            domain: host.clone(),
            host_only: true,
            // The default path is the directory of the request path.
            path: default_path(parsed.path()),
            secure: false,
            http_only: false,
            expires: None,
        };
        let mut expired = false;
        for attribute in parts {
            let attribute = attribute.trim();
            if attribute.is_empty() {
                continue;
            }
            let (key, raw) = match attribute.split_once('=') {
                Some((key, raw)) => (key.trim().to_ascii_lowercase(), raw.trim().to_string()),
                None => (attribute.to_ascii_lowercase(), String::new()),
            };
            match key.as_str() {
                "domain" => {
                    let domain = raw.trim_start_matches('.').to_ascii_lowercase();
                    // A cookie for an unrelated domain is rejected outright.
                    if !domain.is_empty()
                        && (host == domain || host.ends_with(&format!(".{domain}")))
                    {
                        cookie.domain = domain;
                        cookie.host_only = false;
                    } else {
                        return;
                    }
                }
                "path" => {
                    if raw.starts_with('/') {
                        cookie.path = raw;
                    }
                }
                "secure" => cookie.secure = true,
                "httponly" => cookie.http_only = true,
                "max-age" => {
                    if let Ok(seconds) = raw.parse::<i64>() {
                        if seconds <= 0 {
                            expired = true;
                        } else {
                            cookie.expires = Some(now_secs() + seconds as u64);
                        }
                    }
                }
                "expires" => {
                    if let Some(at) = parse_http_date(&raw) {
                        cookie.expires = Some(at);
                        if at <= now_secs() {
                            expired = true;
                        }
                    }
                }
                _ => {}
            }
        }

        // Replace the cookie with the same identity, dropping expired ones.
        self.cookies.retain(|existing| {
            !(existing.name == cookie.name
                && existing.domain == cookie.domain
                && existing.path == cookie.path)
        });
        if expired {
            return;
        }
        self.cookies.push(cookie);
    }

    fn header_for(&self, url: &str) -> Option<String> {
        let (host, path, https) = split_url(url)?;
        let mut pairs: Vec<String> = Vec::new();
        let now = now_secs();
        for cookie in &self.cookies {
            if cookie.expires.map(|at| at <= now).unwrap_or(false) {
                continue;
            }
            if cookie.matches(&host, &path, https) {
                pairs.push(format!("{}={}", cookie.name, cookie.value));
            }
        }
        if pairs.is_empty() {
            None
        } else {
            Some(pairs.join("; "))
        }
    }

    /// `document.cookie`: same filter, minus `HttpOnly`.
    fn visible(&self, url: &str) -> String {
        let Some((host, path, https)) = split_url(url) else {
            return String::new();
        };
        let now = now_secs();
        let mut pairs: Vec<String> = Vec::new();
        for cookie in &self.cookies {
            if cookie.http_only {
                continue;
            }
            if cookie.expires.map(|at| at <= now).unwrap_or(false) {
                continue;
            }
            if cookie.matches(&host, &path, https) {
                pairs.push(format!("{}={}", cookie.name, cookie.value));
            }
        }
        pairs.join("; ")
    }
}

fn split_url(url: &str) -> Option<(String, String, bool)> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some((host, parsed.path().to_string(), parsed.scheme() == "https"))
}

fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(pos) => path[..pos].to_string(),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse the three date formats a `Set-Cookie` uses, returning Unix seconds.
fn parse_http_date(value: &str) -> Option<u64> {
    // `Wdy, DD Mon YYYY HH:MM:SS GMT`
    let cleaned = value.replace(',', " ");
    let mut parts = cleaned.split_whitespace();
    let _weekday = parts.next();
    let day: u32 = parts.next()?.parse().ok()?;
    let month = match parts.next()?.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    let mut clock = time.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next().unwrap_or("0").parse().ok()?;

    // Days from the Unix epoch, proleptic Gregorian, via the civil-from-days
    // algorithm — no chrono dependency for one conversion.
    let year = if year < 100 { year + 1900 } else { year };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(seconds).ok()
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day = day as i64;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Decode a body using the charset the server declared.
///
/// No charset: strict UTF-8 first, then `windows-1252`, which is what a
/// browser does for a legacy page that forgot to say.
pub fn decode_body(body: &[u8], content_type: &str) -> String {
    if let Some(label) = charset_of(content_type) {
        if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
            let (text, _, _) = encoding.decode(body);
            return text.to_string();
        }
    }
    match std::str::from_utf8(body) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let (text, _, _) = encoding_rs::WINDOWS_1252.decode(body);
            text.to_string()
        }
    }
}

fn charset_of(content_type: &str) -> Option<String> {
    for part in content_type.split(';') {
        let part = part.trim();
        if let Some(value) = part.to_ascii_lowercase().strip_prefix("charset=") {
            let _ = value;
            return Some(
                part.split_once('=')
                    .map(|(_, v)| v.trim().trim_matches('"').to_string())
                    .unwrap_or_default(),
            );
        }
    }
    None
}

/// Build an agent with NARO's transport settings (timeouts, UA, TLS).
fn agent() -> Result<ureq::Agent, String> {
    let connector = native_tls::TlsConnector::new()
        .map_err(|e| format!("não foi possível inicializar o TLS: {e}"))?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .tls_connector(std::sync::Arc::new(connector))
        .build())
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

    #[test]
    fn decodes_the_declared_charset_and_falls_back() {
        // 0xE7 is `ç` in windows-1252 and invalid on its own in UTF-8.
        let latin = [0xE7, b'a'];
        assert_eq!(decode_body(&latin, "text/html; charset=windows-1252"), "ça");
        assert_eq!(decode_body(&latin, "text/html"), "ça");
        assert_eq!(
            decode_body("ação".as_bytes(), "text/html; charset=utf-8"),
            "ação"
        );
    }

    #[test]
    fn cache_keeps_the_newest_entry_and_evicts_fifo() {
        let mut cache = HttpCache::default();
        let entry = |body: &str| CacheEntry {
            body: body.as_bytes().to_vec(),
            content_type: "text/plain".to_string(),
            stored_at: Instant::now(),
            max_age: Some(Duration::from_secs(60)),
            etag: None,
            last_modified: None,
            headers: Vec::new(),
        };
        cache.put("a", entry("1"));
        cache.put("b", entry("2"));
        assert!(cache.get("a").is_some());
        // Overwriting an entry keeps one copy and the right byte count.
        cache.put("a", entry("11"));
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.bytes, 3);
        assert_eq!(cache.refresh("a").map(|e| e.body.len()), Some(2));
    }

    #[test]
    fn cookies_are_scoped_by_domain_path_and_secure() {
        let mut jar = CookieJar::default();
        jar.store("https://a.com/x/y", "id=7; Path=/x; Secure");
        assert_eq!(jar.header_for("https://a.com/x/z").as_deref(), Some("id=7"));
        assert_eq!(jar.header_for("https://a.com/other"), None);
        // Not sent over plain HTTP because of `Secure`.
        assert_eq!(jar.header_for("http://a.com/x/z"), None);

        jar.store("https://a.com/", "sess=1; HttpOnly");
        // `HttpOnly` is invisible to script but still sent on the wire.
        assert_eq!(jar.visible("https://a.com/"), "");
        assert_eq!(
            jar.header_for("https://a.com/").as_deref(),
            Some("sess=1")
        );

        // A domain cookie reaches subdomains; the host-only ones above do not.
        jar.store("https://a.com/", "wide=2; Domain=.a.com");
        assert_eq!(jar.header_for("https://sub.a.com/").as_deref(), Some("wide=2"));
        assert_eq!(jar.header_for("https://b.com/"), None);
        // `id=7` is host-only: a subdomain sees `wide` but never `id`.
        assert_eq!(
            jar.header_for("https://sub.a.com/x/z").as_deref(),
            Some("wide=2")
        );

        // `Max-Age=0` removes it.
        jar.store("https://a.com/", "wide=; Max-Age=0; Domain=.a.com");
        assert_eq!(jar.header_for("https://sub.a.com/"), None);
    }

    #[test]
    fn http_dates_convert_to_unix_seconds() {
        // 2024-01-01T00:00:00Z
        assert_eq!(
            parse_http_date("Mon, 01 Jan 2024 00:00:00 GMT"),
            Some(1_704_067_200)
        );
        assert_eq!(parse_http_date("lixo"), None);
    }

    #[test]
    fn origins_compare_by_scheme_host_and_port() {
        let a = Origin::of("https://a.com/x").unwrap();
        assert_eq!(a.serialize(), "https://a.com");
        assert!(a.same_origin(&Origin::of("https://a.com/y?q=1").unwrap()));
        assert!(!a.same_origin(&Origin::of("http://a.com/").unwrap()), "scheme counts");
        assert!(!a.same_origin(&Origin::of("https://b.com/").unwrap()));
        assert!(!a.same_origin(&Origin::of("https://a.com:8443/").unwrap()), "port counts");
        // `file://` is never same-origin with anything, itself included.
        let local = Origin::of("file:///C:/x.html").unwrap();
        assert!(local.is_local());
        assert!(!local.same_origin(&local));
    }

    #[test]
    fn cors_allows_star_and_exact_origin_only() {
        let page = Origin::of("https://a.com/").unwrap();
        assert!(page.allowed_by("*"));
        assert!(page.allowed_by("https://a.com"));
        assert!(!page.allowed_by("https://b.com"));
        assert!(!page.allowed_by(""));
        let local = Origin::of("file:///C:/x.html").unwrap();
        assert!(!local.allowed_by("*"), "a local origin is opaque");
    }

    #[test]
    fn parses_cache_control_max_age() {
        assert_eq!(
            parse_max_age("public, max-age=60"),
            Some(Duration::from_secs(60))
        );
        assert_eq!(parse_max_age("no-cache"), None);
    }
}
