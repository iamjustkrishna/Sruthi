// Home's day filter: a small vertical wheel beside "All days" for stepping
// from day to day, and a date heading that opens a heat calendar for a date
// far back. Scroll, drag, press the arrow keys or click a faded day; the feed
// follows whichever day is picked.

export interface DaySummary {
  date: string;
  count: number;
  words: number;
}

interface Hooks {
  /** Picks a day, or null for all days. */
  select: (date: string | null) => void;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

/** A pixel of wheel travel (or drag) that moves one day. */
const WHEEL_STEP = 70;
const DRAG_STEP = 26;
const WEEK = 7;

let hooks: Hooks;
/** Days that have dictations, newest first. */
let list: DaySummary[] = [];
let selected: string | null = null;
let labelFor: (date: string) => string = (d) => d;
let calOpen = false;
let calYear = 0;
let calMonth = 0;
let wheelAcc = 0;
let lastDir: "older" | "newer" | null = null;

const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

const keyOf = (y: number, m: number, d: number) => `${y}-${String(m + 1).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
const parse = (date: string) => new Date(`${date}T12:00:00`);

function index() {
  return selected ? list.findIndex((d) => d.date === selected) : -1;
}

/** Moves `by` days through the days that have dictations (positive: older). */
function step(by: number) {
  if (list.length === 0) return;
  const at = index();
  // From "All days" the first step lands on the newest day.
  const next = at < 0 ? 0 : Math.max(0, Math.min(list.length - 1, at + by));
  if (next === at) return;
  lastDir = by > 0 ? "older" : "newer";
  hooks.select(list[next].date);
}

// ---------------------------------------------------------------------------
// The wheel

function wheelRow(day: DaySummary | undefined, kind: "up" | "mid" | "down", idle: boolean, onClick: () => void) {
  const row = el("button", `wheel-row ${kind}${idle && kind === "mid" ? " idle" : ""}`);
  row.type = "button";
  row.tabIndex = -1;
  if (!day) {
    row.disabled = true;
    row.setAttribute("aria-hidden", "true");
    return row;
  }
  const d = parse(day.date);
  row.append(el("span", "wheel-dow", d.toLocaleDateString([], { weekday: "short" })), el("span", "wheel-num", String(d.getDate())));
  row.title = `${labelFor(day.date)} · ${plural(day.count, "dictation")}`;
  row.addEventListener("click", onClick);
  return row;
}

function drawWheel() {
  const wheel = $("day-wheel");
  const rows = $("wheel-rows");
  const at = index();
  const mid = at < 0 ? 0 : at;
  const idle = at < 0;
  wheel.classList.toggle("idle", idle);
  wheel.setAttribute("aria-valuetext", selected ? labelFor(selected) : "All days");
  wheel.hidden = list.length === 0;
  rows.replaceChildren(
    wheelRow(list[mid + 1], "up", idle, () => step(1)),
    wheelRow(list[mid], "mid", idle, () => {
      if (idle) hooks.select(list[0]?.date ?? null);
    }),
    wheelRow(list[mid - 1], "down", idle, () => step(-1)),
  );
  // A small slide in the direction of travel.
  rows.classList.remove("tick-older", "tick-newer");
  if (lastDir) {
    void rows.offsetWidth;
    rows.classList.add(`tick-${lastDir}`);
    lastDir = null;
  }
}

function initWheel() {
  const wheel = $("day-wheel");

  // Mouse wheel and touchpad: one day for each notch of travel.
  wheel.addEventListener(
    "wheel",
    (e) => {
      if (list.length < 2) return;
      e.preventDefault();
      wheelAcc += e.deltaY * (e.deltaMode === 1 ? 33 : 1);
      while (Math.abs(wheelAcc) >= WHEEL_STEP) {
        // Scrolling down moves on to a later day.
        step(wheelAcc > 0 ? -1 : 1);
        wheelAcc -= Math.sign(wheelAcc) * WHEEL_STEP;
      }
    },
    { passive: false },
  );

  // Drag (mouse, pen or finger): pulling down reveals earlier days.
  let drag: { y: number; moved: number } | null = null;
  wheel.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    drag = { y: e.clientY, moved: 0 };
    wheel.setPointerCapture(e.pointerId);
    wheel.classList.add("dragging");
  });
  wheel.addEventListener("pointermove", (e) => {
    if (!drag) return;
    let dy = e.clientY - drag.y;
    drag.moved = Math.max(drag.moved, Math.abs(dy));
    while (Math.abs(dy) >= DRAG_STEP) {
      step(dy > 0 ? 1 : -1);
      drag.y += Math.sign(dy) * DRAG_STEP;
      dy = e.clientY - drag.y;
    }
  });
  const end = (e: PointerEvent) => {
    if (!drag) return;
    // A real drag must not also count as a click on a row.
    if (drag.moved > 6) {
      const swallow = (ev: Event) => ev.stopPropagation();
      wheel.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => wheel.removeEventListener("click", swallow, { capture: true }), 0);
    }
    drag = null;
    wheel.classList.remove("dragging");
    if (wheel.hasPointerCapture(e.pointerId)) wheel.releasePointerCapture(e.pointerId);
  };
  wheel.addEventListener("pointerup", end);
  wheel.addEventListener("pointercancel", end);

  // Keyboard: ↑ earlier, ↓ later, Page keys a week, Esc back to all days.
  wheel.addEventListener("keydown", (e) => {
    const by =
      e.key === "ArrowUp" ? 1 : e.key === "ArrowDown" ? -1 : e.key === "PageUp" ? WEEK : e.key === "PageDown" ? -WEEK : 0;
    if (by) {
      e.preventDefault();
      step(by);
    } else if (e.key === "Escape" && selected) {
      hooks.select(null);
    }
  });
}

// ---------------------------------------------------------------------------
// The date heading and its calendar

function drawTitle() {
  const day = selected ? list.find((d) => d.date === selected) : undefined;
  $("date-title").textContent = selected ? labelFor(selected) : "All days";
  const count = selected ? (day?.count ?? 0) : list.reduce((n, d) => n + d.count, 0);
  const words = selected ? (day?.words ?? 0) : list.reduce((n, d) => n + d.words, 0);
  $("date-meta").textContent = `${plural(count, "dictation")} · ${plural(words, "word")}`;
}

function level(words: number, max: number) {
  if (words <= 0) return 0;
  const r = words / Math.max(1, max);
  return r <= 0.25 ? 1 : r <= 0.5 ? 2 : r <= 0.75 ? 3 : 4;
}

function drawCalendar() {
  const by = new Map(list.map((d) => [d.date, d]));
  const now = new Date();
  const today = keyOf(now.getFullYear(), now.getMonth(), now.getDate());
  const first = new Date(calYear, calMonth, 1);
  const length = new Date(calYear, calMonth + 1, 0).getDate();
  const lead = (first.getDay() + 6) % 7; // Monday first
  const monthDays = Array.from({ length }, (_, i) => by.get(keyOf(calYear, calMonth, i + 1)));
  const max = Math.max(1, ...monthDays.map((d) => d?.words ?? 0));

  $("cal-title").textContent = first.toLocaleDateString([], { month: "long", year: "numeric" });
  const oldest = list.length ? parse(list[list.length - 1].date) : now;
  const atOldest = calYear * 12 + calMonth <= oldest.getFullYear() * 12 + oldest.getMonth();
  const atNewest = calYear * 12 + calMonth >= now.getFullYear() * 12 + now.getMonth();
  $<HTMLButtonElement>("cal-prev").disabled = atOldest;
  $<HTMLButtonElement>("cal-next").disabled = atNewest;

  const cells: HTMLElement[] = [];
  for (let i = 0; i < lead; i++) cells.push(el("span", "cal-cell blank"));
  for (let n = 1; n <= length; n++) {
    const date = keyOf(calYear, calMonth, n);
    const d = monthDays[n - 1];
    const cell = el("button", `cal-cell heat-${level(d?.words ?? 0, max)}`, String(n));
    cell.type = "button";
    cell.disabled = !d;
    cell.classList.toggle("today", date === today);
    cell.classList.toggle("picked", date === selected);
    cell.setAttribute(
      "aria-label",
      `${parse(date).toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" })}${d ? `, ${plural(d.count, "dictation")}` : ", no dictations"}`,
    );
    if (d) cell.title = `${plural(d.count, "dictation")} · ${plural(d.words, "word")}`;
    cell.addEventListener("click", () => {
      closeCalendar();
      hooks.select(date);
    });
    cells.push(cell);
  }
  $("cal-grid").replaceChildren(...cells);
  $("cal-today").hidden = !by.has(today);
}

function openCalendar() {
  const base = parse(selected ?? list[0]?.date ?? keyOf(new Date().getFullYear(), new Date().getMonth(), 1));
  calYear = base.getFullYear();
  calMonth = base.getMonth();
  calOpen = true;
  $("cal-pop").hidden = false;
  $("date-btn").setAttribute("aria-expanded", "true");
  drawCalendar();
}

function closeCalendar() {
  if (!calOpen) return;
  calOpen = false;
  $("cal-pop").hidden = true;
  $("date-btn").setAttribute("aria-expanded", "false");
}

function initCalendar() {
  $("date-btn").addEventListener("click", () => (calOpen ? closeCalendar() : openCalendar()));
  const shift = (by: number) => {
    const d = new Date(calYear, calMonth + by, 1);
    calYear = d.getFullYear();
    calMonth = d.getMonth();
    drawCalendar();
  };
  $("cal-prev").addEventListener("click", () => shift(-1));
  $("cal-next").addEventListener("click", () => shift(1));
  $("cal-today").addEventListener("click", () => {
    const now = new Date();
    closeCalendar();
    hooks.select(keyOf(now.getFullYear(), now.getMonth(), now.getDate()));
  });
  document.addEventListener("pointerdown", (e) => {
    if (calOpen && !(e.target as Element).closest?.(".date-wrap")) closeCalendar();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && calOpen) {
      closeCalendar();
      $("date-btn").focus();
    }
  });
}

// ---------------------------------------------------------------------------

export function initDayPicker(h: Hooks) {
  hooks = h;
  initWheel();
  initCalendar();
}

/** Redraws the wheel, the heading and (if open) the calendar. */
export function renderDayPicker(days: DaySummary[], picked: string | null, label: (date: string) => string) {
  list = days;
  selected = picked;
  labelFor = label;
  $("day-all").setAttribute("aria-pressed", String(picked === null));
  drawWheel();
  drawTitle();
  if (calOpen) drawCalendar();
}
