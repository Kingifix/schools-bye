//! HTML parsing for schools.by pages. Everything here is synchronous and
//! returns owned data, so `scraper::Html` (which is !Send) never crosses an
//! `.await` in the async commands.

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

macro_rules! sel {
    ($name:ident, $s:expr) => {
        static $name: LazyLock<Selector> = LazyLock::new(|| Selector::parse($s).unwrap());
    };
}

sel!(DAYS, "div.db_days.clearfix");
sel!(DAY_TABLE, "table.db_table");
sel!(ROW, "tbody tr");
sel!(DAY_HEADER, "thead th.lesson");
sel!(LESSON, "td.lesson span");
sel!(HOMEWORK, "td.ht .ht-text");
sel!(MARK, "td.mark .mark_box strong");
sel!(ATTACH, "a.attachments_dropdown_toggle");
sel!(PREV, "a.prev");
sel!(NEXT, "a.next");
sel!(PERIOD, "div.db_period p");
sel!(QUARTER_TAB, "ul.tabs2 li a[quarter_id]");
sel!(SPAN, "span");
sel!(FILE, ".attachments_file");
sel!(FILE_LINK, ".file_name a");
sel!(FILE_SIZE, ".file_size");
sel!(TABLE, "table");
sel!(TR, "tr");
sel!(LTABLE, "table.ltable");
sel!(MTABLE, "table.mtable");
sel!(BODY_TR, "tbody > tr");
sel!(LINK, "a");
sel!(QMARK, "td.qmark");
sel!(YMARK, "td.ymark");
sel!(TMARK, "td.tmark");
sel!(MARK_ITEM, ".mark-item");
sel!(MARK_SUBJECT, ".mark-subject");
sel!(MARK_DATE, ".mark-date");
sel!(MARK_VALUE, "[class*=mark-value]");
sel!(BOLD, "b, strong, .bold");

static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static MARK_NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(10|[1-9])\b").unwrap());
static ROMAN_Q: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[\s(])(iv|iii|ii|i)(?:[\s.)\-]|$)").unwrap());
static ARABIC_Q: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([1-4])\s*-?\s*(?:я|й)?\s*четв|четв\S*\s*([1-4])").unwrap());

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Lesson {
    pub number: u8,
    pub subject: String,
    pub homework: Option<String>,
    pub mark: Option<String>,
    pub attachments_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Day {
    pub date: String,
    pub day_of_week: String,
    pub lessons: Vec<Lesson>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Week {
    pub days: Vec<Day>,
    pub prev_week_id: Option<String>,
    pub next_week_id: Option<String>,
    pub current_week_label: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Quarter {
    pub id: String,
    pub label: String,
    pub is_current: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub download_url: String,
    pub size: Option<String>,
}

/// One official cell: the value and whether schools.by only *predicts* it (grey on the site).
#[derive(Debug, Clone, PartialEq)]
pub struct OfficialCell {
    pub value: String,
    pub predicted: bool,
}

#[derive(Debug, Clone, Default)]
pub struct OfficialRow {
    /// Name as shown in the diary ("Англ. яз.") — matches the week pages.
    pub subject: String,
    /// Full name from the link title ("Английский язык").
    pub full: Option<String>,
    pub quarters: [Option<OfficialCell>; 4],
    pub year: Option<OfficialCell>,
}

/// Official marks from the "Четвертные и годовые отметки" page.
#[derive(Debug, Clone, Default)]
pub struct OfficialMarks {
    pub rows: Vec<OfficialRow>,
}

/// A mark from the "Новые отметки" popup.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct NewMark {
    pub subject: String,
    pub date: String,
    pub value: String,
}

/// Extra lesson info from the REST API (theme, teacher note, homework…).
#[derive(Serialize, Debug, Clone, Default)]
pub struct DetailSection {
    /// "theme" | "note" | "hometask" | other API key
    pub key: String,
    /// note type, e.g. "Тематический контроль"
    pub kind: Option<String>,
    pub text: String,
    pub created: Option<String>,
    pub updated: Option<String>,
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct LessonDetails {
    pub number: String,
    pub subject: Option<String>,
    pub mark: Option<String>,
    pub sections: Vec<DetailSection>,
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct DayDetails {
    /// ISO date, "2026-10-12"
    pub date: String,
    pub lessons: Vec<LessonDetails>,
}

pub fn clean(s: &str) -> String {
    WS.replace_all(s.trim(), " ").into_owned()
}

fn text(e: ElementRef) -> String {
    clean(&e.text().collect::<String>())
}

fn absolutize(url: &str, domain: &str) -> String {
    if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{}{}", domain, url)
    }
}

/// All numeric marks contained in a mark string ("8", "7/9", "н" -> []).
pub fn mark_numbers(mark: &str) -> Vec<u8> {
    MARK_NUM
        .captures_iter(mark)
        .filter_map(|c| c[1].parse().ok())
        .collect()
}

// --- week / diary ---

pub fn parse_week(html: &str, domain: &str) -> Week {
    let doc = Html::parse_document(html);
    let mut days = Vec::new();
    for container in doc.select(&DAYS) {
        if container
            .value()
            .attr("style")
            .is_some_and(|s| s.replace(' ', "").contains("display:none"))
        {
            continue;
        }
        for table in container.select(&DAY_TABLE) {
            days.push(parse_day(table, domain));
        }
    }

    let prev_week_id = doc
        .select(&PREV)
        .next()
        .and_then(|e| e.value().attr("prev_week_id"))
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let next_week_id = doc
        .select(&NEXT)
        .next()
        .and_then(|e| e.value().attr("next_week_id"))
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let current_week_label = doc.select(&PERIOD).next().map(text).unwrap_or_default();

    Week { days, prev_week_id, next_week_id, current_week_label }
}

fn parse_day(table: ElementRef, domain: &str) -> Day {
    let header = table.select(&DAY_HEADER).next().map(text).unwrap_or_default();
    let mut parts = header.splitn(2, ',');
    let day_of_week = parts.next().unwrap_or("").trim().to_string();
    let date = parts.next().unwrap_or("").trim().to_string();

    let mut lessons = Vec::new();
    for row in table.select(&ROW) {
        let full = row.select(&LESSON).next().map(text).unwrap_or_default();
        let (number, subject) = match full.split_once('.') {
            Some((n, s)) if n.trim().chars().all(|c| c.is_ascii_digit()) && !n.trim().is_empty() => {
                (n.trim().parse().unwrap_or(0), s.trim().to_string())
            }
            _ => (0, full),
        };
        if subject.is_empty() {
            continue;
        }
        let homework = row.select(&HOMEWORK).next().map(text).filter(|t| !t.is_empty());
        let mark = row.select(&MARK).next().map(text).filter(|t| !t.is_empty());
        let attachments_url = row
            .select(&ATTACH)
            .next()
            .and_then(|e| e.value().attr("href"))
            .map(|h| absolutize(h, domain));
        lessons.push(Lesson { number, subject, homework, mark, attachments_url });
    }
    Day { date, day_of_week, lessons }
}

/// subject -> numeric marks for one week.
pub fn week_marks(week: &Week) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    for lesson in week.days.iter().flat_map(|d| &d.lessons) {
        let Some(mark) = &lesson.mark else { continue };
        let nums = mark_numbers(mark);
        if nums.is_empty() {
            continue;
        }
        match out.iter_mut().find(|(s, _)| *s == lesson.subject) {
            Some((_, v)) => v.extend(nums),
            None => out.push((lesson.subject.clone(), nums)),
        }
    }
    out
}

/// subject -> number of lessons in one week (to tell weekly-once "half-year" subjects).
pub fn week_lessons(week: &Week) -> Vec<(String, u8)> {
    let mut out: Vec<(String, u8)> = Vec::new();
    for lesson in week.days.iter().flat_map(|d| &d.lessons) {
        match out.iter_mut().find(|(s, _)| *s == lesson.subject) {
            Some((_, n)) => *n = n.saturating_add(1),
            None => out.push((lesson.subject.clone(), 1)),
        }
    }
    out
}

// --- quarters ---

pub fn parse_quarters(html: &str) -> Vec<Quarter> {
    let doc = Html::parse_document(html);
    let mut out: Vec<Quarter> = Vec::new();
    for a in doc.select(&QUARTER_TAB) {
        let id = a.value().attr("quarter_id").unwrap_or("").to_string();
        if id.is_empty() || out.iter().any(|q| q.id == id) {
            continue;
        }
        let label = a.select(&SPAN).next().map(text).unwrap_or_else(|| text(a));
        let is_current = a.value().attr("class").is_some_and(|c| c.contains("current"))
            || a.parent()
                .and_then(ElementRef::wrap)
                .and_then(|li| li.value().attr("class"))
                .is_some_and(|c| c.contains("current"));
        out.push(Quarter { id, label, is_current });
    }
    out
}

// --- attachments ---

pub fn parse_attachments(html: &str, domain: &str) -> Vec<Attachment> {
    let doc = Html::parse_document(html);
    doc.select(&FILE)
        .filter_map(|file| {
            let link = file.select(&FILE_LINK).next()?;
            let href = link.value().attr("href")?;
            Some(Attachment {
                name: text(link),
                download_url: absolutize(href, domain),
                size: file.select(&FILE_SIZE).next().map(text).filter(|s| !s.is_empty()),
            })
        })
        .collect()
}

// --- official final marks ---

/// Expands a table's rows into a grid honouring colspan/rowspan.
fn grid(table: ElementRef) -> Vec<Vec<(String, bool)>> {
    let mut rows: Vec<Vec<(String, bool)>> = Vec::new();
    // pending rowspans: column -> (remaining rows, text, is_th)
    let mut carry: Vec<Option<(usize, String, bool)>> = Vec::new();
    for tr in table.select(&TR) {
        // skip rows of nested tables
        if tr.ancestors().filter_map(ElementRef::wrap).find(|e| e.value().name() == "table")
            != Some(table)
        {
            continue;
        }
        let mut row: Vec<(String, bool)> = Vec::new();
        let mut cells = tr.children().filter_map(ElementRef::wrap).filter(|c| {
            let n = c.value().name();
            n == "td" || n == "th"
        });
        let mut col = 0;
        loop {
            if let Some(Some((left, t, th))) = carry.get_mut(col) {
                row.push((t.clone(), *th));
                *left -= 1;
                if *left == 0 {
                    carry[col] = None;
                }
                col += 1;
                continue;
            }
            let Some(cell) = cells.next() else { break };
            let span: usize = cell.value().attr("colspan").and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 20);
            let rspan: usize = cell.value().attr("rowspan").and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 50);
            let is_th = cell.value().name() == "th";
            let t = text(cell);
            for _ in 0..span {
                if carry.len() <= col {
                    carry.resize(col + 1, None);
                }
                if rspan > 1 {
                    carry[col] = Some((rspan - 1, t.clone(), is_th));
                }
                row.push((t.clone(), is_th));
                col += 1;
            }
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }
    rows
}

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Subject,
    Quarter(usize),
    Year,
    Other,
}

fn classify(label: &str) -> Col {
    let l = label.to_lowercase().replace('і', "i").replace('І', "i");
    if l.contains("предмет") || l.contains("прадмет") {
        return Col::Subject;
    }
    let bare = l.trim().trim_end_matches('.');
    if let Ok(n @ 1..=4) = bare.parse::<usize>() {
        return Col::Quarter(n - 1);
    }
    if let Some(c) = ARABIC_Q.captures(&l) {
        let n: usize = c.get(1).or(c.get(2)).unwrap().as_str().parse().unwrap();
        return Col::Quarter(n - 1);
    }
    if let Some(c) = ROMAN_Q.captures(&l) {
        return Col::Quarter(match &c[1] {
            "i" => 0,
            "ii" => 1,
            "iii" => 2,
            _ => 3,
        });
    }
    if l.contains("год") || l.contains("итог") || l.contains("выніков") {
        return Col::Year;
    }
    Col::Other
}

fn mark_value(v: &str) -> Option<String> {
    let v = clean(v);
    (!v.is_empty() && !matches!(v.as_str(), "-" | "—" | "–" | "·")).then_some(v)
}

/// schools.by shows predicted quarter marks in grey and teacher-set ones in bold.
fn official_cell(td: ElementRef) -> Option<OfficialCell> {
    let value = mark_value(&td.text().collect::<String>())?;
    let html = td.html().to_lowercase();
    let bold = td.select(&BOLD).next().is_some() || html.contains("font-weight");
    let grey = ["#888", "#999", "#aaa", "gray", "grey", "forecast", "predict", "progn"]
        .iter()
        .any(|m| html.contains(m));
    Some(OfficialCell { value, predicted: grey && !bold })
}

/// The real last page: subject names in `table.ltable`, marks in `table.mtable`,
/// rows aligned by index.
fn parse_last_page(doc: &Html) -> Option<OfficialMarks> {
    let ltable = doc.select(&LTABLE).next()?;
    let mtable = doc.select(&MTABLE).next()?;
    let names: Vec<Option<(String, Option<String>)>> = ltable
        .select(&BODY_TR)
        .map(|tr| {
            tr.select(&LINK).next().map(|a| {
                let short = text(a);
                let title = a.value().attr("title").map(clean).filter(|t| !t.is_empty());
                if short.is_empty() {
                    (title.unwrap_or_default(), None)
                } else {
                    (short, title)
                }
            })
        })
        .collect();
    let mut rows = Vec::new();
    for (tr, name) in mtable.select(&BODY_TR).zip(names) {
        let Some((subject, full)) = name else { continue };
        if subject.is_empty() {
            continue;
        }
        let mut quarters: [Option<OfficialCell>; 4] = Default::default();
        for (i, td) in tr.select(&QMARK).take(4).enumerate() {
            quarters[i] = official_cell(td);
        }
        let year = tr
            .select(&YMARK)
            .next()
            .and_then(official_cell)
            .or_else(|| tr.select(&TMARK).next().and_then(official_cell));
        rows.push(OfficialRow { subject, full, quarters, year });
    }
    (!rows.is_empty()).then_some(OfficialMarks { rows })
}

pub fn parse_official(html: &str) -> OfficialMarks {
    let doc = Html::parse_document(html);
    if let Some(marks) = parse_last_page(&doc) {
        return marks;
    }
    let mut best = OfficialMarks::default();
    for table in doc.select(&TABLE) {
        let g = grid(table);
        // header rows: leading rows made only of <th> (at least one row)
        let n_head = g.iter().take_while(|r| r.iter().all(|(_, th)| *th)).count().max(1);
        if g.len() <= n_head {
            continue;
        }
        let width = g.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut cols = vec![Col::Other; width];
        for (i, c) in cols.iter_mut().enumerate() {
            let label: Vec<&str> =
                g[..n_head].iter().filter_map(|r| r.get(i).map(|(t, _)| t.as_str())).collect();
            *c = classify(&label.join(" "));
        }
        let quarter_cols = cols.iter().filter(|c| matches!(c, Col::Quarter(_))).count();
        if quarter_cols == 0 {
            continue;
        }
        // No explicit "Предмет" header: the first non-quarter column holds subjects.
        let subj_col = match cols.iter().position(|c| *c == Col::Subject) {
            Some(i) => i,
            None if quarter_cols >= 2 => match cols.iter().position(|c| *c == Col::Other) {
                Some(i) => i,
                None => continue,
            },
            None => continue,
        };
        let mut rows = Vec::new();
        for r in &g[n_head..] {
            let Some((subject, _)) = r.get(subj_col) else { continue };
            let subject = subject.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
            if subject.is_empty() {
                continue;
            }
            let mut quarters: [Option<OfficialCell>; 4] = Default::default();
            let mut year = None;
            for (i, c) in cols.iter().enumerate() {
                let v = r
                    .get(i)
                    .and_then(|(t, _)| mark_value(t))
                    .map(|value| OfficialCell { value, predicted: false });
                match c {
                    Col::Quarter(n) if quarters[*n].is_none() => quarters[*n] = v,
                    Col::Year if year.is_none() => year = v,
                    _ => {}
                }
            }
            rows.push(OfficialRow { subject: subject.to_string(), full: None, quarters, year });
        }
        if rows.len() > best.rows.len() {
            best.rows = rows;
        }
    }
    best
}

// --- "новые отметки" popup ---

pub fn parse_new_marks(html: &str) -> Vec<NewMark> {
    let doc = Html::parse_document(html);
    doc.select(&MARK_ITEM)
        .filter_map(|item| {
            let value = item.select(&MARK_VALUE).next().map(text).filter(|v| !v.is_empty())?;
            Some(NewMark {
                subject: item.select(&MARK_SUBJECT).next().map(text).unwrap_or_default(),
                date: item.select(&MARK_DATE).next().map(text).unwrap_or_default(),
                value,
            })
        })
        .collect()
}

// --- REST API week (api.schools.by) ---

use serde_json::Value;

/// A human readable string from an API value: strings, numbers, or objects with a text-ish key.
fn str_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(clean(s)).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(o) => ["text", "name", "title", "value", "label", "mark", "short"]
            .iter()
            .find_map(|k| o.get(*k).and_then(str_of)),
        _ => None,
    }
}

/// Multi-line text keeps its line breaks.
fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().replace("\r\n", "\n")).filter(|s| !s.is_empty()),
        Value::Object(o) => o.get("text").and_then(text_of),
        _ => None,
    }
}

fn first_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| v.get(*k).and_then(str_of))
}

const CREATED: &[&str] = &["created", "created_at", "date_created", "create_date", "created_date"];
const UPDATED: &[&str] = &["updated", "updated_at", "modified", "date_updated", "update_date", "changed"];
const MAIN_SECTIONS: [&str; 3] = ["theme", "note", "hometask"];

fn section(key: &str, v: &Value) -> Option<DetailSection> {
    let text = text_of(v).unwrap_or_default();
    let kind = match v {
        Value::Object(_) => first_str(v, &["type", "type_name", "kind", "note_type", "category"]),
        _ => None,
    };
    if text.is_empty() && kind.is_none() {
        return None;
    }
    Some(DetailSection {
        key: key.to_string(),
        kind,
        text,
        created: first_str(v, CREATED),
        updated: first_str(v, UPDATED),
    })
}

fn api_lesson(number: &str, les: &Value) -> LessonDetails {
    let ld = les.get("lesson_data").unwrap_or(&Value::Null);
    let mut sections: Vec<DetailSection> =
        MAIN_SECTIONS.iter().filter_map(|k| ld.get(*k).and_then(|v| section(k, v))).collect();
    // Other objects with text in them (remarks, comments, …). Plain fields like
    // "date", "number" or "cabinet" are skipped.
    if let Some(o) = ld.as_object() {
        for (k, v) in o {
            if MAIN_SECTIONS.contains(&k.as_str()) || k.ends_with("id") {
                continue;
            }
            if matches!(v, Value::Object(_)) {
                sections.extend(section(k, v));
            }
        }
    }
    // When the mark was set / changed, plus the teacher's comment on it.
    if let Some(md) = les.get("mark_data").filter(|v| v.is_object()) {
        let created = first_str(md, CREATED);
        let updated = first_str(md, UPDATED);
        let text = md.get("note").and_then(text_of).unwrap_or_default();
        if created.is_some() || updated.is_some() || !text.is_empty() {
            sections.push(DetailSection { key: "mark".into(), kind: None, text, created, updated });
        }
    }
    for k in ["remark", "remarks", "comment", "notice"] {
        if let Some(v) = les.get(k) {
            sections.extend(section(k, v));
        }
    }
    LessonDetails {
        number: number.to_string(),
        subject: first_str(les, &["subject_short", "subject_name", "subject"]),
        mark: les.get("mark").and_then(str_of),
        sections,
    }
}

/// Parses `/v3/subdomain-api/pupil/{id}/daybook/week/{monday}`:
/// `{ "YYYY-MM-DD": { "lessons": { "1": {...} } } }`.
pub fn parse_api_week(v: &Value) -> Vec<DayDetails> {
    let Some(days) = v.as_object() else { return Vec::new() };
    let mut out: Vec<DayDetails> = days
        .iter()
        .filter(|(date, _)| week_day_number(date).is_some())
        .map(|(date, day)| {
            let mut lessons: Vec<LessonDetails> = day
                .get("lessons")
                .and_then(Value::as_object)
                .map(|ls| ls.iter().map(|(n, les)| api_lesson(n, les)).collect())
                .unwrap_or_default();
            lessons.sort_by_key(|l| l.number.parse::<i64>().unwrap_or(i64::MAX));
            DayDetails { date: date.clone(), lessons }
        })
        .collect();
    out.sort_by(|a, b| a.date.cmp(&b.date));
    out
}

/// Monday of the current (Minsk) week as "YYYY-MM-DD".
pub fn current_monday() -> String {
    let d = today();
    let (y, m, dd) = civil_from_days(d - (d + 3).rem_euclid(7));
    format!("{y:04}-{m:02}-{dd:02}")
}

// --- subject name matching ---

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// "Бел. лит." matches "Белорусская литература"; exact matches too.
pub fn same_subject(a: &str, b: &str) -> bool {
    let (ta, tb) = (tokens(a), tokens(b));
    if ta.is_empty() || ta.len() != tb.len() {
        return false;
    }
    ta.iter().zip(&tb).all(|(x, y)| x.starts_with(y.as_str()) || y.starts_with(x.as_str()))
}

// --- dates (week ids are YYYY-MM-DD Mondays) ---

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn week_day_number(id: &str) -> Option<i64> {
    let mut it = id.split('-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

pub fn shift_week(id: &str, days: i64) -> Option<String> {
    let (y, m, d) = civil_from_days(week_day_number(id)? + days);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// Today as a day number, Minsk time (UTC+3).
pub fn today() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (secs + 3 * 3600).div_euclid(86400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks() {
        assert_eq!(mark_numbers("7/9"), vec![7, 9]);
        assert_eq!(mark_numbers("10"), vec![10]);
        assert!(mark_numbers("н").is_empty());
    }

    #[test]
    fn subjects() {
        assert!(same_subject("Бел. лит.", "Белорусская литература"));
        assert!(same_subject("Информ.", "Информатика"));
        assert!(!same_subject("Бел. лит.", "Белорусский язык"));
    }

    #[test]
    fn dates() {
        assert_eq!(shift_week("2025-12-29", 7).as_deref(), Some("2026-01-05"));
        assert_eq!(shift_week("2026-03-02", -7).as_deref(), Some("2026-02-23"));
    }

    #[test]
    fn classify_cols() {
        assert!(classify("I четверть") == Col::Quarter(0));
        assert!(classify("IV") == Col::Quarter(3));
        assert!(classify("2 четверть") == Col::Quarter(1));
        assert!(classify("Годовая") == Col::Year);
        assert!(classify("Предмет") == Col::Subject);
        assert!(classify("3") == Col::Quarter(2));
        assert!(classify("III") == Col::Quarter(2));
    }

    #[test]
    fn official_table() {
        let html = r#"<table><thead><tr><th rowspan=2>Предмет</th><th colspan=4>Четверти</th><th rowspan=2>Год</th></tr>
            <tr><th>I</th><th>II</th><th>III</th><th>IV</th></tr></thead>
            <tbody><tr><td>Алгебра</td><td>8</td><td>9</td><td></td><td></td><td></td></tr></tbody></table>"#;
        let o = parse_official(html);
        assert_eq!(o.rows.len(), 1);
        assert_eq!(o.rows[0].subject, "Алгебра");
        assert_eq!(o.rows[0].quarters[1].as_ref().map(|c| c.value.as_str()), Some("9"));
        assert!(o.rows[0].quarters[2].is_none());

        let html2 = r#"<table><tr><th></th><th>1</th><th>2</th><th>3</th><th>4</th><th>Годовая</th></tr>
            <tr><td>Химия</td><td>7</td><td>8</td><td></td><td></td><td></td></tr></table>"#;
        let o = parse_official(html2);
        assert_eq!(o.rows.len(), 1);
        assert_eq!(o.rows[0].subject, "Химия");
        assert_eq!(o.rows[0].quarters[0].as_ref().map(|c| c.value.as_str()), Some("7"));
    }

    #[test]
    fn last_page() {
        let html = r##"<div class="wrap_lmtables"><table class="itable ltable"><thead><tr><td>&nbsp;</td></tr></thead><tbody>
            <tr class="bg"><td class="ttl"><p>1. <a href="/journal/1" title="Английский язык"> Англ. яз. </a></p></td></tr>
            <tr><td class="ttl"><p>2. <a href="/journal/2" title="Белорусский язык">Бел. яз.</a></p></td></tr>
            <tr><td class="ttl"><p>3. <a href="/journal/3" title="Физическая культура">Физ. к.</a></p></td></tr>
            <tr><td class="ttl"><b>Средний балл</b></td></tr></tbody></table>
            <table class="itable mtable"><thead><tr><td class="qdates" colspan="4">Отметки</td><td class="avg" rowspan="2">Средняя</td><td class="ymark" rowspan="2">Годовая</td></tr>
            <tr><td class="qdates">I</td><td class="qdates bold">II / I</td><td class="qdates">III</td><td class="qdates">IV / II</td></tr></thead><tbody>
            <tr class="marks"><td class="qmark"> 9 </td><td class="qmark"><span style="color: #888;">8</span></td><td class="qmark"></td><td class="qmark"></td><td class="avg"></td><td class="ymark"></td><td class="emark"></td><td class="tmark"></td></tr>
            <tr class="marks"><td class="qmark"></td><td class="qmark"></td><td class="qmark"></td><td class="qmark"></td><td class="avg"></td><td class="ymark"></td></tr>
            <tr class="marks"><td class="qmark"> з. </td><td class="qmark"></td><td class="qmark"></td><td class="qmark"></td><td class="avg"></td><td class="ymark"></td></tr>
            <tr><td class="qmark">8.75</td><td class="qmark"></td><td class="qmark"></td><td class="qmark"></td><td>&mdash;</td><td class="ymark"></td></tr>
            </tbody></table></div>"##;
        let o = parse_official(html);
        assert_eq!(o.rows.len(), 3);
        assert_eq!(o.rows[0].subject, "Англ. яз.");
        assert_eq!(o.rows[0].full.as_deref(), Some("Английский язык"));
        assert_eq!(o.rows[0].quarters[0], Some(OfficialCell { value: "9".into(), predicted: false }));
        assert_eq!(o.rows[0].quarters[1], Some(OfficialCell { value: "8".into(), predicted: true }));
        assert!(o.rows[1].quarters.iter().all(Option::is_none));
        assert_eq!(o.rows[2].quarters[0].as_ref().map(|c| c.value.as_str()), Some("з."));
    }

    #[test]
    fn new_marks() {
        let html = r##"<div class="marks-list"><div class="scroll_new_marks">
            <div class="mark-item" style="background: #F3F5FF;"><div class="mark-info">
            <div class="mark-subject">История Беларуси</div><div class="mark-date"> 16 ноября 2026 г. </div></div>
            <div class="

            mark-value-green  mark_border_green_box

            "> 9 </div></div></div></div>"##;
        let m = parse_new_marks(html);
        assert_eq!(
            m,
            vec![NewMark { subject: "История Беларуси".into(), date: "16 ноября 2026 г.".into(), value: "9".into() }]
        );
        assert!(parse_new_marks(r#"<div class="not_marks_conteiner">Нет новых отметок</div>"#).is_empty());
    }

    #[test]
    fn api_week() {
        let v: Value = serde_json::from_str(
            r#"{
          "2026-10-14": {"lessons": {
            "10": {"subject_short": "Химия"},
            "3": {"subject_short": "Матем.", "mark": "8", "lesson_data": {
              "hometask": {"text": "НГ с. 21", "created": "2026-10-13T14:00:00", "updated": "2026-10-13T15:00:08", "attachments": []},
              "theme": {"text": "сечения"},
              "note": {"type": "Тематический контроль", "text": "кр"},
              "date": "2026-10-14", "number": 3, "cabinet": ""},
              "mark_data": {"id": 1, "m": "8", "note": "", "created": "2026-10-29T10:00:39", "updated": "2026-10-29T10:00:39"}}}},
          "2026-10-15": {"lessons": {}}
        }"#,
        )
        .unwrap();
        let d = parse_api_week(&v);
        assert_eq!(d.len(), 2);
        let l = &d[0].lessons[0];
        assert_eq!(l.number, "3");
        assert_eq!(l.mark.as_deref(), Some("8"));
        assert_eq!(l.sections.len(), 4);
        assert_eq!(l.sections[3].key, "mark");
        assert_eq!(l.sections[3].created.as_deref(), Some("2026-10-29T10:00:39"));
        assert_eq!(l.sections[1].kind.as_deref(), Some("Тематический контроль"));
        assert_eq!(l.sections[1].text, "кр");
        assert_eq!(l.sections[2].updated.as_deref(), Some("2026-10-13T15:00:08"));
        assert_eq!(l.sections[2].created.as_deref(), Some("2026-10-13T14:00:00"));
    }

    #[test]
    fn monday() {
        let m = current_monday();
        let d = week_day_number(&m).unwrap();
        assert_eq!((d + 3).rem_euclid(7), 0);
        assert!(today() - d < 7);
    }
}
