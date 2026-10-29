// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

// First-run setup: splash, name and languages, this PC and its model, the
// download with shortcut and microphone, and a first real dictation. It is a
// layer over the main window (#onboarding in index.html) and talks to Rust
// through onboarding_plan, choose_model, download_model, finish_onboarding
// and the commands Settings already uses for the shortcut and microphone.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./onboarding.css";

interface ModelRow {
  id: string;
  name: string;
  description: string;
  languages: string;
  size_mb: number;
  installed: boolean;
  /** False when this OS can't run the model (Whisper models off Windows). */
  supported: boolean;
}

/** The part of the app's Status that setup reads. */
export interface OnboardingStatus {
  onboarding: boolean;
  name: string | null;
  languages: string[];
  hotkey: string;
  model: "missing" | "downloading" | "loading" | "ready" | "error";
  detail: string | null;
  active: string;
  models: ModelRow[];
  downloading: string | null;
  microphone: string | null;
  microphones: string[];
  default_microphone: string | null;
  mic_warning: string | null;
}

interface Hardware {
  cpu: string | null;
  cores: number;
  ram_total_mb: number | null;
  ram_free_mb: number | null;
  gpu: string | null;
  vram_mb: number | null;
  disk_free_mb: number | null;
  platform: string;
  whisper_engine: boolean;
}

interface Pick {
  id: string;
  why: string;
  fit: "good" | "slow" | "plain";
  note: string;
}

interface Plan {
  hardware: Hardware;
  primary: Pick;
  alternatives: Pick[];
  notice: string | null;
}

interface HotkeyOption {
  hotkey: string;
  status: "current" | "free" | "taken";
  note: string;
  recommended: boolean;
}

type DictationEvent =
  | { phase: "recording" }
  | { phase: "transcribing" }
  | { phase: "done"; text: string }
  | { phase: "error"; message: string };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", text = "") {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text) node.textContent = text;
  return node;
}

const STEPS = 4; // the steps after the splash
const ICON = {
  cpu: '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>',
  ram: '<rect x="3" y="8" width="18" height="8" rx="1"/><path d="M7 8v8M11 8v8M15 8v8M6 19v-3M18 19v-3"/>',
  gpu: '<rect x="2" y="6" width="18" height="11" rx="2"/><circle cx="11" cy="11.5" r="3"/><path d="M22 9v6M6 20h10"/>',
  disk: '<ellipse cx="12" cy="6" rx="8" ry="3"/><path d="M4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>',
};

/** "478 MB" below a gigabyte, "9.4 GB" above. */
function size(mb: number) {
  return mb < 1024 ? `${Math.round(mb)} MB` : `${(mb / 1024).toFixed(1)} GB`;
}

export function initOnboarding(logoUrl: string) {
  const root = $("onboarding");
  const nameInput = $<HTMLInputElement>("onb-name");
  const installBtn = $<HTMLButtonElement>("onb-install");
  const toTry = $<HTMLButtonElement>("onb-to-try");
  const hold = $<HTMLButtonElement>("onb-hold");
  const skipBtn = $<HTMLButtonElement>("onb-skip");
  $<HTMLImageElement>("onb-logo").src = logoUrl;
  $<HTMLImageElement>("onb-logo-small").src = logoUrl;

  let status: OnboardingStatus | null = null;
  let isOpen = false;
  let replay = false;
  let step = 0;
  const languages = new Set<string>(["english"]);
  let plan: Plan | null = null;
  let selected = "";
  let showAll = false;
  /** The model whose download this run started (or found installed). */
  let started = "";
  let startedAt = 0;
  let progress = { downloaded: 0, total: 0 };

  // ---- navigation -------------------------------------------------------

  function go(next: number) {
    step = next;
    root.dataset.step = String(step);
    root.querySelectorAll<HTMLElement>("[data-panel]").forEach((p) => {
      p.hidden = p.dataset.panel !== String(step);
    });
    const dots = $("onb-dots");
    dots.replaceChildren(
      ...Array.from({ length: STEPS }, (_, i) => el("i", i + 1 === step ? "on" : i + 1 < step ? "done" : "")),
      el("span", "", step > 0 ? `Step ${step} of ${STEPS}` : ""),
    );
    if (step === 1) nameInput.focus();
    if (step === 2) void loadPlan();
    if (step === 3) void enterSetup();
    if (step === 4) enterTry();
    if (step === 0) $("onb-start").focus();
  }

  root.querySelectorAll<HTMLButtonElement>("[data-next]").forEach((b) =>
    b.addEventListener("click", () => {
      if (b.dataset.clear) {
        nameInput.value = "";
        languages.clear();
      }
      if (!b.disabled) go(Number(b.dataset.next));
    }),
  );
  $("onb-start").addEventListener("click", () => go(1));

  root.querySelectorAll<HTMLButtonElement>(".onb-chip").forEach((chip) =>
    chip.addEventListener("click", () => {
      const key = chip.dataset.lang!;
      if (languages.has(key)) languages.delete(key);
      else languages.add(key);
      chip.setAttribute("aria-pressed", String(languages.has(key)));
    }),
  );

  function syncChips() {
    root.querySelectorAll<HTMLButtonElement>(".onb-chip").forEach((chip) =>
      chip.setAttribute("aria-pressed", String(languages.has(chip.dataset.lang!))),
    );
  }

  // ---- step 2: this PC and the model ------------------------------------

  function tile(icon: keyof typeof ICON, title: string, sub: string) {
    const t = el("div", "onb-tile");
    const ic = el("span", "onb-tile-ic");
    ic.innerHTML = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICON[icon]}</svg>`;
    const text = el("div");
    text.append(el("b", "", title), el("span", "", sub));
    t.append(ic, text);
    return t;
  }

  function renderTiles(hw: Hardware) {
    const sizes = (status?.models ?? []).map((m) => m.size_mb);
    const range = sizes.length ? `${size(Math.min(...sizes))} to ${size(Math.max(...sizes))}` : "";
    $("onb-tiles").replaceChildren(
      tile("cpu", hw.cpu ?? "Processor", hw.cores ? `${hw.cores} threads for local speech.` : "Runs speech on this PC."),
      tile(
        "ram",
        hw.ram_total_mb ? `${size(hw.ram_total_mb)} memory` : "Memory",
        hw.ram_free_mb ? `${size(hw.ram_free_mb)} free right now.` : "",
      ),
      hw.gpu
        ? tile("gpu", hw.gpu, hw.vram_mb ? `${size(hw.vram_mb)} of its own memory.` : "A graphics card Sruthi can use.")
        : hw.platform === "windows"
          ? tile("gpu", "No dedicated graphics card", "Sruthi will use your processor.")
          : tile("gpu", "Runs on your processor", "Sruthi doesn't use the graphics chip on this system yet. The processor is plenty."),
      tile("disk", hw.disk_free_mb ? `${size(hw.disk_free_mb)} free` : "Disk space", range ? `Models need ${range}.` : ""),
    );
  }

  function modelRow(id: string) {
    return status?.models.find((m) => m.id === id);
  }

  function option(pick: { id: string; why: string; fit?: Pick["fit"]; note?: string }, main: boolean) {
    const m = modelRow(pick.id);
    const b = el("button", `onb-opt${main ? " onb-opt-main" : ""}`);
    b.type = "button";
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(selected === pick.id));
    const name = el("div", "onb-opt-name");
    name.append(document.createTextNode(m?.name ?? pick.id));
    if (main) name.append(el("span", "onb-badge", "Recommended"));
    if (pick.note && pick.fit && pick.fit !== "plain") name.append(el("span", `onb-fit ${pick.fit}`, pick.note));
    const body = el("div", "onb-opt-body");
    body.append(name, el("div", "onb-opt-desc", pick.why));
    b.append(el("span", "onb-radio"), body, el("span", "onb-opt-size", m ? size(m.size_mb) : ""));
    b.addEventListener("click", () => {
      selected = pick.id;
      renderOptions();
    });
    return b;
  }

  function renderOptions() {
    if (!plan || !status) return;
    const shown = new Set([plan.primary.id, ...plan.alternatives.map((a) => a.id)]);
    $("onb-pick").replaceChildren(option(plan.primary, true), ...(plan.notice ? [el("p", "onb-hint", plan.notice)] : []));
    const rest = status.models.filter((m) => m.supported && !shown.has(m.id));
    const rows = [
      ...plan.alternatives.map((a) => option(a, false)),
      ...(showAll ? rest.map((m) => option({ id: m.id, why: `${m.description} ${m.languages}.` }, false)) : []),
    ];
    $("onb-alts").replaceChildren(...rows);
    $("onb-others-label").hidden = rows.length === 0;
    const all = $<HTMLButtonElement>("onb-all");
    all.hidden = rest.length === 0;
    all.textContent = showAll ? "Show fewer models" : `Show all ${status.models.filter((m) => m.supported).length} models`;
    const m = modelRow(selected);
    installBtn.textContent = m ? (m.installed ? `Use ${m.name}` : `Download ${m.name} (${size(m.size_mb)})`) : "Continue";
  }

  async function loadPlan() {
    $("onb-tiles").replaceChildren(el("p", "onb-hint", "Looking at your PC…"));
    try {
      plan = await invoke<Plan>("onboarding_plan", { languages: [...languages] });
    } catch (e) {
      $("onb-tiles").replaceChildren(el("p", "onb-error", `Couldn't read your PC (${e}). The recommended model is still a safe choice.`));
      plan = {
        hardware: { cpu: null, cores: 0, ram_total_mb: null, ram_free_mb: null, gpu: null, vram_mb: null, disk_free_mb: null, platform: "", whisper_engine: false },
        primary: { id: "parakeet-v3", why: "Best all-round accuracy, and quick on any modern processor.", fit: "plain", note: "" },
        alternatives: [],
        notice: null,
      };
    }
    if (plan.hardware.cpu || plan.hardware.ram_total_mb) renderTiles(plan.hardware);
    const known = [plan.primary.id, ...plan.alternatives.map((a) => a.id)];
    if (!known.includes(selected) && !(showAll && modelRow(selected))) selected = plan.primary.id;
    renderOptions();
  }

  $("onb-all").addEventListener("click", () => {
    showAll = !showAll;
    renderOptions();
  });

  installBtn.addEventListener("click", () => go(3));

  // ---- step 3: download, shortcut, microphone ---------------------------

  async function startInstall() {
    const m = modelRow(selected);
    if (!m) return;
    $("onb-dl-error").hidden = true;
    started = m.id;
    startedAt = Date.now();
    progress = { downloaded: 0, total: 0 };
    try {
      await invoke("choose_model", { id: m.id });
      if (!m.installed) await invoke("download_model", { id: m.id });
    } catch (e) {
      started = "";
      showDownloadError(String(e));
    }
    renderDownload();
  }

  function showDownloadError(text: string) {
    const box = $("onb-dl-error");
    box.textContent = text;
    box.hidden = false;
  }

  async function enterSetup() {
    if (started !== selected) await startInstall();
    renderDownload();
    renderKeys();
    renderMic();
  }

  function renderDownload() {
    const m = modelRow(started || selected);
    const name = m?.name ?? "the model";
    const s = status;
    const ready = !!s && s.model === "ready" && s.active === (m?.id ?? "");
    const failed = !!s && s.model === "error" && s.active === (m?.id ?? "");
    const fill = $("onb-prog-fill");
    const bar = $("onb-prog");
    let percent = ready ? 100 : progress.total > 0 ? Math.min(99, (progress.downloaded * 100) / progress.total) : 0;
    if (!ready && m?.installed && !failed) percent = 100;
    fill.style.width = `${percent}%`;
    bar.setAttribute("aria-valuenow", String(Math.round(percent)));

    $("onb-dl-title").textContent = ready ? `${name} is ready` : failed ? "The download stopped" : m?.installed ? `Loading ${name}` : `Downloading ${name}`;
    let left = "";
    let right = "";
    if (ready) {
      left = "All set.";
    } else if (!m?.installed && progress.total > 0) {
      left = `${size(progress.downloaded / 1048576)} of ${size(progress.total / 1048576)}`;
      const seconds = (Date.now() - startedAt) / 1000;
      const rate = progress.downloaded / Math.max(seconds, 1);
      const remaining = rate > 0 ? Math.round((progress.total - progress.downloaded) / rate) : 0;
      right = remaining > 1 ? `About ${remaining >= 90 ? `${Math.round(remaining / 60)} minutes` : `${remaining} seconds`} left` : "Almost done";
    } else if (m?.installed) {
      left = "Already on this PC. Loading it now.";
    }
    $("onb-prog-left").textContent = left;
    $("onb-prog-right").textContent = right;

    if (failed) showDownloadError(s?.detail ?? "The download failed. Check your connection and try again.");
    else if (!s?.detail || ready) $("onb-dl-error").hidden = true;

    toTry.disabled = !(ready || failed);
    toTry.dataset.retry = failed ? "1" : "";
    toTry.textContent = ready ? "Continue" : failed ? "Try the download again" : "Finishing the download…";
  }

  toTry.addEventListener("click", (e) => {
    if (toTry.dataset.retry) {
      e.stopImmediatePropagation();
      started = "";
      void startInstall();
    }
  }, true);

  listen<{ id: string; downloaded: number; total: number }>("download-progress", (e) => {
    if (!isOpen || e.payload.id !== started) return;
    progress = { downloaded: e.payload.downloaded, total: e.payload.total };
    if (step === 3) renderDownload();
  });

  function renderKeys() {
    const keys = status?.hotkey ?? "Ctrl+Shift+Space";
    $("onb-keys").replaceChildren(
      ...keys.split("+").flatMap((k, i) => [...(i ? [document.createTextNode("+")] : []), Object.assign(el("kbd"), { textContent: k })]),
    );
  }

  $("onb-keys-change").addEventListener("click", async () => {
    const box = $("onb-keyopts");
    if (!box.hidden) {
      box.hidden = true;
      return;
    }
    $("onb-key-error").hidden = true;
    const options = await invoke<HotkeyOption[]>("hotkey_options");
    box.replaceChildren(
      ...options
        .filter((o) => o.status !== "taken")
        .map((o) => {
          const b = el("button");
          b.type = "button";
          b.append(Object.assign(el("kbd"), { textContent: o.hotkey }));
          const label = o.status === "current" ? "In use" : o.recommended ? "Recommended" : o.note ? `Note: ${o.note}` : "";
          if (label) b.append(el("small", "", label));
          b.disabled = o.status === "current";
          b.addEventListener("click", async () => {
            try {
              await invoke("set_hotkey", { hotkey: o.hotkey });
              box.hidden = true;
            } catch (e) {
              const err = $("onb-key-error");
              err.textContent = String(e);
              err.hidden = false;
            }
          });
          return b;
        }),
    );
    box.hidden = false;
  });

  function renderMic() {
    const select = $<HTMLSelectElement>("onb-mic");
    const def = status?.default_microphone;
    select.replaceChildren(
      Object.assign(el("option"), { value: "", textContent: def ? `Default (${def})` : "Default" }),
      ...(status?.microphones ?? []).map((m) => Object.assign(el("option"), { value: m, textContent: m })),
    );
    select.value = status?.microphone ?? "";
    $("onb-mic-hint").textContent = status?.mic_warning ?? "You can test it in the next step.";
  }

  $("onb-mic").addEventListener("change", (e) => {
    const value = (e.target as HTMLSelectElement).value;
    invoke("set_microphone", { name: value || null }).catch(() => {});
  });

  // ---- step 4: a first dictation ----------------------------------------

  let transcribingSince = 0;
  let held = false;

  function enterTry() {
    $("onb-try-key").textContent = status?.hotkey ?? "your shortcut";
    const ready = status?.model === "ready";
    hold.disabled = !ready;
    if (!ready) setResult("Finishing the download… the button wakes up when the model is ready.", "", true);
  }

  function setResult(text: string, note: string, empty = false) {
    const res = $("onb-res");
    res.textContent = text;
    res.classList.toggle("empty", empty);
    const n = $("onb-res-note");
    n.replaceChildren();
    if (note) {
      const [lead, ...rest] = note.split("|");
      if (rest.length) n.append(el("span", "ok", lead), document.createTextNode(rest.join("|")));
      else n.textContent = note;
    }
  }

  function press() {
    if (hold.disabled || held) return;
    held = true;
    invoke("press_dictation").catch((e) => {
      held = false;
      setResult("Couldn't start listening.", String(e));
    });
  }
  function release() {
    if (!held) return;
    held = false;
    invoke("release_dictation").catch(() => {});
  }
  hold.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    hold.setPointerCapture(e.pointerId);
    e.preventDefault();
    press();
  });
  hold.addEventListener("pointerup", release);
  hold.addEventListener("pointercancel", release);
  hold.addEventListener("keydown", (e) => {
    if (e.code !== "Space" && e.code !== "Enter") return;
    e.preventDefault();
    if (!e.repeat) press();
  });
  hold.addEventListener("keyup", (e) => {
    if (e.code === "Space" || e.code === "Enter") release();
  });
  window.addEventListener("blur", release);

  listen<DictationEvent>("dictation", ({ payload }) => {
    if (!isOpen || step !== 4) return;
    hold.classList.toggle("recording", payload.phase === "recording");
    if (payload.phase === "recording") setResult("Listening…", "", true);
    else if (payload.phase === "transcribing") {
      transcribingSince = performance.now();
      setResult("Typing it out…", "", true);
    } else if (payload.phase === "done") {
      const seconds = ((performance.now() - transcribingSince) / 1000).toFixed(1);
      const name = modelRow(status?.active ?? "")?.name ?? "the model";
      if (!payload.text.trim()) setResult("I didn't catch anything. Try again, a little closer to the microphone.", "", true);
      else setResult(payload.text, `✓ Done in ${seconds} s|Typed by ${name}, on this PC. No audio left it.`);
    } else setResult("That didn't work.", payload.message);
  });

  // ---- finishing --------------------------------------------------------

  function close() {
    isOpen = false;
    root.hidden = true;
    $<HTMLInputElement>("onb-name").blur();
  }

  async function finish() {
    await invoke("finish_onboarding", { name: nameInput.value.trim() || null, languages: [...languages] }).catch(() => {});
    close();
  }
  $("onb-finish").addEventListener("click", () => void finish());

  skipBtn.addEventListener("click", async () => {
    if (replay) {
      close();
      return;
    }
    // Skipping must still leave a working app: install the recommended model.
    const anyInstalled = status?.models.some((m) => m.installed) ?? false;
    if (!anyInstalled && !started) {
      try {
        const p = await invoke<Plan>("onboarding_plan", { languages: [...languages] });
        await invoke("choose_model", { id: p.primary.id });
        await invoke("download_model", { id: p.primary.id });
      } catch {
        // The main window shows the model state and offers the download again.
      }
    }
    await finish();
  });

  // ---- entry points -----------------------------------------------------

  function open(again: boolean) {
    replay = again;
    isOpen = true;
    started = "";
    plan = null;
    showAll = false;
    languages.clear();
    (status?.languages.length ? status.languages : again ? [] : ["english"]).forEach((l) => languages.add(l));
    nameInput.value = status?.name ?? "";
    syncChips();
    selected = again ? (status?.active ?? "") : "";
    skipBtn.textContent = again ? "Close" : "Skip setup";
    root.hidden = false;
    go(again ? 1 : 0);
  }

  return {
    /** Called with every status the app gets; opens setup on a fresh install. */
    update(s: OnboardingStatus) {
      status = s;
      if (!isOpen) {
        if (s.onboarding) open(false);
        return;
      }
      if (step === 3) {
        renderDownload();
        renderMic();
      } else if (step === 4) {
        hold.disabled = s.model !== "ready";
      }
      if (step === 2 && plan) renderOptions();
    },
    /** Settings > Run setup again. */
    replay() {
      if (status) open(true);
    },
  };
}
