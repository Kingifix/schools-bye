import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import fileIcon from "./assets/icons/file-attachment.svg";

// ---------- types ----------

interface Lesson {
  number: number;
  subject: string;
  homework: string | null;
  mark: string | null;
  attachments_url: string | null;
}

interface Day {
  date: string;
  day_of_week: string;
  lessons: Lesson[];
}

interface ScheduleResponse {
  days: Day[];
  prev_week_id: string | null;
  next_week_id: string | null;
  current_week_label: string;
  week_id: string | null;
  quarter_id: string | null;
}

interface Quarter {
  id: string;
  label: string;
  is_current: boolean;
}

interface Attachment {
  name: string;
  download_url: string;
  size: string | null;
}

interface DetailSection {
  key: string;
  kind: string | null;
  text: string;
  created: string | null;
  updated: string | null;
}

interface LessonDetails {
  number: string;
  subject: string | null;
  mark: string | null;
  sections: DetailSection[];
}

interface DetailsResponse {
  monday: string;
  days: { date: string; lessons: LessonDetails[] }[];
}

interface NewMark {
  subject: string;
  date: string;
  value: string;
}

interface FinalCell {
  value: string | null;
  num: number | null;
  computed: boolean;
  predicted: boolean;
  count: number;
  marks: number[];
}

interface FinalRow {
  subject: string;
  full: string | null;
  /** Per half of the year: graded once (mark in column 2 / 4, column 1 / 3 empty). */
  halves?: [boolean, boolean];
  /** Older cached reports. */
  half?: boolean;
  quarters: FinalCell[];
  year: FinalCell;
}

interface SubjectMarks {
  subject: string;
  marks: number[];
  /** Half-year subjects: marks of the previous quarter of the same half. */
  prev: number[];
  half: boolean;
  avg: number;
}

interface FinalReport {
  quarter_labels: string[];
  rows: FinalRow[];
  current: { index: number; label: string; subjects: SubjectMarks[] } | null;
}

type View = { kind: "chapter"; index: number } | { kind: "final" };

// ---------- helpers ----------

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

const store = {
  get<T>(key: string): T | null {
    try {
      const v = localStorage.getItem(key);
      return v ? (JSON.parse(v) as T) : null;
    } catch {
      return null;
    }
  },
  set(key: string, value: unknown) {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch {
      /* storage unavailable */
    }
  },
  clear() {
    try {
      localStorage.clear();
    } catch {
      /* ignore */
    }
  },
};

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

type MarkColor = "red" | "yellow" | "green";
const MARK_COLORS: MarkColor[] = ["red", "yellow", "green"];
/** Default: red ≤7, yellow 8, green 9–10. Overridable in settings. */
const defaultColor = (m: number): MarkColor => (m <= 7 ? "red" : m === 8 ? "yellow" : "green");
let markColors: Record<string, MarkColor> = store.get<Record<string, MarkColor>>("mark-colors") ?? {};
const colorOf = (m: number): MarkColor => markColors[m] ?? defaultColor(m);

/** Averages are rounded first. */
function markLevel(value: number | null): string {
  if (value === null || Number.isNaN(value)) return "mark-neutral";
  const r = Math.min(10, Math.max(1, Math.round(value)));
  return `mark-${colorOf(r)}`;
}

function markNumbers(mark: string): number[] {
  return (mark.match(/\b(10|[1-9])\b/g) ?? []).map(Number);
}

function levelOfMark(mark: string): string {
  const nums = markNumbers(mark);
  if (nums.length === 0) return "mark-neutral";
  return markLevel(nums.reduce((a, b) => a + b, 0) / nums.length);
}

let toastTimer: number | undefined;
function toast(msg: string) {
  const t = $("toast");
  t.textContent = msg;
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => t.classList.remove("show"), 2600);
}

function setBusy(busy: boolean) {
  $("progress-bar").classList.toggle("active", busy);
  $("reload-pill").classList.toggle("spinning", busy);
}

/** Shared error handling: expired session -> login screen. */
function handleError(e: unknown) {
  const msg = String(e);
  console.error(msg);
  if (msg.includes("SESSION_EXPIRED")) {
    showLogin("Сессия истекла, войдите снова");
  } else if (msg.includes("NETWORK")) {
    toast("Нет связи с schools.by");
  } else {
    toast("Ошибка: " + msg.slice(0, 80));
  }
}

// ---------- state ----------

let quarters: Quarter[] = store.get<Quarter[]>("quarters") ?? [];
let view: View = { kind: "chapter", index: -1 };
let currentQuarterId: string | null = null;
let currentWeekId: string | null = null;
let currentIsLanding = true;
let requestSeq = 0;
const weekCache = new Map<string, ScheduleResponse>();
const cacheKey = (q: string | null, w: string | null) => `${q ?? "-"}|${w ?? "landing"}`;

function chapterLabel(i: number) {
  return `${i + 1} четверть`;
}

// ---------- header menu ----------

function renderMenu() {
  const menu = $("view-menu");
  const items: string[] = [];
  for (let i = 0; i < 4; i++) {
    const q = quarters[i];
    const active = view.kind === "chapter" && view.index === i;
    items.push(
      `<button class="menu-item${active ? " active" : ""}" data-index="${i}" ${q ? "" : "disabled"}>
        <span>${chapterLabel(i)}</span>${q?.is_current ? '<span class="menu-dot"></span>' : ""}
      </button>`,
    );
  }
  items.push(
    `<button class="menu-item${view.kind === "final" ? " active" : ""}" data-final="1"><span>Итоги</span></button>`,
  );
  menu.innerHTML = items.join("");
  $("view-label").textContent = view.kind === "final" ? "Итоги" : view.index >= 0 ? chapterLabel(view.index) : "Четверть";
}

function toggleMenu(open?: boolean) {
  const isOpen = open ?? !document.body.classList.contains("menu-open");
  document.body.classList.toggle("menu-open", isOpen);
  $("title-btn").setAttribute("aria-expanded", String(isOpen));
}

$("title-btn").addEventListener("click", () => toggleMenu());
$("menu-backdrop").addEventListener("click", () => toggleMenu(false));
$("view-menu").addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLButtonElement>(".menu-item");
  if (!btn || btn.disabled) return;
  toggleMenu(false);
  if (btn.dataset.final) {
    showFinal();
  } else {
    const i = Number(btn.dataset.index);
    const q = quarters[i];
    if (q) openChapter(i, q.id);
  }
});

function setView(v: View) {
  view = v;
  const isFinal = v.kind === "final";
  $("schedule-view").hidden = isFinal;
  $("final-view").hidden = !isFinal;
  $("week-nav").classList.toggle("hidden", isFinal);
  if (isFinal) window.scrollTo({ top: 0, behavior: "instant" });
  renderMenu();
}

async function loadQuarters() {
  try {
    const q = await invoke<Quarter[]>("fetch_quarters");
    if (q.length) {
      quarters = q;
      store.set("quarters", q);
    }
  } catch (e) {
    handleError(e);
  }
  syncChapterFromQuarter();
}

function syncChapterFromQuarter() {
  if (view.kind !== "chapter") return;
  const idx = quarters.findIndex((q) => q.id === currentQuarterId);
  view = { kind: "chapter", index: idx >= 0 ? idx : quarters.findIndex((q) => q.is_current) };
  renderMenu();
}

// ---------- schedule ----------

function openChapter(index: number, quarterId: string) {
  setView({ kind: "chapter", index });
  fetchWeek(quarterId, null, { scrollToToday: true });
}

async function fetchWeek(
  quarterId: string | null,
  weekId: string | null,
  opts: { keepScroll?: boolean; scrollToToday?: boolean; force?: boolean } = {},
) {
  const seq = ++requestSeq;
  const key = cacheKey(quarterId, weekId);
  const cached = weekCache.get(key);
  const scrollBefore = window.scrollY;

  // Instant render from cache, then revalidate in the background.
  if (cached && !opts.force) {
    applyWeek(cached, weekId === null, opts.keepScroll ? scrollBefore : null, !!opts.scrollToToday);
  }

  // Lesson details come from api.schools.by — a different host, so it runs in parallel.
  const landing = weekId === null && quarterId === null;
  loadDetails(landing ? null : weekId, !!opts.force).then(() => {
    if (seq === requestSeq) markLessonsWithNotes();
  });

  setBusy(true);
  try {
    const res = await invoke<ScheduleResponse>("fetch_schedule", { weekId, quarterId });
    if (seq !== requestSeq) return;
    // Quarter landing pages: the week id is only known now.
    if (!landing && weekId === null && res.week_id) {
      loadDetails(res.week_id).then(() => {
        if (seq === requestSeq) markLessonsWithNotes();
      });
    }
    weekCache.set(key, res);
    if (res.week_id) weekCache.set(cacheKey(res.quarter_id, res.week_id), res);
    if (weekId === null && quarterId === null) store.set("last-week", res);

    const changed = !cached || JSON.stringify(cached) !== JSON.stringify(res);
    if (changed || opts.force) {
      const keep = cached || opts.keepScroll ? window.scrollY : null;
      applyWeek(res, weekId === null, keep, !cached && !!opts.scrollToToday);
    }
    // Quarters come from the same page on the backend, so this costs no extra request.
    if (quarterId === null) loadQuarters();
    markLessonsWithNotes();
    // Next request to the site only after the week is done: one at a time.
    await checkNewMarks(!!opts.force);
  } catch (e) {
    if (seq === requestSeq) handleError(e);
  } finally {
    if (seq === requestSeq) {
      setBusy(false);
      pendingFocus = null;
    }
  }
}

// ---------- lesson details (REST API) ----------

const details = new Map<string, Promise<DetailsResponse | null>>();
/** Same data, resolved: lets a re-render draw note badges synchronously (no layout jump). */
const detailsReady = new Map<string, DetailsResponse>();
let apiTokenMissing = false;

function loadDetails(weekId: string | null, force = false): Promise<DetailsResponse | null> {
  const key = weekId ?? "landing";
  const existing = details.get(key);
  if (existing && !force) return existing;
  const p = invoke<DetailsResponse>("fetch_lesson_details", { weekId })
    .then((r) => {
      apiTokenMissing = false;
      details.set(r.monday, p);
      detailsReady.set(key, r);
      detailsReady.set(r.monday, r);
      return r;
    })
    .catch((e) => {
      if (String(e).includes("NO_API_TOKEN")) apiTokenMissing = true;
      else console.error("details", e);
      if (details.get(key) === p) details.delete(key);
      return null;
    });
  details.set(key, p);
  return p;
}

/** Details for the week on screen (waits for an in-flight request). */
async function currentDetails(): Promise<DetailsResponse | null> {
  if (currentIsLanding && details.has("landing")) {
    const d = await details.get("landing")!;
    if (d && (!currentWeekId || d.monday === currentWeekId)) return d;
  }
  if (currentWeekId) return loadDetails(currentWeekId);
  return loadDetails(null);
}

function isoAddDays(iso: string, n: number): string {
  const [y, m, d] = iso.split("-").map(Number);
  const t = new Date(Date.UTC(y, m - 1, d + n));
  return t.toISOString().slice(0, 10);
}

function findLessonDetails(det: DetailsResponse, dayIndex: number, day: Day, lesson: Lesson): LessonDetails | null {
  const dom = Number(day.date.match(/\d{1,2}/)?.[0] ?? NaN);
  const byDate =
    det.days.find((d) => Number(d.date.slice(8, 10)) === dom) ??
    det.days.find((d) => d.date === isoAddDays(det.monday, dayIndex));
  if (!byDate) return null;
  const sameSubj = (l: LessonDetails) => !!l.subject && l.subject.toLowerCase() === lesson.subject.toLowerCase();
  return (
    byDate.lessons.find((l) => Number(l.number) === lesson.number && (!l.subject || sameSubj(l))) ??
    byDate.lessons.find((l) => Number(l.number) === lesson.number) ??
    byDate.lessons.find(sameSubj) ??
    null
  );
}

function fmtTime(s: string | null): string {
  if (!s) return "";
  const m = s.match(/^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2}))?/);
  if (!m) return s;
  return `${m[3]}.${m[2]}.${m[1]}${m[4] ? ` ${m[4]}:${m[5]}` : ""}`;
}

const SECTION_LABELS: Record<string, string> = {
  theme: "Тема урока",
  note: "Заметка учителя",
  hometask: "Домашнее задание",
  remark: "Замечание",
  remarks: "Замечание",
  comment: "Комментарий",
  notice: "Примечание",
  mark: "Отметка",
};

function sectionHtml(s: DetailSection): string {
  const label = SECTION_LABELS[s.key] ?? s.key;
  const isMark = s.key === "mark";
  const times = [
    s.created ? `${isMark ? "выставлена" : "создано"} ${fmtTime(s.created)}` : "",
    s.updated && s.updated !== s.created ? `${isMark ? "изменена" : "изменено"} ${fmtTime(s.updated)}` : "",
  ]
    .filter(Boolean)
    .join(" · ");
  return `<div class="det-section">
    <div class="det-label">${escapeHtml(label)}${s.kind ? ` <span class="det-kind">${escapeHtml(s.kind)}</span>` : ""}</div>
    ${s.text ? `<div class="det-text">${linkify(s.text)}</div>` : ""}
    ${times ? `<div class="det-time">${times}</div>` : ""}
  </div>`;
}

function linkify(text: string): string {
  return escapeHtml(text).replace(/(https?:\/\/[^\s<]+)/g, '<a href="$1" class="hw-link">$1</a>');
}

async function showLessonDetails(day: Day, dayIndex: number, lesson: Lesson) {
  const title = lesson.number ? `${lesson.number}. ${lesson.subject}` : lesson.subject;
  const head =
    `<div class="det-head"><div>` +
    `<p class="sheet-sub det-date">${escapeHtml(day.date ? `${day.date}, ${day.day_of_week}` : day.day_of_week)}</p>` +
    `<div class="sheet-title">${escapeHtml(title)}</div><div id="det-note"></div></div>` +
    (lesson.mark ? `<div class="lesson-mark ${levelOfMark(lesson.mark)}">${escapeHtml(lesson.mark)}</div>` : "") +
    `</div>`;
  const hwFallback = lesson.homework
    ? sectionHtml({ key: "hometask", kind: null, text: lesson.homework, created: null, updated: null })
    : "";
  openSheet(`${head}<div id="det-body"><p class="sheet-sub det-loading">Загружаю подробности…</p></div>`, undefined);

  const det = await currentDetails();
  const body = document.getElementById("det-body");
  if (!body || !document.body.classList.contains("sheet-open")) return;
  const info = det ? findLessonDetails(det, dayIndex, day, lesson) : null;
  let html = "";
  // Note type + text right under the title: "тематический контроль · кр".
  const note = info?.sections.find((s) => s.key === "note");
  const noteBox = document.getElementById("det-note");
  if (note && noteBox) noteBox.innerHTML = `<span class="det-note-chip">${escapeHtml(noteLabel(note))}</span>`;
  // A short note is fully shown in the chip; only long ones also get their own section.
  const noteInChip = !!note && note.text.length <= 40 && !note.created && !note.updated;
  if (info && info.sections.length) {
    const sections = info.sections.filter((s) => !(noteInChip && s === note));
    if (!sections.some((s) => s.key === "hometask") && hwFallback) html += hwFallback;
    html = sections.map(sectionHtml).join("") + html;
  } else {
    html = hwFallback;
    if (!det) {
      html += `<p class="sheet-sub">${
        apiTokenMissing
          ? "Чтобы видеть темы уроков и заметки, выйди и войди заново."
          : "Не удалось загрузить тему урока и заметки."
      }</p>`;
    } else {
      html += `<p class="sheet-sub">Учитель не добавил тему или заметку.</p>`;
    }
  }
  body.innerHTML = html;
}

function noteLabel(note: DetailSection): string {
  const short = note.text.length <= 40 ? note.text : "";
  return [note.kind, short].filter(Boolean).join(" · ") || "заметка";
}

/** Details of the week on screen if they are already loaded. */
function readyDetails(): DetailsResponse | null {
  if (currentIsLanding) {
    const d = detailsReady.get("landing");
    if (d && (!currentWeekId || d.monday === currentWeekId)) return d;
  }
  return currentWeekId ? (detailsReady.get(currentWeekId) ?? null) : null;
}

/** Badge under the subject for lessons with a teacher note (e.g. "Тематический контроль"). */
function applyNotes(det: DetailsResponse) {
  document.querySelectorAll<HTMLElement>(".lesson[data-day]").forEach((row) => {
    const entry = rowLessons.get(row);
    if (!entry || row.querySelector(".note-badge")) return;
    const info = findLessonDetails(det, entry.dayIndex, entry.day, entry.lesson);
    const note = info?.sections.find((s) => s.key === "note");
    if (!note) return;
    const badge = document.createElement("span");
    badge.className = "note-badge";
    badge.textContent = noteLabel(note);
    row.querySelector(".lesson-subject")?.after(badge);
  });
}

async function markLessonsWithNotes() {
  const det = await currentDetails();
  if (det) applyNotes(det);
}

const rowLessons = new WeakMap<HTMLElement, { day: Day; dayIndex: number; lesson: Lesson }>();

// ---------- new marks ----------

let newMarks: NewMark[] = [];
let lastMarksCheck = 0;

async function checkNewMarks(force = false) {
  if (!force && Date.now() - lastMarksCheck < 60_000) return;
  lastMarksCheck = Date.now();
  try {
    newMarks = await invoke<NewMark[]>("fetch_new_marks");
  } catch (e) {
    if (String(e).includes("SESSION_EXPIRED")) throw e;
    console.error("new marks", e);
    return;
  }
  $("marks-btn").classList.toggle("show", newMarks.length > 0);
}

$("marks-btn").addEventListener("click", () => {
  if (!newMarks.length) return;
  openSheet(
    `<div class="sheet-title">Новые отметки</div>` +
      newMarks
        .map(
          (m, i) => `<div class="new-mark" data-url="${i}">
            <div class="new-mark-info"><div class="new-mark-subj">${escapeHtml(m.subject)}</div>
            <div class="sheet-sub">${escapeHtml(m.date)}</div></div>
            <div class="lesson-mark ${levelOfMark(m.value)}">${escapeHtml(m.value)}</div>
          </div>`,
        )
        .join(""),
    (el) => {
      closeSheet();
      goToMark(newMarks[Number(el.dataset.url)]);
    },
  );
});

const MONTHS = ["янв", "фев", "мар", "апр", "ма", "июн", "июл", "авг", "сен", "окт", "ноя", "дек"];

/** "16 ноября 2026 г." / "16.11.2026" -> "2026-11-16" */
function parseRuDate(s: string): string | null {
  const num = s.match(/(\d{1,2})\.(\d{1,2})\.(\d{4})/);
  let d: number, m: number, y: number;
  if (num) {
    [d, m, y] = [Number(num[1]), Number(num[2]), Number(num[3])];
  } else {
    const w = s.toLowerCase().match(/(\d{1,2})\s+([а-яё]+)\s+(\d{4})/);
    if (!w) return null;
    // "мая" must not be caught by "мар": check the longer prefixes first
    const idx = MONTHS.findIndex((p) => (p === "ма" ? w[2] === "мая" || w[2] === "май" : w[2].startsWith(p)));
    if (idx < 0) return null;
    [d, m, y] = [Number(w[1]), idx + 1, Number(w[3])];
  }
  return `${y}-${String(m).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
}

function mondayOf(iso: string): string {
  const [y, m, d] = iso.split("-").map(Number);
  const wd = (new Date(Date.UTC(y, m - 1, d)).getUTCDay() + 6) % 7; // 0 = Monday
  return isoAddDays(iso, -wd);
}

function tokens(s: string): string[] {
  return s.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

/** "Ист. Бел. в кон. всем. ист." matches "История Беларуси в контексте всемирной истории". */
function sameSubject(a: string, b: string): boolean {
  const [ta, tb] = [tokens(a), tokens(b)];
  return ta.length > 0 && ta.length === tb.length && ta.every((x, i) => x.startsWith(tb[i]) || tb[i].startsWith(x));
}

/** Set by goToMark: which day / lesson to scroll to after the week renders. */
let pendingFocus: { dom: number; subject: string; value: string } | null = null;

function focusDay(f: typeof pendingFocus) {
  if (!f) return;
  const header = document.querySelector<HTMLElement>(`.day-header[data-dom="${f.dom}"]`);
  if (!header) return;
  const headerH = $("app-header").offsetHeight;
  window.scrollTo({ top: header.getBoundingClientRect().top + window.scrollY - headerH, behavior: "instant" });
  // Highlight the lesson the mark belongs to.
  let el = header.nextElementSibling as HTMLElement | null;
  const rows: HTMLElement[] = [];
  while (el && el.classList.contains("lesson")) {
    rows.push(el);
    el = el.nextElementSibling as HTMLElement | null;
  }
  const matches = rows.filter((r) => {
    const l = rowLessons.get(r)?.lesson;
    return !!l && (l.subject === f.subject || sameSubject(l.subject, f.subject));
  });
  const best = matches.find((r) => rowLessons.get(r)?.lesson.mark?.includes(f.value)) ?? matches[0];
  if (best) {
    best.classList.remove("flash");
    void best.offsetWidth;
    best.classList.add("flash");
  }
}

function goToMark(m: NewMark) {
  const iso = parseRuDate(m.date);
  if (!iso) return toast("Не понял дату отметки");
  pendingFocus = { dom: Number(iso.slice(8, 10)), subject: m.subject, value: m.value };
  setView({ kind: "chapter", index: view.kind === "chapter" ? view.index : -1 });
  const monday = mondayOf(iso);
  if (monday === currentWeekId) {
    focusDay(pendingFocus);
    pendingFocus = null;
    return;
  }
  const quarterId = quarters.find((q) => q.is_current)?.id ?? currentQuarterId;
  fetchWeek(quarterId, monday);
}

function applyWeek(res: ScheduleResponse, isLanding: boolean, keepScroll: number | null, scrollToToday: boolean) {
  currentQuarterId = res.quarter_id;
  currentWeekId = res.week_id;
  currentIsLanding = isLanding;
  if (view.kind === "chapter" && view.index < 0) syncChapterFromQuarter();

  renderSchedule(res.days, keepScroll === null && scrollToToday);

  const prev = $<HTMLButtonElement>("prev-week");
  const next = $<HTMLButtonElement>("next-week");
  prev.disabled = !res.prev_week_id;
  next.disabled = !res.next_week_id;
  prev.onclick = () => res.prev_week_id && fetchWeek(res.quarter_id, res.prev_week_id);
  next.onclick = () => res.next_week_id && fetchWeek(res.quarter_id, res.next_week_id);

  if (keepScroll !== null) {
    requestAnimationFrame(() => window.scrollTo({ top: keepScroll, behavior: "instant" }));
  } else if (!scrollToToday) {
    window.scrollTo({ top: 0, behavior: "instant" });
  }
  if (pendingFocus) {
    const f = pendingFocus;
    requestAnimationFrame(() => focusDay(f));
  }
}

$("reload-pill").addEventListener("click", () => {
  if (view.kind === "final") return;
  fetchWeek(currentQuarterId, currentIsLanding ? null : currentWeekId, { keepScroll: true, force: true });
});

function isToday(day: Day): boolean {
  const now = new Date();
  const m = day.date.match(/(\d{1,2})\.(\d{1,2})/);
  if (m) return Number(m[1]) === now.getDate() && Number(m[2]) === now.getMonth() + 1;
  const wd = now.toLocaleDateString("ru-RU", { weekday: "long" }).toLowerCase();
  return currentIsLanding && day.day_of_week.toLowerCase() === wd;
}

let shownDays: Day[] | null = null;
function renderSchedule(days: Day[], scrollToToday: boolean) {
  shownDays = days;
  const container = $("schedule");
  const frag = document.createDocumentFragment();
  let todayHeader: HTMLElement | null = null;

  for (const [dayIndex, day] of days.entries()) {
    if (day.lessons.length === 0) continue;

    const header = document.createElement("div");
    header.className = "day-header";
    header.textContent = day.date ? `${day.date}, ${day.day_of_week}` : day.day_of_week;
    const dom = day.date.match(/\d{1,2}/)?.[0];
    if (dom) header.dataset.dom = String(Number(dom));
    if (isToday(day)) {
      header.classList.add("today");
      todayHeader = header;
    }
    frag.appendChild(header);

    for (const lesson of day.lessons) {
      const row = document.createElement("div");
      row.className = "lesson";
      row.dataset.day = String(dayIndex);
      rowLessons.set(row, { day, dayIndex, lesson });
      row.addEventListener("click", (e) => {
        const t = e.target as HTMLElement;
        if (t.closest("a, button")) return;
        if (window.getSelection()?.toString()) return; // user is selecting homework text
        showLessonDetails(day, dayIndex, lesson);
      });

      const body = document.createElement("div");
      body.className = "lesson-body";

      const subject = document.createElement("p");
      subject.className = "lesson-subject";
      subject.textContent = lesson.number ? `${lesson.number}. ${lesson.subject}` : lesson.subject;

      const hwBox = document.createElement("div");
      hwBox.className = "homework-container";
      const hw = document.createElement("p");
      hw.className = "lesson-homework";
      hw.innerHTML = escapeHtml(lesson.homework ?? "Нет д/з").replace(
        /(https?:\/\/[^\s<]+)/g,
        '<a href="$1" class="hw-link">$1</a>',
      );
      hwBox.appendChild(hw);

      if (lesson.attachments_url) {
        const btn = document.createElement("button");
        btn.className = "attachment-btn";
        btn.innerHTML = `<img src="${fileIcon}" alt="" class="attachment-icon" /><span>Файл</span>`;
        btn.onclick = () => showAttachments(lesson.attachments_url!);
        onLongPress(btn, () => showAttachments(lesson.attachments_url!, true));
        hwBox.appendChild(btn);
      }

      body.append(subject, hwBox);
      row.appendChild(body);

      if (lesson.mark) {
        const mark = document.createElement("div");
        mark.className = `lesson-mark ${levelOfMark(lesson.mark)}`;
        mark.textContent = lesson.mark;
        row.appendChild(mark);
      }
      frag.appendChild(row);
    }
  }

  if (!frag.childNodes.length) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = "На этой неделе уроков нет";
    frag.appendChild(empty);
  }

  container.replaceChildren(frag);
  const det = readyDetails();
  if (det) applyNotes(det);
  container.classList.remove("fade-in");
  void container.offsetWidth;
  container.classList.add("fade-in");

  if (scrollToToday) {
    const headerH = $("app-header").offsetHeight;
    const top = todayHeader ? todayHeader.getBoundingClientRect().top + window.scrollY - headerH : 0;
    window.scrollTo({ top, behavior: "instant" });
  }
}

$("schedule").addEventListener("click", (e) => {
  const a = (e.target as HTMLElement).closest<HTMLAnchorElement>("a.hw-link");
  if (a) {
    e.preventDefault();
    openUrl(a.href);
  }
});

// ---------- attachments / bottom sheet ----------

/** Long press (~0.5 s) on `el` (or a `selector` match inside it); the click that follows is swallowed. */
function onLongPress(el: HTMLElement, cb: (target: HTMLElement) => void, selector?: string) {
  let timer = 0;
  let x = 0;
  let y = 0;
  let fired = false;
  const cancel = () => clearTimeout(timer);
  el.addEventListener("pointerdown", (e) => {
    const target = selector ? (e.target as HTMLElement).closest<HTMLElement>(selector) : el;
    if (!target) return;
    fired = false;
    x = e.clientX;
    y = e.clientY;
    cancel();
    timer = window.setTimeout(() => {
      fired = true;
      navigator.vibrate?.(30);
      cb(target);
    }, 500);
  });
  el.addEventListener("pointermove", (e) => {
    if (Math.abs(e.clientX - x) > 10 || Math.abs(e.clientY - y) > 10) cancel();
  });
  el.addEventListener("pointerup", cancel);
  el.addEventListener("pointercancel", cancel);
  el.addEventListener("contextmenu", (e) => e.preventDefault());
  el.addEventListener(
    "click",
    (e) => {
      if (!fired) return;
      fired = false;
      e.preventDefault();
      e.stopImmediatePropagation();
    },
    true,
  );
}

function openSheet(html: string, onClick?: (el: HTMLElement) => void) {
  const sheet = $("sheet");
  sheet.innerHTML = html;
  sheet.scrollTop = 0;
  sheetLongPress = null;
  sheet.onclick = (e) => {
    const link = (e.target as HTMLElement).closest<HTMLAnchorElement>("a.hw-link");
    if (link) {
      e.preventDefault();
      openUrl(link.href);
      return;
    }
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-url]");
    if (el && onClick) onClick(el);
  };
  document.body.classList.add("sheet-open");
}
function closeSheet() {
  document.body.classList.remove("sheet-open");
}
$("sheet-backdrop").addEventListener("click", closeSheet);
// Long press on a file in the sheet → share it.
let sheetLongPress: ((el: HTMLElement) => void) | null = null;
onLongPress($("sheet"), (el) => sheetLongPress?.(el), "[data-url]");

let opening = false;
async function openFile(f: Attachment, share = false) {
  if (opening) return;
  opening = true;
  toast(share ? `Готовлю «${f.name}»…` : `Открываю «${f.name}»…`);
  try {
    await invoke("open_attachment", { url: f.download_url, name: f.name, share });
  } catch (e) {
    handleError(e);
  } finally {
    opening = false;
  }
}

async function showAttachments(url: string, share = false) {
  try {
    const files = await invoke<Attachment[]>("fetch_attachments", { url });
    if (files.length === 0) return toast("Файлов нет");
    if (files.length === 1) return openFile(files[0], share);
    openSheet(
      files
        .map(
            (f, i) =>
            `<div class="sheet-item" data-url="${i}">${escapeHtml(f.name)} <span class="file-size">${escapeHtml(f.size ?? "")}</span></div>`,
        )
        .join(""),
      (el) => {
        closeSheet();
        openFile(files[Number(el.dataset.url)]);
      },
    );
    sheetLongPress = (el) => {
      closeSheet();
      openFile(files[Number(el.dataset.url)], true);
    };
  } catch (e) {
    handleError(e);
  }
}

// ---------- final ----------

let finalLoading = false;
let lastReport: FinalReport | null = store.get<FinalReport>("last-final");

const halvesOf = (r: FinalRow): [boolean, boolean] => r.halves ?? [!!r.half, !!r.half];

function periodLabel(row: FinalRow, col: number): string {
  if (col === 4) return "Год";
  return halvesOf(row)[col >> 1] ? `${(col >> 1) + 1} полугодие` : chapterLabel(col);
}

function cellHtml(c: FinalCell, rowIdx: number, col: number, span = false): string {
  const td = span ? ` colspan="2" class="half-cell${c.value ? "" : " empty-cell"}"` : c.value ? "" : ` class="empty-cell"`;
  if (!c.value) return `<td${td}>·</td>`;
  const cls = `${markLevel(c.num)}${c.computed ? " computed" : ""}`;
  return `<td${td}><button class="final-chip ${cls}" data-row="${rowIdx}" data-col="${col}">${escapeHtml(c.value)}</button></td>`;
}

function rowCells(r: FinalRow, i: number): string {
  // A half graded once spans both of its quarter columns; its mark is in the second one.
  const halves = halvesOf(r);
  return [0, 1]
    .map((h) =>
      halves[h]
        ? cellHtml(r.quarters[2 * h + 1], i, 2 * h + 1, true)
        : cellHtml(r.quarters[2 * h], i, 2 * h) + cellHtml(r.quarters[2 * h + 1], i, 2 * h + 1),
    )
    .join("");
}

// ---------- goal (target mark for the current period) ----------

let goal: number | null = store.get<number>("goal");
/** subject -> chosen plan key ("even+1", "only:9"), shown instead of the cheapest plan. */
let goalPins: Record<string, string> = store.get<Record<string, string>>("goal-pins") ?? {};
/** subject -> own goal (0 = no goal for it); overrides the common one. */
let subjectGoals: Record<string, number> = store.get<Record<string, number>>("subject-goals") ?? {};
const goalOf = (subject: string): number | null =>
  subject in subjectGoals ? subjectGoals[subject] || null : goal;
const MAX_PLAN = 40;
const CHECK_SVG = `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>`;

interface Plan {
  key: string;
  marks: number[];
}

/**
 * The period mark is the rounded average, so goal g needs avg ≥ g − 0.5,
 * i.e. 2·sum ≥ (2g − 1)·n. Positive deficit = not reached yet.
 */
const deficit = (sum: number, n: number, g: number) => (2 * g - 1) * n - 2 * sum;

function minCount(sum: number, n: number, g: number): number {
  const d = deficit(sum, n, g);
  if (d <= 0 && n > 0) return 0;
  return Math.max(1, Math.ceil(d / (21 - 2 * g)));
}

/** k new marks with the smallest total, spread as evenly as possible (lowest top mark). */
function evenPlan(sum: number, n: number, g: number, k: number): number[] | null {
  const need = Math.max(k, Math.ceil(((2 * g - 1) * (n + k) - 2 * sum) / 2));
  if (need > 10 * k) return null;
  const base = Math.floor(need / k);
  const r = need % k;
  return [...Array<number>(r).fill(base + 1), ...Array<number>(k - r).fill(base)];
}

/** Only marks `m` (m ≥ g). */
function onlyPlan(sum: number, n: number, g: number, m: number): number[] | null {
  if (m < g) return null;
  const k = Math.max(1, Math.ceil(deficit(sum, n, g) / (2 * m - 2 * g + 1)));
  return k <= MAX_PLAN ? Array<number>(k).fill(m) : null;
}

function plans(sum: number, n: number, g: number): Plan[] {
  const k0 = minCount(sum, n, g);
  if (k0 === 0 || k0 > MAX_PLAN) return [];
  const out: Plan[] = [];
  const seen = new Set<string>();
  const push = (key: string, marks: number[] | null) => {
    if (!marks) return;
    const sig = marks.join(",");
    if (seen.has(sig)) return;
    seen.add(sig);
    out.push({ key, marks });
  };
  for (let d = 0; d < 5 && k0 + d <= MAX_PLAN; d++) push(`even+${d}`, evenPlan(sum, n, g, k0 + d));
  for (let m = 10; m >= g; m--) push(`only:${m}`, onlyPlan(sum, n, g, m));
  return out.sort((a, b) => a.marks.length - b.marks.length || Math.max(...a.marks) - Math.max(...b.marks));
}

function planFor(key: string, sum: number, n: number, g: number): number[] | null {
  const k0 = minCount(sum, n, g);
  if (k0 === 0 || k0 > MAX_PLAN) return null;
  const even = key.match(/^even\+(\d+)$/);
  if (even) return evenPlan(sum, n, g, Math.min(MAX_PLAN, k0 + Number(even[1])));
  const only = key.match(/^only:(\d+)$/);
  if (only) return onlyPlan(sum, n, g, Number(only[1]));
  return null;
}

/** [9, 9, 8] -> "9×2 8" as chips. */
function planChips(marks: number[]): string {
  const groups: [number, number][] = [];
  for (const m of marks) {
    const last = groups[groups.length - 1];
    if (last && last[0] === m) last[1]++;
    else groups.push([m, 1]);
  }
  return groups
    .map(([m, c]) => `<span class="plan-mark ${markLevel(m)}">${m}${c > 1 ? `<small>×${c}</small>` : ""}</span>`)
    .join("");
}

const allMarks = (s: SubjectMarks) => [...(s.prev ?? []), ...s.marks];
const sumOf = (a: number[]) => a.reduce((x, y) => x + y, 0);

function goalHint(s: SubjectMarks, i: number): string {
  const goal = goalOf(s.subject);
  if (goal === null) return "";
  const all = allMarks(s);
  const [sum, n] = [sumOf(all), all.length];
  const k0 = minCount(sum, n, goal);
  let inner: string;
  if (k0 === 0) inner = `<span class="goal-ok">${CHECK_SVG}${goal}</span>`;
  else if (k0 > MAX_PLAN) inner = `<span class="goal-ok">${MAX_PLAN}+</span>`;
  else {
    const pin = goalPins[s.subject];
    const marks = (pin && planFor(pin, sum, n, goal)) || evenPlan(sum, n, goal, k0)!;
    inner = planChips(marks);
  }
  return `<button class="goal-hint" data-goal="${i}"><span class="goal-br">(</span>${inner}<span class="goal-br">)</span></button>`;
}

function renderGoalButton() {
  $("goal-label").textContent = goal === null ? "Цель" : `Цель ${goal}`;
  $("final-goal").classList.toggle("on", goal !== null);
}

function chooseGoal() {
  const opts = [10, 9, 8, 7, 6, 5, 4]
    .map((g) => `<button class="goal-opt ${markLevel(g)}${g === goal ? " picked" : ""}" data-url="${g}">${g}</button>`)
    .join("");
  openSheet(
    `<div class="sheet-title">Целевая отметка</div>
     <p class="sheet-sub">Покажу у каждого предмета, какие отметки нужно получить, чтобы выйти на цель. Зажми предмет, чтобы задать ему свою цель.</p>
     <div class="goal-grid">${opts}</div>
     <button class="goal-off" data-url="off">Без цели</button>`,
    (el) => {
      const v = el.dataset.url!;
      goal = v === "off" ? null : Number(v);
      store.set("goal", goal);
      closeSheet();
      renderGoalButton();
      if (lastReport) renderCurrent(lastReport);
    },
  );
}

function chooseSubjectGoal(s: SubjectMarks) {
  const own = s.subject in subjectGoals;
  const cur = goalOf(s.subject);
  const opts = [10, 9, 8, 7, 6, 5, 4]
    .map((g) => `<button class="goal-opt ${markLevel(g)}${own && g === cur ? " picked" : ""}" data-url="${g}">${g}</button>`)
    .join("");
  openSheet(
    `<div class="sheet-title">Цель · ${escapeHtml(s.subject)}</div>
     <p class="sheet-sub">Только для этого предмета.</p>
     <div class="goal-grid">${opts}</div>
     <button class="goal-off${own ? "" : " picked"}" data-url="common">Как у всех${goal === null ? " (без цели)" : ` (${goal})`}</button>
     <button class="goal-off${own && cur === null ? " picked" : ""}" data-url="off">Без цели</button>`,
    (el) => {
      const v = el.dataset.url!;
      if (v === "common") delete subjectGoals[s.subject];
      else subjectGoals[s.subject] = v === "off" ? 0 : Number(v);
      store.set("subject-goals", subjectGoals);
      closeSheet();
      if (lastReport) renderCurrent(lastReport);
    },
  );
}

function showGoalDetails(s: SubjectMarks) {
  const g = goalOf(s.subject);
  if (g === null) return;
  const all = allMarks(s);
  const [sum, n] = [sumOf(all), all.length];
  const avg = n ? sum / n : 0;
  const title = `<div class="sheet-title">${escapeHtml(s.subject)} → ${g}</div>`;
  const sub = `<p class="sheet-sub">Сейчас ${n ? avg.toFixed(2) : "—"} · ${n} отм.${s.half ? " за полугодие" : ""}. Для ${g} нужно среднее от ${(g - 0.5).toFixed(1)}.</p>`;
  const d = deficit(sum, n, g);

  if (n > 0 && d <= 0) {
    // Reached: how many lower marks it can take.
    const lines: string[] = [];
    for (let v = g - 1; v >= Math.max(1, g - 3); v--) {
      const j = Math.floor(-d / (2 * g - 1 - 2 * v));
      lines.push(
        `<div class="plan-row static"><span class="plan-mark ${markLevel(v)}">${v}</span><span class="plan-info">${
          j > 0 ? `выдержит ещё ${j} шт.` : "уже опасно"
        }</span></div>`,
      );
    }
    openSheet(`${title}${sub}<p class="goal-done">${CHECK_SVG}Цель достигнута</p><div class="det-label plan-caption">Запас</div>${lines.join("")}`);
    return;
  }

  const list = plans(sum, n, g);
  const render = () => {
    const pin = goalPins[s.subject];
    const rows = list
      .map((p) => {
        const after = (sum + sumOf(p.marks)) / (n + p.marks.length);
        const picked = pin ? pin === p.key : p === list[0];
        return `<div class="plan-row${picked ? " picked" : ""}" data-url="${p.key}">
          <span class="plan-chips">${planChips(p.marks)}</span>
          <span class="plan-info">${p.marks.length} отм. → ${after.toFixed(2)}</span>
        </div>`;
      })
      .join("");
    openSheet(
      `${title}${sub}${
        rows || `<p class="sheet-sub">Нужно больше ${MAX_PLAN} отметок.</p>`
      }<p class="sheet-sub plan-foot">Выбери вариант — он будет показан в списке. Зажми предмет, чтобы сменить его цель.</p>`,
      (el) => {
        const key = el.dataset.url!;
        if (key === list[0]?.key) delete goalPins[s.subject];
        else goalPins[s.subject] = key;
        store.set("goal-pins", goalPins);
        if (lastReport) renderCurrent(lastReport);
        render();
      },
    );
  };
  render();
}

$("final-goal").addEventListener("click", chooseGoal);
$("current-wrap").addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLElement>(".goal-hint");
  const s = btn && lastReport?.current?.subjects[Number(btn.dataset.goal)];
  if (s) showGoalDetails(s);
});
// Long press on a subject (its marks or the hint) → its own goal.
onLongPress(
  $("current-wrap"),
  (row) => {
    const s = lastReport?.current?.subjects[Number(row.dataset.i)];
    if (s) chooseSubjectGoal(s);
  },
  ".cur-row",
);

function markChips(marks: number[], extra = ""): string {
  return marks.map((m) => `<span class="cur-mark ${markLevel(m)}${extra}">${m}</span>`).join("");
}

function renderCurrent(report: FinalReport) {
  const cur = report.current;
  const title = $("current-title");
  const wrap = $("current-wrap");
  if (!cur) {
    title.textContent = "";
    wrap.innerHTML = "";
    return;
  }
  title.textContent = `${chapterLabel(cur.index)} — все отметки`;
  if (cur.subjects.length === 0) {
    wrap.innerHTML = `<p class="empty">В этой четверти отметок пока нет</p>`;
    return;
  }
  wrap.innerHTML = cur.subjects
    .map((s, i) => {
      const prev = s.prev ?? [];
      const marks =
        markChips(prev, " prev") + (prev.length && s.marks.length ? `<span class="cur-sep"></span>` : "") + markChips(s.marks);
      const own = s.subject in subjectGoals ? goalOf(s.subject) : undefined;
      const tags =
        (s.half ? `<span class="half-tag">полугодие</span>` : "") +
        (own !== undefined ? `<span class="half-tag goal-tag">${own === null ? "без цели" : `цель ${own}`}</span>` : "");
      return `<div class="cur-row" data-i="${i}">
        <div class="cur-head">
          <span class="cur-subj">${escapeHtml(s.subject)}${tags}</span>
          <span class="cur-avg ${markLevel(s.avg)}">${s.avg.toFixed(2)}</span>
        </div>
        <div class="cur-marks">${marks}${goalHint(s, i)}</div>
      </div>`;
    })
    .join("");
}

function renderFinal(report: FinalReport) {
  lastReport = report;
  renderCurrent(report);
  const wrap = $("final-table-wrap");
  if (report.rows.length === 0) {
    wrap.innerHTML = `<p class="empty">Итоговых пока нет</p>`;
    return;
  }
  const head = ["1", "2", "3", "4"].map((n) => `<th>${n}</th>`).join("") + "<th>Год</th>";
  const body = report.rows
    .map(
      (r, i) =>
        `<tr><th class="subj">${escapeHtml(r.subject)}</th>${rowCells(
          r,
          i,
        )}${cellHtml(r.year, i, 4)}</tr>`,
    )
    .join("");
  wrap.innerHTML = `<table class="final-table"><thead><tr><th class="subj">Предмет</th>${head}</tr></thead><tbody>${body}</tbody></table>`;
}

$("final-table-wrap").addEventListener("click", (e) => {
  const chip = (e.target as HTMLElement).closest<HTMLElement>(".final-chip");
  if (!chip || !lastReport) return;
  const row = lastReport.rows[Number(chip.dataset.row)];
  const col = Number(chip.dataset.col);
  const cell = col === 4 ? row.year : row.quarters[col];
  const title = `${escapeHtml(row.full ?? row.subject)} · ${periodLabel(row, col)}`;
  const chips = cell.marks.length
    ? `<div class="sheet-marks">${cell.marks.map((m) => `<span class="lesson-mark inline ${markLevel(m)}">${m}</span>`).join("")}</div>`
    : "";
  let detail: string;
  if (cell.predicted) {
    detail = `<p class="sheet-sub">Прогноз schools.by: <b>${escapeHtml(cell.value ?? "")}</b>. Учитель ещё не выставил.</p>${chips}`;
  } else if (cell.marks.length) {
    detail = `${chips}<p class="sheet-sub">${cell.count} отм. · среднее ${cell.value}</p>`;
  } else if (cell.computed) {
    detail = `<p class="sheet-sub">Среднее по ${col === 4 && halvesOf(row).some(Boolean) ? (halvesOf(row).every(Boolean) ? "полугодиям" : "четвертям и полугодиям") : "четвертям"}: ${cell.value}</p>`;
  } else {
    detail = `<p class="sheet-sub">Итоговая отметка: ${cell.value}</p>`;
  }
  openSheet(`<div class="sheet-title">${title}</div>${detail}`);
});

async function showFinal(force = false) {
  setView({ kind: "final" });
  if (lastReport) renderFinal(lastReport);
  if (finalLoading) return;
  finalLoading = true;
  setBusy(true);
  $<HTMLButtonElement>("final-reload").disabled = true;
  $("final-status").textContent = force ? "Пересчитываю всё заново…" : lastReport ? "Обновляю…" : "Загружаю итоги…";
  try {
    const report = await invoke<FinalReport>("fetch_final", { force });
    store.set("last-final", report);
    renderFinal(report);
    $("final-status").textContent = "";
  } catch (e) {
    $("final-status").textContent = "";
    handleError(e);
  } finally {
    finalLoading = false;
    setBusy(false);
    $<HTMLButtonElement>("final-reload").disabled = false;
  }
}

$("final-reload").addEventListener("click", () => showFinal(true));

listen<{ quarter: number; weeks: number; label: string }>("final-progress", (e) => {
  if (view.kind !== "final") return;
  const { quarter, weeks, label } = e.payload;
  $("final-status").textContent = `Собираю отметки · ${chapterLabel(quarter)} · неделя ${weeks}${label ? ` (${label})` : ""}`;
});

// ---------- self-update ----------

interface UpdateInfo {
  version: string;
  notes: string;
  url: string;
  size: number;
  sha256: string;
}

let update: UpdateInfo | null = store.get<UpdateInfo>("update-info") ?? null;
let updating = false;
// One unauthenticated GitHub request; their limit is 60/hour, so every 30 min is safe.
const UPDATE_CHECK_EVERY = 30 * 60_000;

/** Minimal Markdown for release notes: headings, lists, **bold**, `code`, [links](…), bare URLs. */
function markdown(src: string): string {
  const inline = (t: string) =>
    escapeHtml(t)
      .replace(/`([^`]+)`/g, "<code>$1</code>")
      .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
      .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" class="hw-link">$1</a>')
      .replace(/(^|[\s(])(https?:\/\/[^\s<)]+)/g, '$1<a href="$2" class="hw-link">$2</a>');
  const out: string[] = [];
  let list = false;
  for (const raw of src.replace(/\r/g, "").split("\n")) {
    const line = raw.trim();
    const li = line.match(/^[-*]\s+(.*)/);
    if (list && !li) {
      out.push("</ul>");
      list = false;
    }
    if (!line) continue;
    const h = line.match(/^#{1,6}\s+(.*)/);
    if (h) out.push(`<h4>${inline(h[1])}</h4>`);
    else if (li) {
      if (!list) out.push("<ul>");
      list = true;
      out.push(`<li>${inline(li[1])}</li>`);
    } else out.push(`<p>${inline(line)}</p>`);
  }
  if (list) out.push("</ul>");
  return out.join("");
}

function renderBanner(progress?: number) {
  const banner = $("update-banner");
  banner.hidden = !update;
  if (!update) return;
  const downloading = progress !== undefined;
  banner.classList.toggle("loading", downloading);
  banner.style.setProperty("--p", `${progress ?? 0}%`);
  banner.innerHTML = downloading
    ? `<span class="ub-text">Скачиваю обновление${progress ? ` · ${progress}%` : "…"}</span>`
    : `<span class="ub-dot"></span><span class="ub-text">Доступна версия ${escapeHtml(update.version)}</span><span class="ub-go">Что нового</span>`;
}

async function checkUpdate(manual = false) {
  const last = store.get<number>("update-check") ?? 0;
  if (!manual && Date.now() - last < UPDATE_CHECK_EVERY) return renderBanner();
  try {
    update = await invoke<UpdateInfo | null>("check_update");
    store.set("update-check", Date.now());
    store.set("update-info", update);
  } catch (e) {
    if (manual) toast("Не удалось проверить обновления");
    console.error("update", e);
    return;
  }
  if (!updating) renderBanner();
  if (!update && manual) toast("Установлена последняя версия");
}

function showUpdate() {
  if (!update || updating) return;
  const u = update;
  const size = u.size ? ` · ${(u.size / 1048576).toFixed(1)} МБ` : "";
  openSheet(
    `<div class="sheet-title">Версия ${escapeHtml(u.version)}</div>
     <p class="sheet-sub">Сейчас ${escapeHtml(appVersion)}${size}</p>
     ${u.notes ? `<div class="update-notes">${markdown(u.notes)}</div>` : ""}
     <button class="update-go" data-url="go">Обновить</button>`,
    () => {
      closeSheet();
      installUpdate(u);
    },
  );
}

async function installUpdate(u: UpdateInfo) {
  if (updating) return;
  updating = true;
  setBusy(true);
  renderBanner(0);
  try {
    await invoke("install_update", { url: u.url, sha256: u.sha256 });
  } catch (e) {
    if (String(e).includes("UPDATE_VERIFY")) toast("Обновление не прошло проверку подлинности и не установлено");
    else handleError(e);
  } finally {
    updating = false;
    setBusy(false);
    renderBanner();
  }
}

listen<number>("update-progress", (e) => {
  if (updating) renderBanner(e.payload);
});
$("update-banner").addEventListener("click", showUpdate);
// The WebView survives in the background, so startApp() alone would check only once per cold start.
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") checkUpdate();
});

let appVersion = "";
invoke<string>("app_version")
  .then((v) => {
    appVersion = v;
    // Just updated: the cached "new version" is the one running now.
    if (store.get<string>("update-for") !== v) {
      store.set("update-for", v);
      store.set("update-check", 0);
      store.set("update-info", null);
      update = null;
      renderBanner();
    }
  })
  .catch(() => {});

// ---------- settings ----------

function repaintMarks() {
  if (shownDays) renderSchedule(shownDays, false);
  if (lastReport) renderFinal(lastReport);
}

function colorGrid(): string {
  return Array.from({ length: 10 }, (_, i) => i + 1)
    .map((m) => `<button class="color-opt mark-${colorOf(m)}" data-url="m${m}">${m}</button>`)
    .join("");
}

function showSettings() {
  openSheet(
    `<div class="sheet-title">Настройки</div>
     <div class="set-group">
       <div class="det-label">Цвета отметок</div>
       <p class="sheet-sub">Нажми на отметку, чтобы сменить цвет</p>
       <div class="color-grid">${colorGrid()}</div>
       <button class="set-btn" data-url="reset">Сбросить цвета</button>
     </div>
     <div class="set-group">
       <div class="det-label">Обновления</div>
       <button class="set-btn" data-url="update" id="set-update">Проверить обновления</button>
     </div>
     <p class="set-version">schools.bye${appVersion ? ` · версия ${escapeHtml(appVersion)}` : ""}</p>`,
    async (el) => {
      const key = el.dataset.url!;
      if (key === "update") {
        const b = el as HTMLButtonElement;
        b.disabled = true;
        b.textContent = "Проверяю…";
        await checkUpdate(true);
        b.disabled = false;
        b.textContent = "Проверить обновления";
        if (update) showUpdate();
        return;
      }
      if (key === "reset") markColors = {};
      else {
        const m = Number(key.slice(1));
        const next = MARK_COLORS[(MARK_COLORS.indexOf(colorOf(m)) + 1) % MARK_COLORS.length];
        if (next === defaultColor(m)) delete markColors[m];
        else markColors[m] = next;
      }
      store.set("mark-colors", markColors);
      $("sheet").querySelector(".color-grid")!.innerHTML = colorGrid();
      repaintMarks();
    },
  );
}

$("settings-btn").addEventListener("click", showSettings);

// ---------- scroll indicator ----------

{
  const thumb = $("scroll-thumb");
  let hideTimer = 0;
  const paint = () => {
    const doc = document.documentElement;
    const vh = window.innerHeight;
    const max = doc.scrollHeight - vh;
    if (max <= 4 || document.body.classList.contains("sheet-open")) return;
    const top = $("app-header").offsetHeight + 6;
    const track = vh - top - 6;
    const h = Math.max(36, (track * vh) / doc.scrollHeight);
    thumb.style.height = `${h}px`;
    thumb.style.transform = `translateY(${top + ((track - h) * window.scrollY) / max}px)`;
    thumb.classList.add("show");
    clearTimeout(hideTimer);
    hideTimer = window.setTimeout(() => thumb.classList.remove("show"), 900);
  };
  window.addEventListener("scroll", paint, { passive: true });
}

// ---------- auth ----------

function showLogin(message = "") {
  $("login-error").textContent = message;
  $("login-modal").classList.remove("hidden");
}

$("login-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const user = $<HTMLInputElement>("login-username");
  const pass = $<HTMLInputElement>("login-password");
  const btn = $<HTMLButtonElement>("login-submit");
  $("login-error").textContent = "";
  btn.disabled = true;
  try {
    await invoke("login", { username: user.value, password: pass.value });
    pass.value = "";
    details.clear();
    detailsReady.clear();
    apiTokenMissing = false;
    lastMarksCheck = 0;
    $("login-modal").classList.add("hidden");
    startApp();
  } catch (err) {
    const msg = String(err);
    $("login-error").textContent = msg.includes("NETWORK") ? "Нет связи с schools.by" : "Неверный логин или пароль";
  } finally {
    btn.disabled = false;
  }
});

$("logout-btn").addEventListener("click", async () => {
  if (!confirm("Вы точно хотите выйти?")) return;
  await invoke("logout");
  store.clear();
  store.set("mark-colors", markColors); // device preference, not account data
  weekCache.clear();
  details.clear();
  detailsReady.clear();
  newMarks = [];
  $("marks-btn").classList.remove("show");
  quarters = [];
  lastReport = null;
  $("schedule").innerHTML = "";
  $("final-table-wrap").innerHTML = "";
  showLogin();
});

function startApp() {
  setView({ kind: "chapter", index: -1 });
  const last = store.get<ScheduleResponse>("last-week");
  if (last) weekCache.set(cacheKey(null, null), last);
  fetchWeek(null, null, { scrollToToday: true });
  // After the first page is in: one request to GitHub, not to schools.by.
  renderBanner();
  setTimeout(() => checkUpdate(), 4000);
}

window.addEventListener("DOMContentLoaded", async () => {
  renderMenu();
  renderGoalButton();
  let restored = false;
  try {
    restored = await invoke<boolean>("restore_session");
  } catch (e) {
    console.error(e);
  }
  if (restored) startApp();
  else showLogin();
});
