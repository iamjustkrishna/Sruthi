import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type ModelState = "missing" | "downloading" | "loading" | "ready" | "error";

interface ModelEntry {
  id: string;
  name: string;
  description: string;
  languages: string;
  size_mb: number;
  installed: boolean;
}

interface Status {
  model: ModelState;
  detail: string | null;
  active: string;
  models: ModelEntry[];
  downloading: string | null;
  overlay: "bottom" | "field";
  hotkey: string;
  history: string[];
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
const history = $<HTMLUListElement>("history");
const historyEmpty = $("history-empty");

const STATE_COPY: Record<ModelState, [string, string]> = {
  missing: ["Not installed", "Download a model once to start dictating. Everything runs offline."],
  downloading: ["Downloading", "Downloading the speech model…"],
  loading: ["Loading", "Loading the speech model into memory…"],
  ready: ["Ready", "Running locally on this PC."],
  error: ["Error", "Something went wrong."],
};

let status: Status | null = null;
let progress: DownloadProgress | null = null;

function formatMb(bytes: number) {
  return `${Math.round(bytes / 1_048_576)} MB`;
}

function showError(message: string | null) {
  modelError.hidden = !message;
  modelError.textContent = message ?? "";
}

async function run(command: string, id: string) {
  showError(null);
  try {
    await invoke(command, { id });
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

function renderModels(s: Status) {
  modelList.replaceChildren(
    ...s.models.map((m) => {
      const li = document.createElement("li");
      li.className = "model";
      li.dataset.active = String(m.id === s.active);

      const info = document.createElement("div");
      info.className = "model-info";
      info.innerHTML = `<strong></strong><span class="muted"></span><span class="muted small"></span>`;
      info.children[0].textContent = m.name;
      info.children[1].textContent = m.description;
      info.children[2].textContent = `${m.languages} · ${m.size_mb} MB`;

      const actions = document.createElement("div");
      actions.className = "model-actions";

      if (s.downloading === m.id) {
        const pct = progress?.id === m.id && progress.total > 0 ? (progress.downloaded / progress.total) * 100 : 0;
        const bar = document.createElement("div");
        bar.className = "progress";
        bar.innerHTML = `<div class="progress-bar" style="width:${pct}%"></div>`;
        const label = document.createElement("span");
        label.className = "muted small";
        label.textContent =
          progress?.id === m.id
            ? `${formatMb(progress.downloaded)}${progress.total ? ` of ${formatMb(progress.total)}` : ""}`
            : "Starting…";
        actions.append(bar, label);
      } else if (!m.installed) {
        const b = button("Download", () => run("download_model", m.id));
        b.disabled = s.downloading !== null;
        actions.append(b);
      } else if (m.id === s.active) {
        const tag = document.createElement("span");
        tag.className = "tag";
        tag.textContent = "In use";
        actions.append(tag);
      } else {
        actions.append(
          button("Use", () => run("select_model", m.id)),
          button("Delete", () => {
            if (confirm(`Delete ${m.name} (${m.size_mb} MB) from this PC?`)) run("delete_model", m.id);
          }, "ghost"),
        );
      }

      li.append(info, actions);
      return li;
    }),
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
  document.querySelectorAll<HTMLInputElement>('input[name="overlay"]').forEach((input) => {
    input.checked = input.value === s.overlay;
  });

  history.replaceChildren(
    ...s.history.map((text) => {
      const li = document.createElement("li");
      li.textContent = text;
      li.title = "Click to copy";
      li.addEventListener("click", () => navigator.clipboard.writeText(text));
      return li;
    }),
  );
  historyEmpty.hidden = s.history.length > 0;
}

document.querySelectorAll<HTMLInputElement>('input[name="overlay"]').forEach((input) => {
  input.addEventListener("change", () => {
    if (input.checked) invoke("set_overlay_position", { position: input.value }).catch((e) => showError(String(e)));
  });
});

listen<Status>("status-changed", (e) => render(e.payload));
listen<DownloadProgress>("download-progress", (e) => {
  progress = e.payload;
  if (status) renderModels(status);
});
listen<string>("download-error", (e) => showError(e.payload));

invoke<Status>("get_status").then(render);
