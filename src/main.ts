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
  hotkey: string;
  instant_mic: boolean;
  theme: ThemeName;
  microphone: string | null;
  microphones: string[];
  default_microphone: string | null;
  mic_warning: string | null;
  history: HistoryEntry[];
  usage: Usage;
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

function render(s: Status) {
  status = s;
  document.documentElement.dataset.theme = s.theme;
  const active = s.models.find((m) => m.id === s.active);
  const [label, text] = STATE_COPY[s.model];
  badge.textContent = label;
  badge.dataset.state = s.model;
  modelText.textContent =
    s.detail ?? (s.model === "ready" && active ? `${active.name} · ${text}` : text);
  $("hotkey").textContent = s.hotkey;
  renderModels(s);
  renderLlms(s);
  renderCloud(s);
  document.querySelectorAll<HTMLInputElement>('input[name="overlay"]').forEach((input) => {
    input.checked = input.value === s.overlay;
  });
  renderWords(s.dictionary);
  $<HTMLInputElement>("instant-toggle").checked = s.instant_mic;
  renderMicrophones(s);

  showHoldPhase(holdPhase);
}

// ---------------------------------------------------------------------------
// Hold-to-dictate button: same flow as the hotkey, press and release

const holdBtn = $<HTMLButtonElement>("hold-dictate");
const HOLD_LABELS = {
  idle: "Hold to dictate",
  recording: "Listening… release to finish",
  transcribing: "Transcribing…",
} as const;
type HoldPhase = keyof typeof HOLD_LABELS;
let holdPhase: HoldPhase = "idle";
/** True only between this button's press and release (used by the blur net). */
let buttonHeld = false;

function showHoldPhase(phase: HoldPhase) {
  holdPhase = phase;
  holdBtn.textContent = HOLD_LABELS[phase];
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
  // The microphone controls sit right below; jump to them and start checking.
  $("mic-card").scrollIntoView({ behavior: "smooth", block: "center" });
  checkMicrophones();
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
