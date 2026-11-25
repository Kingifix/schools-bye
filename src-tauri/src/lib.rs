mod parse;

use parse::{Attachment, DayDetails, NewMark, Quarter, Week};
use reqwest::header::{COOKIE, LOCATION};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Emitter, Manager};
use tauri_plugin_store::StoreExt;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:154.0) Gecko/20100101 Firefox/154.0";
const SESSION_EXPIRED: &str = "SESSION_EXPIRED";
/// Pause between sequential requests while crawling weeks (parallel requests get blocked).
const CRAWL_DELAY: Duration = Duration::from_millis(1);
const MAX_WEEKS_PER_QUARTER: usize = 16;

// REST API of the official mobile app (themes, notes, timestamps the site doesn't show).
const API_BASE: &str = "https://api.schools.by";
const API_CLIENT_ID: &str = "0a6d97ffe21e6a9a9d9b7317456af1a92a6d6dbb59a02b24db2ad6add1381849";
const API_DIGEST_KEY: &[u8] = b"01K04AW1FYZ34GB2RKWH8ZANA1";
const API_UA: &str = "Dart/3.11 (dart:io)";
const NO_API_TOKEN: &str = "NO_API_TOKEN";

// --- STATE ---

#[derive(Clone)]
struct Session {
    cookie: String,
    pupil_id: String,
    domain: String,
    /// Token for api.schools.by (None for sessions saved before 0.4 or if the API login failed).
    api_token: Option<String>,
}

impl Session {
    fn csrf(&self) -> Option<&str> {
        self.cookie
            .split(';')
            .map(str::trim)
            .find_map(|c| c.strip_prefix("csrftoken="))
    }
}

// --- REST API ---

/// HMAC-SHA256 over `{"url_path":…,"parameters":{sorted, stringified}}` (same as the mobile app).
fn api_digest(path: &str, params: &BTreeMap<String, String>) -> String {
    use hmac::{Hmac, Mac};
    let msg = format!(
        "{{\"url_path\":{},\"parameters\":{}}}",
        serde_json::to_string(path).unwrap_or_default(),
        serde_json::to_string(params).unwrap_or_default()
    );
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(API_DIGEST_KEY).expect("hmac key");
    mac.update(msg.as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

async fn api_request(
    client: &reqwest::Client,
    path: &str,
    token: Option<&str>,
    body: Option<BTreeMap<String, String>>,
) -> Result<serde_json::Value, String> {
    let params = body.clone().unwrap_or_default();
    let url = format!("{API_BASE}{path}");
    let mut req = match &body {
        Some(b) => client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_string(b).unwrap_or_default()),
        None => client.get(&url),
    };
    req = req
        .header(reqwest::header::USER_AGENT, API_UA)
        .header("Client-ID", API_CLIENT_ID)
        .header("x-digest", api_digest(path, &params))
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(t) = token {
        req = req.header(reqwest::header::AUTHORIZATION, format!("Token {t}"));
    }
    let resp = req.send().await.map_err(|e| format!("NETWORK: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(NO_API_TOKEN.into());
    }
    if !status.is_success() {
        return Err(format!("API HTTP {status}"));
    }
    let text = resp.text().await.map_err(|e| format!("NETWORK: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("API JSON: {e}"))
}

async fn api_login(client: &reqwest::Client, username: &str, password: &str) -> Result<String, String> {
    let body = BTreeMap::from([
        ("username".to_string(), username.to_string()),
        ("password".to_string(), password.to_string()),
    ]);
    let r = api_request(client, "/v3/api/auth", None, Some(body)).await?;
    r.get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| "API: no token".into())
}

/// Marks of one already-finished week, persisted so the Final tab doesn't refetch it.
#[derive(Serialize, Deserialize, Clone)]
struct CachedWeek {
    prev: Option<String>,
    next: Option<String>,
    marks: Vec<(String, Vec<u8>)>,
    /// Lessons per subject this week.
    #[serde(default)]
    lessons: Vec<(String, u8)>,
}

/// Bumped when CachedWeek gains data old entries lack (forces a one-time refetch).
const WEEKS_KEY: &str = "weeks_v2";

struct AppState {
    client: reqwest::Client,
    session: Mutex<Option<Session>>,
    quarters: Mutex<Option<Vec<Quarter>>>,
    weeks: Mutex<Option<HashMap<String, CachedWeek>>>,
    crawl_lock: tokio::sync::Mutex<()>,
}

impl AppState {
    fn session(&self) -> Result<Session, String> {
        self.session.lock().unwrap().clone().ok_or_else(|| SESSION_EXPIRED.to_string())
    }

    fn reset(&self) {
        *self.session.lock().unwrap() = None;
        *self.quarters.lock().unwrap() = None;
        *self.weeks.lock().unwrap() = None;
    }

    /// GET a page of the school site with the session cookie.
    async fn get(&self, url: &str) -> Result<String, String> {
        self.fetch(url, false).await
    }

    /// GET like the site's own AJAX calls (some fragments are only served this way).
    async fn get_xhr(&self, url: &str) -> Result<String, String> {
        self.fetch(url, true).await
    }

    async fn fetch(&self, url: &str, xhr: bool) -> Result<String, String> {
        let s = self.session()?;
        let mut url = url.to_string();
        for _ in 0..4 {
            let mut req = self.client.get(&url).header(COOKIE, &s.cookie);
            if xhr {
                req = req
                    .header("X-Requested-With", "XMLHttpRequest")
                    .header(reqwest::header::ACCEPT, "text/html, */*; q=0.01")
                    .header(reqwest::header::REFERER, format!("{}/pupil/{}", s.domain, s.pupil_id));
                if let Some(csrf) = s.csrf() {
                    req = req.header("X-CSRFToken", csrf);
                }
            }
            let resp = req.send().await.map_err(|e| format!("NETWORK: {e}"))?;
            let status = resp.status();
            if status.is_redirection() {
                let loc = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("");
                if loc.is_empty() || loc.contains("login") {
                    return Err(SESSION_EXPIRED.into());
                }
                url = if loc.starts_with("http") { loc.to_string() } else { format!("{}{}", s.domain, loc) };
                continue;
            }
            if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                return Err(SESSION_EXPIRED.into());
            }
            if !status.is_success() {
                return Err(format!("HTTP {status}"));
            }
            return resp.text().await.map_err(|e| format!("NETWORK: {e}"));
        }
        Err("Too many redirects".into())
    }
}

fn set_cookies(resp: &reqwest::Response) -> Vec<String> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| c.split(';').next())
        .map(|c| c.trim().to_string())
        .collect()
}

fn cookie_value(cookies: &[String], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    cookies.iter().find_map(|c| c.strip_prefix(&prefix).map(str::to_string))
}

// --- AUTH ---

#[derive(Serialize)]
struct LoginResult {
    pupil_id: String,
    school_subdomain: String,
}

#[tauri::command]
async fn login(
    username: String,
    password: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<LoginResult, String> {
    // The site (fast HTML diary) and the REST API (lesson details) log in at the same time:
    // they are different hosts, so this doesn't trigger the site's rate limiting.
    let (site, api) = tokio::join!(
        site_login(&state.client, username.trim(), &password),
        api_login(&state.client, username.trim(), &password)
    );
    let (cookie, pupil_id, domain) = site?;
    let session = Session { cookie, pupil_id: pupil_id.clone(), domain: domain.clone(), api_token: api.ok() };

    let store = app.store("session.json").map_err(|e| e.to_string())?;
    store.set("cookie", serde_json::json!(session.cookie));
    store.set("pupil_id", serde_json::json!(session.pupil_id));
    store.set("school_domain", serde_json::json!(session.domain));
    store.set("api_token", serde_json::json!(session.api_token));
    store.save().map_err(|e| e.to_string())?;

    state.reset();
    *state.session.lock().unwrap() = Some(session);
    Ok(LoginResult { pupil_id, school_subdomain: domain })
}

/// Logs in to schools.by and returns (cookie header, pupil id, school domain).
async fn site_login(
    client: &reqwest::Client,
    username: &str,
    password: &str,
) -> Result<(String, String, String), String> {
    let net = |e: reqwest::Error| format!("NETWORK: {e}");

    let home = client.get("https://schools.by/").send().await.map_err(net)?;
    let phpsessid = cookie_value(&set_cookies(&home), "PHPSESSID");

    let mut cookie = phpsessid.map(|p| format!("PHPSESSID={p}")).unwrap_or_default();
    let mut req = client.get("https://schools.by/login");
    if !cookie.is_empty() {
        req = req.header(COOKIE, &cookie);
    }
    let page = req.send().await.map_err(net)?;
    let csrf_cookie = cookie_value(&set_cookies(&page), "csrftoken").ok_or("csrftoken not found")?;
    let html = page.text().await.map_err(net)?;
    let csrf_token = regex::Regex::new(r#"name=["']csrfmiddlewaretoken["']\s+value=["']([^"']+)"#)
        .unwrap()
        .captures(&html)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| csrf_cookie.clone());

    if !cookie.is_empty() {
        cookie.push_str("; ");
    }
    cookie.push_str(&format!("csrftoken={csrf_cookie}"));

    let resp = client
        .post("https://schools.by/login")
        .header("Referer", "https://schools.by/login")
        .header("Origin", "https://schools.by")
        .header(COOKIE, &cookie)
        .form(&[
            ("csrfmiddlewaretoken", csrf_token.as_str()),
            ("username", username),
            ("password", password),
            ("|123", "|123"),
        ])
        .send()
        .await
        .map_err(net)?;

    if resp.status() != reqwest::StatusCode::FOUND {
        return Err("BAD_CREDENTIALS".into());
    }
    let location = resp
        .headers()
        .get(LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or("No redirect location")?
        .to_string();
    let post = set_cookies(&resp);
    let session_id = cookie_value(&post, "sessionid").ok_or("BAD_CREDENTIALS")?;
    let csrf = cookie_value(&post, "csrftoken").unwrap_or(csrf_cookie);

    let url = url::Url::parse(&location).map_err(|e| e.to_string())?;
    let host = url.host_str().ok_or("No host in redirect")?.to_string();
    let pupil_id = url
        .path_segments()
        .and_then(|mut s| {
            s.next();
            s.next()
        })
        .filter(|s| !s.is_empty())
        .ok_or("Could not parse pupil_id")?
        .to_string();

    Ok((format!("csrftoken={csrf}; sessionid={session_id}"), pupil_id, format!("https://{host}")))
}

#[tauri::command]
async fn restore_session(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let store = app.store("session.json").map_err(|e| e.to_string())?;
    let get = |k: &str| store.get(k).and_then(|v| v.as_str().map(str::to_string));
    match (get("cookie"), get("pupil_id"), get("school_domain")) {
        (Some(cookie), Some(pupil_id), Some(domain)) => {
            let api_token = get("api_token");
            *state.session.lock().unwrap() = Some(Session { cookie, pupil_id, domain, api_token });
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[tauri::command]
async fn logout(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.reset();
    for name in ["session.json", "weeks.json"] {
        let store = app.store(name).map_err(|e| e.to_string())?;
        store.clear();
        store.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

// --- SCHEDULE ---

#[derive(Serialize)]
struct ScheduleResponse {
    #[serde(flatten)]
    week: Week,
    /// Week id of the returned page (computed for the landing page).
    week_id: Option<String>,
    quarter_id: Option<String>,
}

fn landing_week_id(week: &Week) -> Option<String> {
    week.prev_week_id
        .as_deref()
        .and_then(|p| parse::shift_week(p, 7))
        .or_else(|| week.next_week_id.as_deref().and_then(|n| parse::shift_week(n, -7)))
}

fn is_finished_week(week_id: &str) -> bool {
    parse::week_day_number(week_id).is_some_and(|d| d + 7 <= parse::today())
}

fn weeks_cache<'a>(
    app: &tauri::AppHandle,
    guard: &'a mut Option<HashMap<String, CachedWeek>>,
) -> &'a mut HashMap<String, CachedWeek> {
    guard.get_or_insert_with(|| {
        let store = app.store("weeks.json").ok();
        if let Some(st) = &store {
            st.delete("weeks"); // pre-v2 cache
        }
        store
            .and_then(|s| s.get(WEEKS_KEY))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    })
}

fn persist_weeks(app: &tauri::AppHandle, state: &AppState) {
    let guard = state.weeks.lock().unwrap();
    if let (Some(map), Ok(store)) = (guard.as_ref(), app.store("weeks.json")) {
        store.set(WEEKS_KEY, serde_json::to_value(map).unwrap_or_default());
        let _ = store.save();
    }
}

/// Caches a finished week; returns true if something was stored.
fn remember_week(app: &tauri::AppHandle, state: &AppState, quarter_id: &str, week_id: &str, week: &Week) -> bool {
    if !is_finished_week(week_id) {
        return false;
    }
    let mut guard = state.weeks.lock().unwrap();
    weeks_cache(app, &mut guard).insert(
        format!("{quarter_id}/{week_id}"),
        CachedWeek {
            prev: week.prev_week_id.clone(),
            next: week.next_week_id.clone(),
            marks: parse::week_marks(week),
            lessons: parse::week_lessons(week),
        },
    );
    true
}

#[tauri::command]
async fn fetch_schedule(
    week_id: Option<String>,
    quarter_id: Option<String>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ScheduleResponse, String> {
    let s = state.session()?;
    let base = format!("{}/pupil/{}/dnevnik", s.domain, s.pupil_id);
    let url = match (&quarter_id, &week_id) {
        (Some(q), Some(w)) => format!("{base}/quarter/{q}/week/{w}"),
        (Some(q), None) => format!("{base}/quarter/{q}"),
        // No quarter chosen yet: the diary root shows the current week *and* the quarter tabs.
        (None, _) => base.clone(),
    };
    let html = state.get(&url).await?;

    let (mut week, quarters) = {
        let week = parse::parse_week(&html, &s.domain);
        let quarters = if quarter_id.is_none() { parse::parse_quarters(&html) } else { Vec::new() };
        (week, quarters)
    };
    if !quarters.is_empty() {
        *state.quarters.lock().unwrap() = Some(quarters.clone());
    }
    // The diary root may not contain the week itself: fall back to the current quarter page.
    if quarter_id.is_none() && week.days.is_empty() {
        if let Some(q) = quarters.iter().find(|q| q.is_current).or(quarters.last()) {
            let html = state.get(&format!("{base}/quarter/{}", q.id)).await?;
            week = parse::parse_week(&html, &s.domain);
        }
    }
    let quarter_id = quarter_id.or_else(|| {
        let q = state.quarters.lock().unwrap();
        q.as_ref().and_then(|qs| qs.iter().find(|q| q.is_current).or(qs.last()).map(|q| q.id.clone()))
    });
    let week_id = week_id.or_else(|| landing_week_id(&week));
    if let (Some(q), Some(w)) = (&quarter_id, &week_id) {
        if remember_week(&app, &state, q, w, &week) {
            persist_weeks(&app, &state);
        }
    }
    Ok(ScheduleResponse { week, week_id, quarter_id })
}

#[tauri::command]
async fn fetch_quarters(state: tauri::State<'_, AppState>) -> Result<Vec<Quarter>, String> {
    if let Some(q) = state.quarters.lock().unwrap().clone() {
        return Ok(q);
    }
    let s = state.session()?;
    let html = state.get(&format!("{}/pupil/{}/dnevnik", s.domain, s.pupil_id)).await?;
    let quarters = parse::parse_quarters(&html);
    *state.quarters.lock().unwrap() = Some(quarters.clone());
    Ok(quarters)
}

#[tauri::command]
async fn fetch_attachments(url: String, state: tauri::State<'_, AppState>) -> Result<Vec<Attachment>, String> {
    let s = state.session()?;
    let full = if url.starts_with("http") { url } else { format!("{}{}", s.domain, url) };
    let html = state.get(&full).await?;
    Ok(parse::parse_attachments(&html, &s.domain))
}

// --- LESSON DETAILS (REST API) ---

#[derive(Serialize)]
struct DetailsResponse {
    monday: String,
    days: Vec<DayDetails>,
}

/// Themes, teacher notes (e.g. "Тематический контроль: КР") and homework timestamps for one week.
/// Goes to api.schools.by, so the frontend runs it in parallel with `fetch_schedule`.
#[tauri::command]
async fn fetch_lesson_details(
    week_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<DetailsResponse, String> {
    let s = state.session()?;
    let token = s.api_token.clone().ok_or(NO_API_TOKEN)?;
    let monday = week_id
        .filter(|w| parse::week_day_number(w).is_some())
        .unwrap_or_else(parse::current_monday);
    let path = format!("/v3/subdomain-api/pupil/{}/daybook/week/{monday}", s.pupil_id);
    let json = api_request(&state.client, &path, Some(&token), None).await?;
    Ok(DetailsResponse { days: parse::parse_api_week(&json), monday })
}

// --- NEW MARKS ---

/// The site's "Новые отметки" popup. Called after the week has loaded (never in parallel with it).
#[tauri::command]
async fn fetch_new_marks(state: tauri::State<'_, AppState>) -> Result<Vec<NewMark>, String> {
    let s = state.session()?;
    let html = state.get_xhr(&format!("{}/pupil/{}/info-today-marks", s.domain, s.pupil_id)).await?;
    Ok(parse::parse_new_marks(&html))
}

// --- FILE DOWNLOAD + "OPEN WITH" ---

#[cfg(target_os = "android")]
struct FileViewer(tauri::plugin::PluginHandle<tauri::Wry>);

#[cfg(target_os = "android")]
fn file_viewer_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("fileviewer")
        .setup(|app, api| {
            let handle = api.register_android_plugin("com.koder.schoolsbye", "FileViewerPlugin")?;
            app.manage(FileViewer(handle));
            Ok(())
        })
        .build()
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "file".into() } else { cleaned.chars().take(120).collect() }
}

/// filename from Content-Disposition (RFC 5987 `filename*=` preferred).
fn disposition_name(value: &str) -> Option<String> {
    for part in value.split(';').map(str::trim) {
        if let Some(v) = part.strip_prefix("filename*=") {
            let v = v.splitn(3, '\'').last().unwrap_or(v);
            if let Ok(decoded) = url::form_urlencoded::parse(format!("x={}", v.replace('+', "%2B")).as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .ok_or(())
            {
                return Some(decoded);
            }
        }
    }
    value
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("filename="))
        .map(|v| v.trim_matches('"').to_string())
}

#[derive(Serialize, Deserialize)]
struct OpenFileArgs {
    path: String,
    mime: Option<String>,
}

/// Downloads an attachment into the app cache (once) and opens it with the system chooser.
#[tauri::command]
async fn open_attachment(
    url: String,
    name: Option<String>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use std::hash::{Hash, Hasher};
    let s = state.session()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("attachments")
        .join(format!("{:016x}", hasher.finish()));

    // Reuse an already downloaded copy.
    let existing = std::fs::read_dir(&dir)
        .ok()
        .and_then(|mut it| it.find_map(|e| e.ok().map(|e| e.path())))
        .filter(|p| p.metadata().map(|m| m.len() > 0).unwrap_or(false));

    let (path, mime) = match existing {
        Some(p) => (p, None),
        None => {
            let domain_host = url::Url::parse(&s.domain).ok().and_then(|u| u.host_str().map(str::to_string));
            let mut current = if url.starts_with("http") { url.clone() } else { format!("{}{}", s.domain, url) };
            let mut resp = None;
            for _ in 0..6 {
                let same_site = url::Url::parse(&current)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .is_some_and(|h| Some(&h) == domain_host.as_ref() || h.ends_with("schools.by"));
                let mut req = state.client.get(&current).timeout(Duration::from_secs(120));
                if same_site {
                    req = req.header(COOKIE, &s.cookie);
                }
                let r = req.send().await.map_err(|e| format!("NETWORK: {e}"))?;
                if r.status().is_redirection() {
                    let loc = r.headers().get(LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    if loc.contains("/login") {
                        return Err(SESSION_EXPIRED.into());
                    }
                    current = url::Url::parse(&current)
                        .and_then(|base| base.join(&loc))
                        .map(|u| u.to_string())
                        .map_err(|e| e.to_string())?;
                    continue;
                }
                resp = Some(r);
                break;
            }
            let resp = resp.ok_or("Too many redirects")?;
            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status()));
            }
            let mime = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.split(';').next().unwrap_or(v).trim().to_string());
            let header_name = resp
                .headers()
                .get(reqwest::header::CONTENT_DISPOSITION)
                .and_then(|v| v.to_str().ok())
                .and_then(disposition_name);
            let url_name = url::Url::parse(&current)
                .ok()
                .and_then(|u| u.path_segments().and_then(|mut s| s.next_back().map(str::to_string)))
                .filter(|n| n.contains('.'));
            let file_name = sanitize_file_name(&header_name.or(name).or(url_name).unwrap_or_else(|| "file".into()));
            let bytes = resp.bytes().await.map_err(|e| format!("NETWORK: {e}"))?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(file_name);
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
            (path, mime)
        }
    };

    #[cfg(target_os = "android")]
    {
        let viewer = app.state::<FileViewer>();
        viewer
            .0
            .run_mobile_plugin::<()>("open", OpenFileArgs { path: path.to_string_lossy().into_owned(), mime })
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = mime;
        use tauri_plugin_opener::OpenerExt;
        app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// --- SELF-UPDATE (GitHub releases) ---

const RELEASES_API: &str = "https://api.github.com/repos/Kingifix/schools-bye/releases/latest";

#[derive(Serialize)]
struct UpdateInfo {
    version: String,
    notes: String,
    url: String,
    size: u64,
}

/// "v0.5.2" / "0.5.2" -> (0, 5, 2)
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

/// Newer release than the installed app, if any.
#[tauri::command]
async fn check_update(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<Option<UpdateInfo>, String> {
    let resp = state
        .client
        .get(RELEASES_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("NETWORK: {e}"))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let text = resp.text().await.map_err(|e| format!("NETWORK: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or_default();
    let current = app.package_info().version.to_string();
    match (parse_version(tag), parse_version(&current)) {
        (Some(latest), Some(cur)) if latest > cur => {}
        _ => return Ok(None),
    }
    let Some(asset) = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str().is_some_and(|n| n.ends_with(".apk"))))
    else {
        return Ok(None);
    };
    Ok(Some(UpdateInfo {
        version: tag.trim_start_matches('v').to_string(),
        notes: v["body"].as_str().unwrap_or_default().to_string(),
        url: asset["browser_download_url"].as_str().unwrap_or_default().to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
    }))
}

/// Downloads the APK (reporting progress) and opens the system installer.
#[tauri::command]
async fn install_update(url: String, app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    if !url.starts_with("https://github.com/") {
        return Err("bad update url".into());
    }
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("update");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("schools.bye.apk");

    // GitHub answers with a redirect to its CDN; the shared client doesn't follow redirects.
    let mut current = url;
    let mut resp = None;
    for _ in 0..5 {
        let r = state
            .client
            .get(&current)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| format!("NETWORK: {e}"))?;
        if r.status().is_redirection() {
            current = r.headers().get(LOCATION).and_then(|v| v.to_str().ok()).ok_or("bad redirect")?.to_string();
            continue;
        }
        resp = Some(r);
        break;
    }
    let mut resp = resp.ok_or("Too many redirects")?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut bytes = Vec::with_capacity(total as usize);
    let mut last = 0u64;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("NETWORK: {e}"))? {
        bytes.extend_from_slice(&chunk);
        let pct = if total > 0 { bytes.len() as u64 * 100 / total } else { 0 };
        if pct != last {
            last = pct;
            let _ = app.emit("update-progress", pct);
        }
    }
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;

    #[cfg(target_os = "android")]
    {
        app.state::<FileViewer>()
            .0
            .run_mobile_plugin::<()>("install", OpenFileArgs { path: path.to_string_lossy().into_owned(), mime: None })
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

// --- FINAL ---

#[derive(Serialize, Clone, Default)]
struct FinalCell {
    /// Displayed value: official mark, or computed average ("8.43").
    value: Option<String>,
    /// Numeric value used for colouring / the year average.
    num: Option<f32>,
    /// true = not set by a teacher yet (our average, or schools.by's prediction).
    computed: bool,
    /// true = schools.by's own predicted quarter mark (grey on the site).
    predicted: bool,
    /// How many marks the computed average is based on.
    count: usize,
    marks: Vec<u8>,
}

#[derive(Serialize)]
struct FinalRow {
    subject: String,
    /// Full subject name, if the site gives one.
    full: Option<String>,
    /// Graded per half-year: halves are in columns 2 and 4, columns 1 and 3 stay empty.
    half: bool,
    quarters: Vec<FinalCell>,
    year: FinalCell,
}

#[derive(Serialize)]
struct SubjectMarks {
    subject: String,
    marks: Vec<u8>,
    /// Half-year subjects: marks of the previous quarter of the same half.
    prev: Vec<u8>,
    half: bool,
    avg: f32,
}

/// All marks of the current quarter, per subject.
#[derive(Serialize)]
struct CurrentQuarter {
    index: usize,
    label: String,
    subjects: Vec<SubjectMarks>,
}

#[derive(Serialize)]
struct FinalReport {
    quarter_labels: Vec<String>,
    rows: Vec<FinalRow>,
    current: Option<CurrentQuarter>,
}

#[derive(Serialize, Clone)]
struct Progress {
    quarter: usize,
    weeks: usize,
    label: String,
}

fn official_cell(c: &parse::OfficialCell) -> FinalCell {
    let nums = parse::mark_numbers(&c.value);
    let num = (!nums.is_empty()).then(|| nums.iter().map(|&n| n as f32).sum::<f32>() / nums.len() as f32);
    FinalCell { value: Some(c.value.clone()), num, computed: c.predicted, predicted: c.predicted, ..Default::default() }
}

fn is_set(c: &FinalCell) -> bool {
    c.value.is_some() && !c.computed
}

fn computed_cell(marks: &[u8]) -> FinalCell {
    if marks.is_empty() {
        return FinalCell::default();
    }
    let avg = marks.iter().map(|&m| m as f32).sum::<f32>() / marks.len() as f32;
    FinalCell {
        value: Some(format!("{avg:.2}")),
        num: Some(avg),
        computed: true,
        count: marks.len(),
        marks: marks.to_vec(),
        predicted: false,
    }
}

/// Marks and lesson counts collected from one quarter.
#[derive(Default, Clone)]
struct QuarterData {
    marks: Vec<(String, Vec<u8>)>,
    /// Lessons per subject, one entry per week.
    weekly: Vec<Vec<(String, u8)>>,
}

impl QuarterData {
    fn add(&mut self, marks: Vec<(String, Vec<u8>)>, lessons: Vec<(String, u8)>) {
        for (subj, m) in marks {
            match self.marks.iter_mut().find(|(t, _)| *t == subj) {
                Some((_, v)) => v.extend(m),
                None => self.marks.push((subj, m)),
            }
        }
        if !lessons.is_empty() {
            self.weekly.push(lessons);
        }
    }

    fn marks_of(&self, name: &str) -> Vec<u8> {
        self.marks
            .iter()
            .filter(|(s, _)| s == name || parse::same_subject(s, name))
            .flat_map(|(_, m)| m.iter().copied())
            .collect()
    }
}

/// Typical number of lessons per week (most frequent count; ties -> the larger one).
fn lessons_per_week(data: &[Option<QuarterData>], names: &[&str]) -> Option<u8> {
    let mut freq = [0usize; 16];
    for week in data.iter().flatten().flat_map(|q| &q.weekly) {
        let n: usize = week
            .iter()
            .filter(|(s, _)| names.iter().any(|n| s == n || parse::same_subject(s, n)))
            .map(|(_, c)| *c as usize)
            .sum();
        if n > 0 {
            freq[n.min(15)] += 1;
        }
    }
    (1..16).filter(|&i| freq[i] > 0).max_by_key(|&i| (freq[i], i)).map(|i| i as u8)
}

/// Walks a quarter week by week (sequentially) and collects all marks and lesson counts.
async fn crawl_quarter(
    app: &tauri::AppHandle,
    state: &AppState,
    s: &Session,
    index: usize,
    quarter_id: &str,
) -> Result<QuarterData, String> {
    let base = format!("{}/pupil/{}/dnevnik/quarter/{}", s.domain, s.pupil_id, quarter_id);
    let mut data = QuarterData::default();

    let landing_html = state.get(&base).await?;
    let landing = parse::parse_week(&landing_html, &s.domain);
    let landing_id = landing_week_id(&landing);
    let mut visited: Vec<String> = landing_id.iter().cloned().collect();
    data.add(parse::week_marks(&landing), parse::week_lessons(&landing));
    let mut fetched = 1;
    let _ = app.emit("final-progress", Progress { quarter: index, weeks: fetched, label: landing.current_week_label.clone() });

    // Go back to the first week, then forward up to today.
    for (start, backwards) in [(landing.prev_week_id.clone(), true), (landing.next_week_id.clone(), false)] {
        let mut cursor = start;
        while let Some(week_id) = cursor.take() {
            if visited.contains(&week_id) || visited.len() >= MAX_WEEKS_PER_QUARTER {
                break;
            }
            if parse::week_day_number(&week_id).is_some_and(|d| d > parse::today()) {
                break;
            }
            visited.push(week_id.clone());
            let key = format!("{quarter_id}/{week_id}");
            let cached = {
                let mut guard = state.weeks.lock().unwrap();
                weeks_cache(app, &mut guard).get(&key).cloned()
            };
            let week = match cached {
                Some(c) => c,
                None => {
                    tokio::time::sleep(CRAWL_DELAY).await;
                    let html = state.get(&format!("{base}/week/{week_id}")).await?;
                    let week = parse::parse_week(&html, &s.domain);
                    remember_week(app, state, quarter_id, &week_id, &week);
                    fetched += 1;
                    let _ = app.emit(
                        "final-progress",
                        Progress { quarter: index, weeks: fetched, label: week.current_week_label.clone() },
                    );
                    CachedWeek {
                        prev: week.prev_week_id.clone(),
                        next: week.next_week_id.clone(),
                        marks: parse::week_marks(&week),
                        lessons: parse::week_lessons(&week),
                    }
                }
            };
            data.add(week.marks, week.lessons);
            cursor = if backwards { week.prev } else { week.next };
        }
    }
    persist_weeks(app, state);
    Ok(data)
}

fn row_matches(r: &FinalRow, subject: &str) -> bool {
    r.subject == subject
        || parse::same_subject(&r.subject, subject)
        || r.full.as_deref().is_some_and(|f| parse::same_subject(f, subject))
}

fn row_names(r: &FinalRow) -> Vec<&str> {
    std::iter::once(r.subject.as_str()).chain(r.full.as_deref()).collect()
}

/// Fills a cell that the teacher hasn't set yet from the collected marks.
fn fill_cell(cell: &mut FinalCell, marks: Vec<u8>) {
    if is_set(cell) || marks.is_empty() {
        return;
    }
    if cell.predicted {
        // Keep schools.by's prediction, but attach the marks behind it.
        cell.count = marks.len();
        cell.marks = marks;
    } else {
        *cell = computed_cell(&marks);
    }
}

/// A subject is graded per half-year if it has one lesson a week. A teacher-set mark in
/// quarter 1 or 3 proves otherwise.
fn is_half_year(row: &FinalRow, data: &[Option<QuarterData>]) -> bool {
    if [0, 2].iter().any(|&i| is_set(&row.quarters[i])) {
        return false;
    }
    lessons_per_week(data, &row_names(row)).is_some_and(|n| n <= 1)
}

#[tauri::command]
async fn fetch_final(
    force: Option<bool>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<FinalReport, String> {
    let _guard = state.crawl_lock.lock().await;
    let s = state.session()?;
    if force.unwrap_or(false) {
        // Full recheck: drop cached quarters and weeks, fetch everything again.
        *state.quarters.lock().unwrap() = None;
        *state.weeks.lock().unwrap() = Some(HashMap::new());
    }

    let quarters = {
        let cached = state.quarters.lock().unwrap().clone();
        match cached {
            Some(q) if !q.is_empty() => q,
            _ => {
                let html = state.get(&format!("{}/pupil/{}/dnevnik", s.domain, s.pupil_id)).await?;
                let q = parse::parse_quarters(&html);
                *state.quarters.lock().unwrap() = Some(q.clone());
                q
            }
        }
    };
    let quarters: Vec<Quarter> = quarters.into_iter().take(4).collect();

    // 1. Official marks (quarter + year) from the diary's last page.
    let official = match state.get_xhr(&format!("{}/pupil/{}/dnevnik/last-page", s.domain, s.pupil_id)).await {
        Ok(html) => parse::parse_official(&html),
        Err(e) if e == SESSION_EXPIRED => return Err(e),
        Err(_) => Default::default(),
    };

    let mut rows: Vec<FinalRow> = official
        .rows
        .iter()
        .map(|r| FinalRow {
            subject: r.subject.clone(),
            full: r.full.clone(),
            half: false,
            quarters: r.quarters.iter().map(|c| c.as_ref().map(official_cell).unwrap_or_default()).collect(),
            year: r.year.as_ref().map(official_cell).unwrap_or_default(),
        })
        .collect();

    // 2. Crawl the weeks. The current quarter always (its marks are listed separately and
    //    tell which subjects are weekly-once); the others when something there isn't set yet.
    let current = quarters.iter().position(|q| q.is_current);
    let started = |i: usize| current.is_none_or(|c| i <= c);
    let mut data: Vec<Option<QuarterData>> = vec![None; 4];
    if let Some(ci) = current {
        data[ci] = Some(crawl_quarter(&app, &state, &s, ci, &quarters[ci].id).await?);
    }
    for (i, q) in quarters.iter().enumerate() {
        if data[i].is_some() || !started(i) {
            continue;
        }
        let quarter_set = rows.iter().any(|r| is_set(&r.quarters[i]));
        // Half-year subjects need both quarters of the half while its mark isn't set yet.
        let pair = i ^ 1;
        let half_open = current == Some(pair)
            || rows.iter().any(|r| !is_set(&r.quarters[i]) && !is_set(&r.quarters[pair]) && is_half_year(r, &data));
        if quarter_set && !half_open {
            continue;
        }
        data[i] = Some(crawl_quarter(&app, &state, &s, i, &q.id).await?);
    }

    // Subjects found only in the weeks (no row on the last page yet).
    let week_subjects: Vec<String> =
        data.iter().flatten().flat_map(|d| d.marks.iter().map(|(s, _)| s.clone())).collect();
    for subject in week_subjects {
        if !rows.iter().any(|r| row_matches(r, &subject)) {
            rows.push(FinalRow {
                subject,
                full: None,
                half: false,
                quarters: vec![FinalCell::default(); 4],
                year: FinalCell::default(),
            });
        }
    }

    // 3. Fill cells the teacher hasn't set from our averages.
    for row in &mut rows {
        let names: Vec<String> = row_names(row).into_iter().map(str::to_string).collect();
        let marks_in = |i: usize| -> Vec<u8> {
            data[i]
                .as_ref()
                .map(|d| {
                    // Prefer the short name (matches the weeks); fall back to the full one.
                    let m = d.marks_of(&names[0]);
                    if m.is_empty() && names.len() > 1 { d.marks_of(&names[1]) } else { m }
                })
                .unwrap_or_default()
        };
        row.half = is_half_year(row, &data);
        if row.half {
            // Halves live in columns 2 and 4; the site may put the mark into either column.
            for h in 0..2 {
                let (a, b) = (2 * h, 2 * h + 1);
                if !is_set(&row.quarters[b]) && (is_set(&row.quarters[a]) || row.quarters[b].value.is_none()) {
                    row.quarters[b] = std::mem::take(&mut row.quarters[a]);
                }
                row.quarters[a] = FinalCell::default();
                let mut marks = marks_in(a);
                marks.extend(marks_in(b));
                fill_cell(&mut row.quarters[b], marks);
            }
        } else {
            for i in 0..4 {
                fill_cell(&mut row.quarters[i], marks_in(i));
            }
        }
    }

    // 4. Year: official if present, otherwise the mean of the known quarter/half values.
    for row in &mut rows {
        if row.year.value.is_none() {
            let nums: Vec<f32> = row.quarters.iter().filter_map(|c| c.num).collect();
            if !nums.is_empty() {
                let avg = nums.iter().sum::<f32>() / nums.len() as f32;
                row.year = FinalCell { value: Some(format!("{avg:.2}")), num: Some(avg), computed: true, ..Default::default() };
            }
        }
    }
    rows.retain(|r| r.quarters.iter().any(|c| c.value.is_some()) || r.year.value.is_some());
    rows.sort_by(|a, b| a.subject.to_lowercase().cmp(&b.subject.to_lowercase()));

    // 5. Current quarter: every mark per subject; half-year subjects also get the marks of
    //    the other quarter of the half.
    let current_quarter = current.map(|ci| {
        let cur = data[ci].clone().unwrap_or_default();
        let prev_data = if ci % 2 == 1 { data[ci - 1].as_ref() } else { None };
        let mut names: Vec<String> = cur.marks.iter().map(|(s, _)| s.clone()).collect();
        // Half-year subjects with marks only in the previous quarter so far.
        for (s, _) in cur.weekly.iter().flatten() {
            let has_prev = prev_data.is_some_and(|d| !d.marks_of(s).is_empty());
            if has_prev && !names.iter().any(|n| n == s || parse::same_subject(n, s)) {
                names.push(s.clone());
            }
        }
        let mut subjects: Vec<SubjectMarks> = names
            .into_iter()
            .map(|subject| {
                let marks = cur.marks_of(&subject);
                let half = match rows.iter().find(|r| row_matches(r, &subject)) {
                    Some(r) => r.half,
                    None => lessons_per_week(&data, &[&subject]).is_some_and(|n| n <= 1),
                };
                let prev = match (half, prev_data) {
                    (true, Some(d)) => d.marks_of(&subject),
                    _ => Vec::new(),
                };
                let all: Vec<u8> = prev.iter().chain(&marks).copied().collect();
                let avg = if all.is_empty() { 0.0 } else { all.iter().map(|&m| m as f32).sum::<f32>() / all.len() as f32 };
                SubjectMarks { subject, marks, prev, half, avg }
            })
            .filter(|s| !s.marks.is_empty() || !s.prev.is_empty())
            .collect();
        subjects.sort_by(|a, b| a.subject.to_lowercase().cmp(&b.subject.to_lowercase()));
        CurrentQuarter { index: ci, label: quarters[ci].label.clone(), subjects }
    });

    let quarter_labels = (0..4)
        .map(|i| quarters.get(i).map(|q| q.label.clone()).unwrap_or_else(|| format!("{}", i + 1)))
        .collect();
    Ok(FinalReport { quarter_labels, rows, current: current_quarter })
}

// --- ENTRY POINT ---

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = reqwest::Client::builder()
        .user_agent(UA)
        .redirect(reqwest::redirect::Policy::none())
        .gzip(true)
        .brotli(true)
        .pool_idle_timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()
        .expect("http client");

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(file_viewer_plugin());

    builder
        .setup(move |app| {
            app.manage(AppState {
                client,
                session: Mutex::new(None),
                quarters: Mutex::new(None),
                weeks: Mutex::new(None),
                crawl_lock: tokio::sync::Mutex::new(()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            login,
            restore_session,
            logout,
            fetch_schedule,
            fetch_quarters,
            fetch_attachments,
            open_attachment,
            fetch_lesson_details,
            fetch_new_marks,
            check_update,
            install_update,
            app_version,
            fetch_final
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
