import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";

type ModelState = "missing" | "downloading" | "loading" | "ready" | "error";
type LlmState = "off" | "missing" | "downloading" | "starting" | "ready" | "error";
type ThemeName = "system" | "light" | "dark";

/** Mirrors the pill's phases so the hold button follows along. */
type DictationPhaseEvent =
  | { phase: "recording" }
  | { phase: "transcribing" }
  | { phase: "done"; text: string }
  | { phase: "error"; message: string };

interface ModelEntry {
  id: string;
  name: string;
  description: string;
  languages: string;
  size_mb: number;
  installed: boolean;
}

interface LlmEntry {
  id: string;
  name: string;
  description: string;
  size_mb: number;
  download_mb: number;
  installed: boolean;
}

interface Status {
  model: ModelState;
  detail: string | null;
  active: string;
  models: ModelEntry[];
  downloading: string | null;
  overlay: "bottom" | "field";
  smart: boolean;
  llm: {
    state: LlmState;
    detail: string | null;
    active: string;
    models: LlmEntry[];
    downloading: string | null;
  };
  cloud: {
    active: string | null;
    providers: CloudEntry[];
  };
  dictionary: string[];
  snippets: Snippet[];
  hotkey: string;
  instant_mic: boolean;
  theme: ThemeName;
  microphone: string | null;
  microphones: string[];
  default_microphone: string | null;
  mic_warning: string | null;
  gpu: { name: string | null; runtime_installed: boolean };
  device: "auto" | "gpu" | "cpu";
  engine_on_gpu: boolean;
  gpu_note: string | null;
  memory_gate: { message: string; gpu_path: boolean; card_option: "use-card" | "download" | null } | null;
  mic_fallback: string | null;
  gpu_downloading: boolean;
  history: HistoryEntry[];
  usage: Usage;
}

interface Snippet {
  trigger: string;
  text: string;
  exact: boolean;
}

interface CloudEntry {
  id: string;
  name: string;
  model: string;
  default_model: string;
  key_saved: boolean;
  key_url: string;
}

interface MicProbe {
  name: string;
  is_default: boolean;
  level: number;
  working: boolean;
  volume: number;
  muted: boolean;
}

interface HistoryEntry {
  id: string;
  time_ms: number;
  seconds: number;
  heard: string;
  text: string;
  /** Process name of the app the text went into; empty for older entries. */
  app?: string;
}

interface UsageWeek {
  date: string;
  words: number;
}

interface UsageApp {
  name: string;
  words: number;
}

interface Usage {
  words: number;
  words_today: number;
  dictations: number;
  wpm: number;
  saved_min: number;
  streak: number;
  longest_streak: number;
  week: UsageWeek[];
  top_apps: UsageApp[];
  avg_latency_ms: number;
  cleaned: number;
}

interface HotkeyOption {
  hotkey: string;
  status: "current" | "free" | "taken";
  note: string;
  recommended: boolean;
}

interface DownloadProgress {
  id: string;
  downloaded: number;
  total: number;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const badge = $("model-badge");
const modelText = $("model-text");
const modelList = $<HTMLUListElement>("models");
const modelError = $("model-error");
const llmText = $("llm-text");
const llmList = $<HTMLUListElement>("llms");
const smartToggle = $<HTMLInputElement>("smart-toggle");

const STATE_COPY: Record<ModelState, [string, string]> = {
  missing: ["Not installed", "Download a model once to start dictating. Everything runs offline."],
  downloading: ["Downloading", "Downloading the speech model…"],
  loading: ["Loading", "Loading the speech model into memory…"],
  ready: ["Ready", "Running locally on this PC."],
  error: ["Error", "Something went wrong."],
};

const LLM_COPY: Record<LlmState, string> = {
  off: "Off. Numbers, dates, codes and spoken punctuation are still cleaned up instantly.",
  missing: "Download a small AI model to remove filler words, fix self-corrections and format lists. Runs offline.",
  downloading: "Downloading the formatting model…",
  starting: "Starting the formatting model…",
  ready: "On. Each phrase is polished by a local AI model as you speak.",
  error: "Something went wrong.",
};

let status: Status | null = null;
const progress: Record<string, DownloadProgress> = {};

function formatMb(bytes: number) {
  return `${Math.round(bytes / 1_048_576)} MB`;
}

function showError(message: string | null) {
  modelError.hidden = !message;
  modelError.textContent = message ?? "";
}

async function run(command: string, args: Record<string, unknown>) {
  showError(null);
  try {
    await invoke(command, args);
  } catch (e) {
    showError(String(e));
  }
}

function button(label: string, onClick: () => void, variant = "") {
  const b = document.createElement("button");
  b.textContent = label;
  if (variant) b.className = variant;
  b.addEventListener("click", onClick);
  return b;
}

interface RowOptions {
  id: string;
  name: string;
  description: string;
  meta: string;
  installed: boolean;
  active: boolean;
  downloading: string | null;
  sizeMb: number;
  commands: { download: string; use: string; remove: string };
}

/** One row of a model list: name and details, plus Download / Use / Delete. */
function modelRow(o: RowOptions) {
  const li = document.createElement("li");
  li.className = "model";
  li.dataset.active = String(o.active && o.installed);

  const info = document.createElement("div");
  info.className = "model-info";
  info.innerHTML = `<strong></strong><span class="muted"></span><span class="muted small"></span>`;
  info.children[0].textContent = o.name;
  info.children[1].textContent = o.description;
  info.children[2].textContent = o.meta;

  const actions = document.createElement("div");
  actions.className = "model-actions";
  const p = progress[o.id];

  if (o.downloading === o.id) {
    const pct = p && p.total > 0 ? (p.downloaded / p.total) * 100 : 0;
    const label = document.createElement("span");
    label.className = "muted small";
    label.textContent = p ? `${formatMb(p.downloaded)}${p.total ? ` of ${formatMb(p.total)}` : ""}` : "Starting…";
    const bar = document.createElement("div");
    bar.className = "progress";
    // Progress must reach the screen reader too, not just the pixels.
    bar.setAttribute("role", "progressbar");
    bar.setAttribute("aria-label", `Downloading ${o.name}`);
    bar.setAttribute("aria-valuemin", "0");
    bar.setAttribute("aria-valuemax", "100");
    bar.setAttribute("aria-valuenow", String(Math.round(pct)));
    bar.setAttribute("aria-valuetext", label.textContent ?? "");
    bar.innerHTML = `<div class="progress-bar" style="width:${pct}%"></div>`;
    actions.append(bar, label);
  } else if (!o.installed) {
    const b = button("Download", () => run(o.commands.download, { id: o.id }));
    b.disabled = o.downloading !== null;
    actions.append(b);
  } else if (o.active) {
    const tag = document.createElement("span");
    tag.className = "tag";
    tag.textContent = "In use";
    actions.append(tag);
  } else {
    actions.append(
      button("Use", () => run(o.commands.use, { id: o.id })),
      button(
        "Delete",
        () => {
          if (confirm(`Delete ${o.name} (${o.sizeMb} MB) from this PC?`)) run(o.commands.remove, { id: o.id });
        },
        "ghost",
      ),
    );
  }

  li.append(info, actions);
  return li;
}

function renderModels(s: Status) {
  modelList.replaceChildren(
    ...s.models.map((m) =>
      modelRow({
        id: m.id,
        name: m.name,
        description: m.description,
        meta: `${m.languages} · ${m.size_mb} MB`,
        installed: m.installed,
        active: m.id === s.active,
        downloading: s.downloading,
        sizeMb: m.size_mb,
        commands: { download: "download_model", use: "select_model", remove: "delete_model" },
      }),
    ),
  );
}

function renderLlms(s: Status) {
  smartToggle.checked = s.smart;
  llmText.textContent = s.llm.detail ?? LLM_COPY[s.llm.state];
  llmList.hidden = !s.smart;
  llmList.replaceChildren(
    ...s.llm.models.map((m) =>
      modelRow({
        id: m.id,
        name: m.name,
        description: m.description,
        meta: m.installed ? `${m.size_mb} MB` : `${m.download_mb} MB download`,
        installed: m.installed,
        active: m.id === s.llm.active,
        downloading: s.llm.downloading,
        sizeMb: m.size_mb,
        commands: { download: "download_llm", use: "select_llm", remove: "delete_llm" },
      }),
    ),
  );
}

// ---------------------------------------------------------------------------
// AI engine: the local model or a cloud provider with the user's API key

/** Provider whose settings are open (null: on this PC). */
let viewing: string | null | undefined;
const cloudPanel = $("cloud-panel");
const cloudKey = $<HTMLInputElement>("cloud-key");
const cloudModel = $<HTMLInputElement>("cloud-model");
const cloudResult = $("cloud-test-result");

function cloudMessage(text: string, isError = false) {
  cloudResult.textContent = text;
  cloudResult.classList.toggle("error", isError);
}

async function cloudRun(command: string, args: Record<string, unknown>) {
  try {
    await invoke(command, args);
    return true;
  } catch (e) {
    cloudMessage(String(e), true);
    return false;
  }
}

function engineButton(id: string | null, label: string, s: Status) {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = label;
  b.setAttribute("aria-pressed", String(s.cloud.active === id));
  b.classList.toggle("viewing", (viewing ?? s.cloud.active) === id);
  b.addEventListener("click", async () => {
    if (viewing !== id) cloudMessage("");
    viewing = id;
    const entry = s.cloud.providers.find((p) => p.id === id);
    // A provider is switched on once it has a key; until then its settings open.
    if (id === null || entry?.key_saved) {
      await cloudRun("set_cloud_provider", { provider: id });
    } else if (status) {
      renderCloud(status);
      cloudKey.focus();
    }
  });
  return b;
}

function renderCloud(s: Status) {
  $("cloud").hidden = !s.smart;
  if (viewing === undefined) viewing = s.cloud.active;
  const inUse = s.cloud.providers.find((p) => p.id === s.cloud.active);
  if (s.smart && inUse) {
    llmText.textContent = `On. Dictations are polished by ${inUse.name} (${inUse.model}); the model on this PC is the backup.`;
  }
  $("engines").replaceChildren(
    engineButton(null, "On this PC", s),
    ...s.cloud.providers.map((p) => engineButton(p.id, p.name, s)),
  );
  const entry = s.cloud.providers.find((p) => p.id === viewing);
  // On this PC: pick the offline model; a cloud provider: its key and model.
  llmList.hidden = !s.smart || !!entry;
  cloudPanel.hidden = !entry;
  if (!entry) return;

  const state = $("cloud-key-state");
  const getKey = document.createElement("button");
  getKey.type = "button";
  getKey.className = "linkish";
  getKey.textContent = `Get a ${entry.name} key`;
  getKey.addEventListener("click", () => openUrl(entry.key_url));
  if (entry.key_saved) {
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "linkish";
    remove.textContent = "Remove key";
    remove.addEventListener("click", () => cloudRun("delete_cloud_key", { provider: entry.id }));
    state.replaceChildren(
      s.cloud.active === entry.id ? "In use. " : "Key saved. ",
      "Your API key is stored in Windows Credential Manager. ",
      remove,
    );
  } else {
    state.replaceChildren("Add your API key to use this provider. ", getKey);
  }
  cloudKey.placeholder = entry.key_saved ? "Paste a new key to replace it" : "Paste your API key";
  if (document.activeElement !== cloudModel) {
    cloudModel.value = entry.model;
    cloudModel.placeholder = entry.default_model;
  }
  $<HTMLButtonElement>("cloud-test").disabled = !entry.key_saved;
}

$("cloud-key-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const provider = viewing;
  const key = cloudKey.value.trim();
  if (!provider || !key) return;
  if (await cloudRun("save_cloud_key", { provider, key })) {
    cloudKey.value = "";
    if (await cloudRun("set_cloud_provider", { provider })) cloudMessage("Key saved. Try Test to check it works.");
  }
});

cloudModel.addEventListener("change", () => {
  if (viewing) cloudRun("set_cloud_model", { provider: viewing, model: cloudModel.value });
});

$("cloud-test").addEventListener("click", async () => {
  if (!viewing) return;
  const test = $<HTMLButtonElement>("cloud-test");
  test.disabled = true;
  cloudMessage("Cleaning up a sample sentence…");
  try {
    const r = await invoke<{ text: string; ms: number }>("test_cloud", { provider: viewing });
    cloudMessage(`Works (${(r.ms / 1000).toFixed(1)} s): "${r.text}"`);
  } catch (e) {
    cloudMessage(String(e), true);
  } finally {
    test.disabled = false;
  }
});

/** Model badge text: state, plus where it runs once ready. */
function badgeLabel(s: Status, label: string) {
  return s.engine_on_gpu ? `${label} · on the card` : label;
}

/* Where the speech model runs: Auto | Graphics card | CPU, plus the
   card's runtime state and anything the engine wants to explain. */
function renderDevice(s: Status) {
  for (const d of ["auto", "gpu", "cpu"] as const) {
    const b = $<HTMLButtonElement>(`device-${d}`);
    b.classList.toggle("active", s.device === d);
    b.setAttribute("aria-pressed", String(s.device === d));
    if (d === "gpu") b.disabled = !s.gpu.name;
  }
  const p = progress["gpu-runtime"];
  const downloading = $("gpu-runtime-download");
  const offer = s.device !== "cpu" && s.gpu.name !== null && !s.gpu.runtime_installed;
  downloading.hidden = !offer || s.gpu_downloading;
  const prog = $("gpu-runtime-progress");
  prog.hidden = !s.gpu_downloading;
  if (s.gpu_downloading) {
    const pct = p && p.total > 0 ? ` ${Math.round((p.downloaded / p.total) * 100)}%` : "";
    prog.textContent = `Downloading the card runtime…${pct}`;
  }
  const parts = [s.gpu.name ?? "No dedicated graphics card"];
  if (s.gpu.name !== null) {
    parts.push(s.gpu.runtime_installed ? "runtime ready" : "runtime not installed");
  }
  if (s.gpu_note) parts.push(s.gpu_note);
  $("device-note").textContent = parts.join(" · ");
}

/* The two alert strips above the model card: the memory gate's question
   and the instant-mic fallback notice. */
function renderWarnings(s: Status) {
  const gate = $("memory-warning");
  gate.hidden = !s.memory_gate;
  if (s.memory_gate) {
    $("memory-warning-text").textContent = s.memory_gate.message;
    // Staying on CPU answers the question differently when the card was
    // never in the plan.
    $<HTMLButtonElement>("gate-cpu").hidden = !s.memory_gate.gpu_path;
    // The CPU was the plan but the card exists: offer the lighter path
    // (the click either switches the device or fetches the runtime).
    const card = $<HTMLButtonElement>("gate-card");
    card.hidden = !s.memory_gate.card_option;
    card.textContent =
      s.memory_gate.card_option === "use-card" ? "Use graphics card" : "Download card runtime";
  }
  const fallback = $("mic-fallback-warning");
  fallback.hidden = !s.mic_fallback;
  $("mic-fallback-text").textContent = s.mic_fallback ?? "";
}

/** The one-line value each foldable card shows while it is collapsed. */
function renderSummaries(s: Status, activeModel: string | undefined) {
  const mic = s.microphone ?? s.default_microphone;
  $("mic-summary").textContent = mic ? `${mic}${s.microphone ? "" : " · Windows default"}` : "Windows default";

  const runsOn = s.model === "ready" ? (s.engine_on_gpu ? "Graphics card" : "CPU") : "";
  $("model-summary").textContent = [activeModel ?? "No model yet", runsOn].filter(Boolean).join(" · ");

  const words = s.dictionary;
  $("dict-summary").textContent = words.length
    ? words.slice(0, 3).join(", ") + (words.length > 3 ? `, +${words.length - 3}` : "")
    : "No words yet";

  $("shortcut-summary").textContent = [
    s.hotkey,
    s.instant_mic ? "Instant start on" : "Instant start off",
    s.overlay === "field" ? "Pill under the field" : "Pill at bottom center",
  ].join(" · ");
}

function render(s: Status) {
  status = s;
  document.documentElement.dataset.theme = s.theme;
  const active = s.models.find((m) => m.id === s.active);
  const [label, text] = STATE_COPY[s.model];
  badge.textContent = badgeLabel(s, label);
  badge.dataset.state = s.model;
  modelText.textContent =
    s.detail ?? (s.model === "ready" && active ? `${active.name} · ${text}` : text);
  $("hotkey").textContent = s.hotkey;
  $("hotkey-hint").textContent = s.hotkey;
  renderSummaries(s, active?.name);
  renderModels(s);
  renderDevice(s);
  renderWarnings(s);
  renderLlms(s);
  renderCloud(s);
  document.querySelectorAll<HTMLInputElement>('input[name="overlay"]').forEach((input) => {
    input.checked = input.value === s.overlay;
  });
  renderWords(s.dictionary);
  $<HTMLInputElement>("instant-toggle").checked = s.instant_mic;
  renderMicrophones(s);
  renderUsage(s);
  renderHistory();
  renderSnippets(s.snippets);

  showHoldPhase(holdPhase);
}

// ---------------------------------------------------------------------------
// Pages: the sidebar picks one; the window remembers it between launches

const PAGES = ["home", "dictionary", "snippets", "models", "settings"] as const;
type Page = (typeof PAGES)[number];

const mainScroller = $("main");

function setPage(next: Page) {
  for (const p of PAGES) {
    $(`page-${p}`).hidden = p !== next;
    const item = $<HTMLButtonElement>(`nav-${p}`);
    item.classList.toggle("active", p === next);
    item.setAttribute("aria-current", p === next ? "page" : "false");
  }
  mainScroller.scrollTo({ top: 0 });
  try {
    localStorage.setItem("page", next);
  } catch {
    // Storage can be blocked; the sidebar still works without it.
  }
}

for (const p of PAGES) {
  $<HTMLButtonElement>(`nav-${p}`).addEventListener("click", () => setPage(p));
}
try {
  const saved = localStorage.getItem("page");
  if (PAGES.includes(saved as Page)) setPage(saved as Page);
} catch {
  // Start on Home.
}

// ---------------------------------------------------------------------------
// Home: everything you dictated

const historyList = $("history-list");
const historyEmpty = $("history-empty");
const historySearch = $<HTMLInputElement>("history-search");
/** Entries whose "what the model heard" panel is open. */
const heardOpen = new Set<string>();
let historySignature = "";
let playing: { id: string; audio: HTMLAudioElement; url: string } | null = null;

const ICONS = {
  copy: "M9 9h10v11H9zM5 15V4h10",
  play: "M7 5l12 7-12 7z",
  stop: "M7 7h10v10H7z",
  trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13",
  changes: "M4 7h16M4 12h10M4 17h16",
};

function icon(path: string) {
  return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${path}" /></svg>`;
}

/** "chrome.exe" -> "Chrome"; empty or unknown stays empty. */
function appLabel(process: string | undefined) {
  const base = (process ?? "").replace(/\.exe$/i, "").trim();
  if (!base || base === "unknown") return "";
  return base.charAt(0).toUpperCase() + base.slice(1);
}

function dayLabel(ms: number) {
  const d = new Date(ms);
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return d.toLocaleDateString([], { weekday: "long", month: "short", day: "numeric" });
}

const normalizeWord = (w: string) => w.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");

/**
 * What the model heard, with the words formatting dropped or rewrote struck
 * through: a word-level longest common subsequence against the final text.
 */
function heardWithChanges(heard: string, text: string): Node[] {
  const a = heard.split(/\s+/).filter(Boolean);
  const b = text.split(/\s+/).filter(Boolean).map(normalizeWord);
  const n = a.length;
  const m = b.length;
  const lcs = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] =
        normalizeWord(a[i]) === b[j] && b[j] !== ""
          ? lcs[i + 1][j + 1] + 1
          : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const nodes: Node[] = [];
  const push = (word: string, kept: boolean) => {
    const el = document.createElement(kept ? "span" : "del");
    el.textContent = word + " ";
    nodes.push(el);
  };
  let i = 0;
  let j = 0;
  while (i < n) {
    if (j < m && b[j] !== "" && normalizeWord(a[i]) === b[j] && lcs[i][j] === lcs[i + 1][j + 1] + 1) {
      push(a[i], true);
      i++;
      j++;
    } else if (j < m && lcs[i][j + 1] >= lcs[i + 1][j]) {
      j++;
    } else {
      push(a[i], false);
      i++;
    }
  }
  return nodes;
}

function actionButton(label: string, path: string, onClick: () => void, pressed?: boolean) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "act";
  b.innerHTML = `${icon(path)}<span>${label}</span>`;
  if (pressed !== undefined) b.setAttribute("aria-pressed", String(pressed));
  b.addEventListener("click", onClick);
  return b;
}

function stopPlayback() {
  if (!playing) return;
  playing.audio.pause();
  URL.revokeObjectURL(playing.url);
  playing = null;
}

async function togglePlay(id: string) {
  const wasPlaying = playing?.id === id;
  stopPlayback();
  if (wasPlaying) {
    renderHistory(true);
    return;
  }
  try {
    const bytes = await invoke<ArrayBuffer>("get_recording", { id });
    const url = URL.createObjectURL(new Blob([bytes], { type: "audio/wav" }));
    const audio = new Audio(url);
    playing = { id, audio, url };
    audio.addEventListener("ended", () => {
      if (playing?.audio === audio) stopPlayback();
      renderHistory(true);
    });
    await audio.play();
  } catch (e) {
    stopPlayback();
    historyEmpty.hidden = false;
    historyEmpty.textContent = `Couldn't play that recording: ${String(e)}`;
  }
  renderHistory(true);
}

function historyEntry(e: HistoryEntry) {
  const root = document.createElement("article");
  root.className = "entry";

  const meta = document.createElement("div");
  meta.className = "entry-meta";
  const app = appLabel(e.app);
  if (app) {
    const chip = document.createElement("span");
    chip.className = "app-chip";
    chip.textContent = app.charAt(0);
    chip.setAttribute("aria-hidden", "true");
    const name = document.createElement("b");
    name.textContent = app;
    meta.append(chip, name);
  }
  const words = e.text.trim() ? e.text.trim().split(/\s+/).length : 0;
  const when = new Date(e.time_ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const rest = document.createElement("span");
  rest.textContent = `${when} · ${Math.max(1, Math.round(e.seconds))} s${words ? ` · ${words} ${words === 1 ? "word" : "words"}` : ""}`;
  meta.append(rest);

  const text = document.createElement("p");
  text.className = e.text ? "entry-text" : "entry-text none";
  text.textContent = e.text || "Nothing was heard.";

  const copy = actionButton("Copy", ICONS.copy, async () => {
    const label = copy.querySelector("span")!;
    try {
      await invoke("copy_text", { text: e.text });
      label.textContent = "Copied";
      setTimeout(() => (label.textContent = "Copy"), 1400);
    } catch {
      // Clipboard busy; the text is still on screen to select.
    }
  });
  copy.disabled = !e.text;

  const isPlaying = playing?.id === e.id;
  const play = actionButton(
    isPlaying ? "Stop" : "Play",
    isPlaying ? ICONS.stop : ICONS.play,
    () => togglePlay(e.id),
    isPlaying,
  );

  const actions = document.createElement("div");
  actions.className = "entry-actions";
  actions.append(copy, play);
  if (e.heard && e.heard !== e.text) {
    const open = heardOpen.has(e.id);
    actions.append(
      actionButton(
        open ? "Hide changes" : "See changes",
        ICONS.changes,
        () => {
          if (heardOpen.has(e.id)) heardOpen.delete(e.id);
          else heardOpen.add(e.id);
          renderHistory(true);
        },
        open,
      ),
    );
  }
  const grow = document.createElement("span");
  grow.className = "grow";
  actions.append(
    grow,
    actionButton("Delete", ICONS.trash, () => {
      if (!confirm("Delete this dictation and its recording from this PC?")) return;
      if (playing?.id === e.id) stopPlayback();
      heardOpen.delete(e.id);
      run("delete_recording", { id: e.id });
    }),
  );

  root.append(meta, text);
  if (heardOpen.has(e.id) && e.heard) {
    const panel = document.createElement("div");
    panel.className = "entry-heard";
    const label = document.createElement("h3");
    label.textContent = "What the model heard";
    const body = document.createElement("p");
    body.append(...heardWithChanges(e.heard, e.text));
    panel.append(label, body);
    root.append(panel);
  }
  root.append(actions);
  return root;
}

function renderHistory(force = false) {
  const all = status?.history ?? [];
  const query = historySearch.value.trim().toLowerCase();
  const signature = `${all.map((e) => e.id).join(",")}|${query}|${[...heardOpen].join(",")}|${playing?.id ?? ""}`;
  if (!force && signature === historySignature) return;
  historySignature = signature;

  const shown = query
    ? all.filter((e) => `${e.text} ${e.heard} ${appLabel(e.app)}`.toLowerCase().includes(query))
    : all;

  const nodes: HTMLElement[] = [];
  let lastDay = "";
  for (const e of shown) {
    const day = dayLabel(e.time_ms);
    if (day !== lastDay) {
      const label = document.createElement("div");
      label.className = "day-label";
      label.textContent = day;
      nodes.push(label);
      lastDay = day;
    }
    nodes.push(historyEntry(e));
  }
  historyList.replaceChildren(...nodes);

  historyEmpty.hidden = shown.length > 0;
  if (!shown.length) {
    historyEmpty.textContent = all.length
      ? "No dictation matches that search."
      : "Nothing yet. Hold the shortcut in any app, say a sentence, and it shows up here.";
  }
}

historySearch.addEventListener("input", () => renderHistory());

// ---------------------------------------------------------------------------
// Snippets: a spoken phrase that types a saved block of text

const snippetList = $<HTMLUListElement>("snippet-list");
const snippetEmpty = $("snippet-empty");
const snippetForm = $<HTMLFormElement>("snippet-form");
const snippetTrigger = $<HTMLInputElement>("snippet-trigger");
const snippetText = $<HTMLTextAreaElement>("snippet-text");
const snippetExact = $<HTMLInputElement>("snippet-exact");
const snippetError = $("snippet-error");
const snippetDelete = $<HTMLButtonElement>("snippet-delete");
/** The saved phrase the form is editing; null while writing a new one. */
let editingSnippet: string | null = null;
let snippetsSeen: Snippet[] = [];

function showSnippetError(message: string | null) {
  snippetError.hidden = !message;
  snippetError.textContent = message ?? "";
}

function editSnippet(snippet: Snippet | null) {
  editingSnippet = snippet?.trigger ?? null;
  snippetTrigger.value = snippet?.trigger ?? "";
  snippetText.value = snippet?.text ?? "";
  snippetExact.checked = snippet?.exact ?? true;
  snippetDelete.hidden = !snippet;
  showSnippetError(null);
  renderSnippets(snippetsSeen);
}

function renderSnippets(snippets: Snippet[]) {
  snippetsSeen = snippets;
  snippetEmpty.hidden = snippets.length > 0;
  snippetEmpty.textContent = snippets.length
    ? ""
    : "No snippets yet. Save one on the right, then say its phrase in any sentence.";
  snippetList.replaceChildren(
    ...snippets.map((s) => {
      const li = document.createElement("li");
      const b = document.createElement("button");
      b.type = "button";
      b.className = "snippet-item";
      b.setAttribute("aria-pressed", String(s.trigger === editingSnippet));
      const name = document.createElement("strong");
      name.textContent = s.trigger;
      const preview = document.createElement("span");
      preview.textContent = s.text.replace(/\s+/g, " ");
      b.append(name, preview);
      b.addEventListener("click", () => editSnippet(s));
      li.append(b);
      return li;
    }),
  );
}

$("snippet-new").addEventListener("click", () => {
  editSnippet(null);
  snippetTrigger.focus();
});

snippetForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  showSnippetError(null);
  const trigger = snippetTrigger.value.trim().replace(/\s+/g, " ");
  try {
    await invoke("save_snippet", {
      trigger,
      text: snippetText.value,
      exact: snippetExact.checked,
      replacing: editingSnippet,
    });
    editingSnippet = trigger;
    snippetDelete.hidden = false;
    renderSnippets(status?.snippets ?? snippetsSeen);
  } catch (err) {
    showSnippetError(String(err));
  }
});

snippetDelete.addEventListener("click", async () => {
  if (editingSnippet === null) return;
  if (!confirm(`Delete the snippet "${editingSnippet}"?`)) return;
  await run("remove_snippet", { trigger: editingSnippet });
  editSnippet(null);
});

// ---------------------------------------------------------------------------
// Hold-to-dictate button: same flow as the hotkey, press and release

const holdBtn = $<HTMLButtonElement>("hold-dictate");
type HoldPhase = "idle" | "recording" | "transcribing";
let holdPhase: HoldPhase = "idle";
/** True only between this button's press and release (used by the blur net). */
let buttonHeld = false;

const holdTitle = $("hold-title");
const holdSub = $("hold-sub");
const statusChip = $("status-chip");
const statusText = $("status-text");

/** What the hero says and what the status chip shows, for a phase and model state. */
function heroCopy(phase: HoldPhase, model: ModelState | undefined) {
  if (phase === "recording") {
    return {
      title: "Listening… release to type",
      sub: "In this window the text opens in a result box. In other apps it types at your cursor.",
      chip: ["live", "Listening"],
    } as const;
  }
  if (phase === "transcribing") {
    return { title: "Transcribing…", sub: "One moment.", chip: ["live", "Transcribing"] } as const;
  }
  if (model === "ready") {
    return {
      title: "Hold to dictate",
      sub: "Hold the button, speak, release. The text goes to your cursor.",
      chip: ["ready", "Ready"],
    } as const;
  }
  if (model === "downloading" || model === "loading") {
    return {
      title: "Getting the model ready…",
      sub: "Dictation starts as soon as the speech model is loaded.",
      chip: ["wait", model === "loading" ? "Loading" : "Downloading"],
    } as const;
  }
  if (model === "error") {
    return { title: "The model needs attention", sub: "See the Speech model card below.", chip: ["error", "Error"] } as const;
  }
  return {
    title: "Download a model to start",
    sub: "Sruthi needs one speech model. It downloads once and then works offline.",
    chip: ["wait", model ? "Model needed" : "Starting"],
  } as const;
}

function showHoldPhase(phase: HoldPhase) {
  holdPhase = phase;
  const copy = heroCopy(phase, status?.model);
  holdTitle.textContent = copy.title;
  holdSub.textContent = copy.sub;
  statusChip.dataset.state = copy.chip[0];
  statusText.textContent = copy.chip[1];
  holdBtn.setAttribute("aria-label", copy.title);
  holdBtn.classList.toggle("recording", phase === "recording");
  holdBtn.disabled = (status?.model !== "ready") || phase !== "idle";
}

holdBtn.addEventListener("pointerdown", (e) => {
  if (holdBtn.disabled || e.button !== 0) return;
  // Keep capture, so a release outside the button (or the window) still lands.
  holdBtn.setPointerCapture(e.pointerId);
  e.preventDefault();
  buttonHeld = true;
  invoke("press_dictation").catch((err) => {
    showHoldPhase("idle");
    showError(String(err));
  });
});
holdBtn.addEventListener("pointerup", (e) => {
  if (holdBtn.hasPointerCapture(e.pointerId)) holdBtn.releasePointerCapture(e.pointerId);
  buttonHeld = false;
  invoke("release_dictation").catch(() => {});
});
holdBtn.addEventListener("pointercancel", () => {
  buttonHeld = false;
  invoke("release_dictation").catch(() => {});
});
// Keyboard twin: Space or Enter presses and holds, release ends, exactly
// like the pointer. Auto-repeat is filtered out.
let keyHeld = false;
holdBtn.addEventListener("keydown", (e) => {
  if (holdBtn.disabled || keyHeld || (e.code !== "Space" && e.code !== "Enter")) return;
  e.preventDefault();
  keyHeld = true;
  invoke("press_dictation").catch((err) => {
    keyHeld = false;
    showHoldPhase("idle");
    showError(String(err));
  });
});
holdBtn.addEventListener("keyup", (e) => {
  if (!keyHeld || (e.code !== "Space" && e.code !== "Enter")) return;
  e.preventDefault();
  keyHeld = false;
  invoke("release_dictation").catch(() => {});
});
// Safety net: never leave the mic capturing after the window loses focus.
window.addEventListener("blur", () => {
  if (buttonHeld || keyHeld) {
    buttonHeld = false;
    keyHeld = false;
    invoke("release_dictation").catch(() => {});
  }
});

listen<DictationPhaseEvent>("dictation", ({ payload }) => {
  showHoldPhase(
    payload.phase === "recording"
      ? "recording"
      : payload.phase === "transcribing"
        ? "transcribing"
        : "idle",
  );
});

// ---------------------------------------------------------------------------
// Recordings live in one folder; open it from here.

$("open-folder").addEventListener("click", () => run("open_recordings_folder", {}));

// ---------------------------------------------------------------------------
// Microphone

const micSelect = $<HTMLSelectElement>("mic-select");
const DEFAULT_MIC = "";

function renderMicrophones(s: Status) {
  const options = [
    new Option(`Windows default${s.default_microphone ? ` (${s.default_microphone})` : ""}`, DEFAULT_MIC),
    ...s.microphones.map((name) => new Option(name, name)),
  ];
  micSelect.replaceChildren(...options);
  micSelect.value = s.microphone ?? DEFAULT_MIC;

  const warning = $("mic-warning");
  warning.hidden = !s.mic_warning;
  $("mic-warning-text").textContent = s.mic_warning ?? "";
}

micSelect.addEventListener("change", () =>
  run("set_microphone", { name: micSelect.value === DEFAULT_MIC ? null : micSelect.value }),
);

async function checkMicrophones() {
  setPage("settings");
  $<HTMLDetailsElement>("mic-card").open = true;
  const statusLine = $("mic-check-status");
  statusLine.hidden = false;
  statusLine.textContent = "Listening to each microphone for a second… (stay quiet or talk normally)";
  $<HTMLButtonElement>("mic-check").disabled = true;
  try {
    const probes = await invoke<MicProbe[]>("probe_microphones");
    const working = probes.filter((p) => p.working && !p.muted);
    statusLine.textContent = working.length
      ? `${working.length} of ${probes.length} microphones receive sound.`
      : "None of your microphones receive any sound. Check that one is connected and not muted.";
    $<HTMLUListElement>("mic-probes").replaceChildren(
      ...probes.map((p) => {
        const li = document.createElement("li");
        li.className = `mic-probe ${p.working && !p.muted ? "ok" : "dead"}`;
        const info = document.createElement("div");
        info.className = "model-info";
        const name = document.createElement("strong");
        name.textContent = p.name + (p.is_default ? " (Windows default)" : "");
        const detail = document.createElement("span");
        detail.className = "small";
        detail.textContent = p.muted
          ? "Muted in Windows"
          : p.working
            ? `Receiving sound · volume ${Math.round(p.volume * 100)}%`
            : "No sound at all: unplugged, switched off, or a virtual device";
        info.append(name, detail);
        li.append(info);
        const chosen = (status?.microphone ?? DEFAULT_MIC) === p.name;
        if (p.working && !p.muted && !chosen) {
          li.append(button("Use", () => run("set_microphone", { name: p.name })));
        }
        return li;
      }),
    );
  } catch (e) {
    statusLine.textContent = `Couldn't check microphones: ${e}`;
  } finally {
    $<HTMLButtonElement>("mic-check").disabled = false;
  }
}

$("mic-check").addEventListener("click", checkMicrophones);
$("mic-warning-check").addEventListener("click", () => {
  // The microphone controls live in Settings; jump there and start checking.
  setPage("settings");
  $("mic-card").scrollIntoView({ behavior: "smooth", block: "center" });
  checkMicrophones();
});

// ---------------------------------------------------------------------------
// Your dictation: the usage numbers on the Stats view

function savedLabel(min: number) {
  if (min >= 60) return `${(min / 60).toFixed(1).replace(/.0$/, "")} h`;
  if (min >= 1) return `${Math.round(min)} min`;
  return `${Math.max(1, Math.round(min * 60))} s`;
}

function renderUsage(s: Status) {
  const u = s.usage;
  const has = u.dictations > 0;
  $("usage-empty").hidden = has;
  $<HTMLElement>("usage-grid").hidden = !has;
  $<HTMLElement>("usage-week").hidden = !has;
  $("usage-extra").textContent = "";
  if (!has) return;

  $("usage-words").textContent = u.words.toLocaleString();
  $("usage-today").textContent = u.words_today > 0 ? `${u.words_today.toLocaleString()} today` : "";
  $("usage-wpm").textContent = u.wpm > 0 ? String(Math.round(u.wpm)) : "–";
  $("usage-vs").textContent = u.wpm > 0 ? ` · ${(u.wpm / 40).toFixed(1).replace(/.0$/, "")}× typing` : "";
  $("usage-saved").textContent = savedLabel(u.saved_min);
  $("usage-streak").textContent = String(u.streak);

  const most = u.week.reduce((m, d) => Math.max(m, d.words), 1);
  $<HTMLElement>("usage-week").replaceChildren(
    ...u.week.map((d) => {
      const cell = document.createElement("button");
      cell.type = "button";
      cell.className = "day";
      if (new Date(`${d.date}T00:00:00`).toDateString() === new Date().toDateString()) cell.classList.add("today");
      cell.title = `${d.date}: ${d.words.toLocaleString()} words`;
      cell.setAttribute("aria-label", `${d.date}: ${d.words.toLocaleString()} words`);
      const bar = document.createElement("div");
      bar.className = "bar";
      bar.style.height = `${Math.max(6, Math.round((d.words / most) * 100))}%`;
      const when = new Date(`${d.date}T00:00:00`);
      const name = document.createElement("span");
      name.className = "day-name";
      name.textContent =
        when.toDateString() === new Date().toDateString()
          ? "Today"
          : when.toLocaleDateString([], { weekday: "short" });
      cell.append(bar, name);
      return cell;
    }),
  );

  const parts: string[] = [`${u.dictations.toLocaleString()} dictations`];
  if (u.avg_latency_ms > 0) parts.push(`text lands ${(u.avg_latency_ms / 1000).toFixed(1)} s after release on average`);
  if (u.cleaned > 0) parts.push(`${u.cleaned.toLocaleString()} words trimmed by formatting`);
  if (u.longest_streak > u.streak) parts.push(`best streak: ${u.longest_streak} days`);
  const [topApp] = u.top_apps;
  if (topApp) {
    const share = Math.round((topApp.words / Math.max(1, u.words)) * 100);
    parts.push(`most dictated into ${topApp.name} (${share}%)`);
  }
  $("usage-extra").textContent = parts.join(" · ");
}

$("usage-reset").addEventListener("click", () => {
  if (confirm("Clear all your dictation stats on this PC?")) run("reset_usage", {});
});

// ---------------------------------------------------------------------------
// Dictation key

const hotkeyChange = $<HTMLButtonElement>("hotkey-change");
const hotkeyCapture = $("hotkey-capture");
const hotkeySuggestions = $("hotkey-suggestions");
const hotkeyError = $("hotkey-error");
let capturing = false;

const MODIFIER_CODES = ["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"];

/** "Ctrl+Alt+D" from a key press; null while only modifiers are held. */
function comboFrom(e: KeyboardEvent): string | null {
  if (MODIFIER_CODES.includes(e.code)) return null;
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Win");
  const key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : e.code;
  return [...mods, key].join("+");
}

function showHotkeyError(message: string | null) {
  hotkeyError.hidden = !message;
  hotkeyError.textContent = message ?? "";
}

async function saveHotkey(hotkey: string) {
  showHotkeyError(null);
  try {
    await invoke("set_hotkey", { hotkey });
    stopCapture();
  } catch (e) {
    showHotkeyError(String(e));
  }
  await loadHotkeyOptions();
}

function stopCapture() {
  capturing = false;
  hotkeyCapture.hidden = true;
  hotkeyChange.textContent = "Change key";
}

async function loadHotkeyOptions() {
  const options = await invoke<HotkeyOption[]>("hotkey_options");
  $<HTMLUListElement>("hotkey-options").replaceChildren(
    ...options.map((o) => {
      const li = document.createElement("li");
      li.className = `hotkey-option ${o.status}${o.recommended ? " recommended" : ""}`;
      const key = document.createElement("kbd");
      key.textContent = o.hotkey;
      const label = document.createElement("span");
      label.className = "small";
      label.textContent =
        o.status === "current"
          ? "In use now"
          : o.status === "taken"
            ? "Taken by another app"
            : o.recommended
              ? "Free · recommended"
              : o.note
                ? `Free, but: ${o.note}`
                : "Free";
      li.append(key, label);
      if (o.status === "free") {
        const use = button("Use", () => saveHotkey(o.hotkey));
        li.append(use);
      }
      return li;
    }),
  );
}

hotkeyChange.addEventListener("click", async () => {
  if (capturing) {
    stopCapture();
    return;
  }
  capturing = true;
  showHotkeyError(null);
  hotkeyCapture.hidden = false;
  hotkeySuggestions.hidden = false;
  hotkeyChange.textContent = "Cancel";
  await loadHotkeyOptions();
});

window.addEventListener("keydown", (e) => {
  if (!capturing) return;
  e.preventDefault();
  if (e.code === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) {
    stopCapture();
    return;
  }
  const combo = comboFrom(e);
  if (combo) {
    saveHotkey(combo);
    return;
  }
  // A bare Alt press (left or right) is the hold-to-talk key; other lone
  // modifiers have no meaning here.
  if ((e.code === "AltLeft" || e.code === "AltRight") && !e.ctrlKey && !e.shiftKey && !e.metaKey) {
    saveHotkey("Alt");
  }
});

function renderWords(words: string[]) {
  $<HTMLUListElement>("words").replaceChildren(
    ...words.map((word) => {
      const li = document.createElement("li");
      li.className = "word";
      const text = document.createElement("span");
      text.textContent = word;
      const remove = document.createElement("button");
      remove.className = "ghost remove";
      remove.textContent = "×";
      remove.title = `Remove ${word}`;
      remove.setAttribute("aria-label", `Remove ${word}`);
      remove.addEventListener("click", () => run("remove_word", { word }));
      li.append(text, remove);
      return li;
    }),
  );
}

const wordInput = $<HTMLInputElement>("word-input");
$<HTMLFormElement>("word-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const word = wordInput.value.trim();
  if (!word) return;
  await run("add_word", { word });
  wordInput.value = "";
  wordInput.focus();
});

smartToggle.addEventListener("change", () => run("set_smart", { enabled: smartToggle.checked }));

const instantToggle = $<HTMLInputElement>("instant-toggle");
instantToggle.addEventListener("change", () => run("set_instant_mic", { enabled: instantToggle.checked }));

// Where the speech model runs.
for (const d of ["auto", "gpu", "cpu"] as const) {
  $<HTMLButtonElement>(`device-${d}`).addEventListener("click", () =>
    run("set_device", { device: d }),
  );
}
$("gate-anyway").addEventListener("click", () => run("memory_decision", { decision: "anyway" }));
$("gate-later").addEventListener("click", () => run("memory_decision", { decision: "later" }));
$("gate-cpu").addEventListener("click", () => run("memory_decision", { decision: "cpu" }));
$("gate-card").addEventListener("click", () =>
  run("memory_decision", {
    decision: status?.memory_gate?.card_option === "use-card" ? "card-use" : "card-download",
  }),
);
$("gpu-runtime-download").addEventListener("click", () => run("download_gpu_runtime", {}));

document.querySelectorAll<HTMLInputElement>('input[name="overlay"]').forEach((input) => {
  input.addEventListener("change", () => {
    if (input.checked) run("set_overlay_position", { position: input.value });
  });
});

listen<Status>("status-changed", (e) => render(e.payload));
listen<DownloadProgress>("download-progress", (e) => {
  progress[e.payload.id] = e.payload;
  if (!status) return;
  renderModels(status);
  if (e.payload.id === "gpu-runtime") renderDevice(status);
});
listen<DownloadProgress>("llm-progress", (e) => {
  progress[e.payload.id] = e.payload;
  if (status) renderLlms(status);
});
listen<string>("download-error", (e) => showError(e.payload));

invoke<Status>("get_status").then(render);
