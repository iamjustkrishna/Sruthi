import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type ModelState = "missing" | "downloading" | "loading" | "ready" | "error";
type LlmState = "off" | "missing" | "downloading" | "starting" | "ready" | "error";

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
  dictionary: string[];
  hotkey: string;
  history: HistoryEntry[];
}

interface HistoryEntry {
  id: string;
  time_ms: number;
  seconds: number;
  heard: string;
  text: string;
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
const history = $<HTMLUListElement>("history");
const historyEmpty = $("history-empty");

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
    const bar = document.createElement("div");
    bar.className = "progress";
    bar.innerHTML = `<div class="progress-bar" style="width:${pct}%"></div>`;
    const label = document.createElement("span");
    label.className = "muted small";
    label.textContent = p ? `${formatMb(p.downloaded)}${p.total ? ` of ${formatMb(p.total)}` : ""}` : "Starting…";
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

function render(s: Status) {
  status = s;
  const active = s.models.find((m) => m.id === s.active);
  const [label, text] = STATE_COPY[s.model];
  badge.textContent = label;
  badge.dataset.state = s.model;
  modelText.textContent =
    s.detail ?? (s.model === "ready" && active ? `${active.name} · ${text}` : text);
  $("hotkey").textContent = s.hotkey;
  renderModels(s);
  renderLlms(s);
  document.querySelectorAll<HTMLInputElement>('input[name="overlay"]').forEach((input) => {
    input.checked = input.value === s.overlay;
  });
  renderWords(s.dictionary);

  renderHistory(s.history);
}

// ---------------------------------------------------------------------------
// Recent dictations with playable recordings

let player: HTMLAudioElement | null = null;
let playingId: string | null = null;

function stopPlayback() {
  if (player) {
    player.pause();
    URL.revokeObjectURL(player.src);
  }
  player = null;
  playingId = null;
  document.querySelectorAll<HTMLButtonElement>(".play").forEach((b) => (b.textContent = "▶"));
}

async function togglePlay(id: string, button: HTMLButtonElement) {
  const wasPlaying = playingId === id;
  stopPlayback();
  if (wasPlaying) return;
  try {
    const bytes = await invoke<ArrayBuffer>("get_recording", { id });
    const audio = new Audio(URL.createObjectURL(new Blob([bytes], { type: "audio/wav" })));
    player = audio;
    playingId = id;
    button.textContent = "■";
    audio.addEventListener("ended", stopPlayback);
    await audio.play();
  } catch (e) {
    stopPlayback();
    showError(`Couldn't play that recording: ${e}`);
  }
}

function when(ms: number) {
  const d = new Date(ms);
  const today = new Date();
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return d.toDateString() === today.toDateString() ? `Today ${time}` : `${d.toLocaleDateString()} ${time}`;
}

function renderHistory(entries: HistoryEntry[]) {
  history.replaceChildren(
    ...entries.map((e) => {
      const li = document.createElement("li");
      li.className = "entry";

      const play = button("▶", () => togglePlay(e.id, play), "ghost play");
      play.title = "Play recording";
      play.setAttribute("aria-label", "Play recording");
      if (playingId === e.id) play.textContent = "■";

      const body = document.createElement("div");
      body.className = "entry-body";
      const meta = document.createElement("span");
      meta.className = "muted small";
      meta.textContent = `${when(e.time_ms)} · ${e.seconds.toFixed(1)}s`;
      const text = document.createElement("span");
      text.className = e.text ? "entry-text" : "entry-text muted";
      text.textContent = e.text || "(nothing was heard)";
      if (e.text) {
        text.title = "Click to copy";
        text.addEventListener("click", () => navigator.clipboard.writeText(e.text));
      }
      body.append(meta, text);
      // Show the raw transcript when formatting changed it.
      if (e.heard && e.heard !== e.text) {
        const heard = document.createElement("span");
        heard.className = "muted small";
        heard.textContent = `Heard: ${e.heard}`;
        body.append(heard);
      }

      const remove = button("×", () => {
        if (playingId === e.id) stopPlayback();
        run("delete_recording", { id: e.id });
      }, "ghost remove");
      remove.title = "Delete recording";
      remove.setAttribute("aria-label", "Delete recording");

      li.append(play, body, remove);
      return li;
    }),
  );
  historyEmpty.hidden = entries.length > 0;
}

$("open-folder").addEventListener("click", () => run("open_recordings_folder", {}));

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
  if (combo) saveHotkey(combo);
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

document.querySelectorAll<HTMLInputElement>('input[name="overlay"]').forEach((input) => {
  input.addEventListener("change", () => {
    if (input.checked) run("set_overlay_position", { position: input.value });
  });
});

listen<Status>("status-changed", (e) => render(e.payload));
listen<DownloadProgress>("download-progress", (e) => {
  progress[e.payload.id] = e.payload;
  if (status) renderModels(status);
});
listen<DownloadProgress>("llm-progress", (e) => {
  progress[e.payload.id] = e.payload;
  if (status) renderLlms(status);
});
listen<string>("download-error", (e) => showError(e.payload));

invoke<Status>("get_status").then(render);
