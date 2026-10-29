// Audio on disk: how much there is, a reminder when it grows, how long to
// keep it, and a way to free space by zipping, moving or deleting it. The
// words of every dictation stay whatever is chosen here.

import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

export interface ArchiveInfo {
  path: string;
  kind: "zip" | "folder";
  count: number;
  bytes: number;
  present: boolean;
  from_ms: number;
  to_ms: number;
}

export interface AudioUsage {
  bytes: number;
  recordings: number;
  entries: number;
  days: number;
  archives: ArchiveInfo[];
}

interface Candidates {
  count: number;
  bytes: number;
  kept: number;
}

interface Archived {
  count: number;
  bytes_before: number;
  bytes_after: number;
  path: string;
}

export interface StorageSettings {
  audio_limit_mb: number | null;
  keep_audio_days: number | null;
}

interface Hooks {
  askConfirm: (title: string, body: string, action?: string) => Promise<boolean>;
  settings: () => StorageSettings | null;
  /** Called after audio changed, so the history can redraw. */
  changed: () => void;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const MB = 1024 * 1024;
const LIMITS: { label: string; mb: number | null }[] = [
  { label: "250 MB", mb: 250 },
  { label: "500 MB", mb: 500 },
  { label: "1 GB", mb: 1000 },
  { label: "2 GB", mb: 2000 },
  { label: "Never", mb: null },
];
const KEEP: { name: string; note: string; days: number | null }[] = [
  { name: "Until 1,000 dictations", note: "How it has always worked: older audio is dropped after the newest 1,000.", days: null },
  { name: "90 days", note: "A quarter of replayable history.", days: 90 },
  { name: "30 days", note: "Enough to fix a mishearing from last week.", days: 30 },
  { name: "7 days", note: "Light on disk, still lets you correct this week.", days: 7 },
  { name: "Don't keep audio", note: "Words only. Smallest and most private. No Play or Try another model.", days: 0 },
];

let hooks: Hooks;
export let usage: AudioUsage | null = null;

export function fmtBytes(bytes: number) {
  if (bytes >= 1024 * MB) return `${(bytes / (1024 * MB)).toFixed(1)} GB`;
  if (bytes >= MB) return `${Math.round(bytes / MB)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

/** Re-reads how much audio there is and redraws what depends on it. */
export async function refreshUsage(changed = false) {
  try {
    usage = await invoke<AudioUsage>("audio_usage");
  } catch {
    return;
  }
  render();
  if (changed) hooks.changed();
}

function snoozedUntil() {
  try {
    return Number(localStorage.getItem("audioNudgeUntil") ?? 0);
  } catch {
    return 0;
  }
}

/** Draws the Settings card and the reminder on Home from the latest numbers. */
export function render() {
  const s = hooks.settings();
  if (!usage || !s) return;
  const limit = s.audio_limit_mb;
  const over = limit !== null && usage.bytes >= limit * MB;

  // Home reminder: only past the limit, and not while snoozed.
  const nudge = $("audio-nudge");
  nudge.hidden = !(over && Date.now() > snoozedUntil());
  $("nudge-text").replaceChildren(
    el("strong", "", `Audio is using ${fmtBytes(usage.bytes)}`),
    document.createTextNode(`, past the ${fmtBytes((limit ?? 0) * MB)} you set. Your words stay whatever you choose.`),
  );

  // Settings card.
  $("store-meta").textContent = `${fmtBytes(usage.bytes)} · ${plural(usage.entries, "dictation")} · ${plural(usage.days, "day")}`;
  const scale = Math.max(usage.bytes, (limit ?? 0) * MB, MB) * (limit !== null ? 1.4 : 1.1);
  $("store-fill").style.width = `${Math.min(100, (usage.bytes / scale) * 100)}%`;
  const mark = $("store-mark");
  mark.hidden = limit === null;
  if (limit !== null) mark.style.left = `${Math.min(100, ((limit * MB) / scale) * 100)}%`;
  $("store-bar").classList.toggle("over", over);
  $("store-legend").textContent =
    limit === null
      ? `${plural(usage.recordings, "recording")} on disk · no reminder`
      : `${plural(usage.recordings, "recording")} on disk · reminder at ${fmtBytes(limit * MB)}`;

  $("limit-seg")
    .querySelectorAll<HTMLButtonElement>("button")
    .forEach((b, i) => b.setAttribute("aria-pressed", String(LIMITS[i].mb === limit)));
  $("keep-list")
    .querySelectorAll<HTMLInputElement>("input")
    .forEach((r, i) => (r.checked = KEEP[i].days === s.keep_audio_days));

  const list = $("archive-list");
  list.hidden = usage.archives.length === 0;
  $("archive-title").hidden = usage.archives.length === 0;
  list.replaceChildren(
    ...usage.archives.map((a) => {
      const li = el("li", "archive-item");
      const text = el("span", "archive-text");
      const day = (ms: number) => new Date(ms).toLocaleDateString([], { month: "short", day: "numeric" });
      const span = day(a.from_ms) === day(a.to_ms) ? day(a.from_ms) : `${day(a.from_ms)} to ${day(a.to_ms)}`;
      text.append(
        el("b", "", a.kind === "zip" ? (a.path.split(/[\\/]/).pop() ?? a.path) : a.path),
        el(
          "span",
          "muted small",
          `${plural(a.count, "recording")} · ${span} · ${fmtBytes(a.bytes)} · ${a.kind === "zip" ? "can't be played until restored" : "plays while the folder is available"}${a.present ? "" : " · not found"}`,
        ),
      );
      const restore = el("button", "ghost small-btn", "Restore");
      restore.type = "button";
      restore.disabled = !a.present;
      restore.addEventListener("click", async () => {
        restore.disabled = true;
        try {
          const n = await invoke<number>("restore_audio", { location: a.path });
          say(`Restored ${plural(n, "recording")}.`);
        } catch (e) {
          say(String(e), true);
        }
        await refreshUsage(true);
      });
      const show = el("button", "ghost small-btn", "Show");
      show.type = "button";
      show.disabled = !a.present;
      show.addEventListener("click", () => void revealItemInDir(a.path).catch(() => undefined));
      li.append(text, restore, show);
      return li;
    }),
  );
}

function say(message: string, bad = false) {
  const line = $("store-status");
  line.textContent = message;
  line.classList.toggle("bad", bad);
  line.hidden = !message;
}

// ---------------------------------------------------------------------------
// The "Free up space" dialog

const dialog = $<HTMLDialogElement>("space-dialog");
const daysSelect = $<HTMLSelectElement>("space-days");
type Action = "zip" | "move" | "delete";
let action: Action = "zip";
let plan: Candidates = { count: 0, bytes: 0, kept: 0 };
let finished = false;
let working = false;

const ACTIONS: { id: Action; name: string; note: string; cost: string; danger: boolean }[] = [
  {
    id: "zip",
    name: "Zip into one file",
    note: "Packs these recordings into a .zip in Documents\\Sruthi, checks it, then removes the originals. Roughly half the size.",
    cost: "They can't be played until you restore them.",
    danger: false,
  },
  {
    id: "move",
    name: "Move to another folder or drive",
    note: "Copies them somewhere else, such as an external drive, checks each copy, then removes the originals.",
    cost: "They play while that folder is available.",
    danger: false,
  },
  {
    id: "delete",
    name: "Delete the audio",
    note: "Removes the recordings but keeps what was said, so Home, search and stats are unchanged.",
    cost: "This can't be undone. No Play or Try another model for these.",
    danger: true,
  },
];

function freed(a: Action) {
  return a === "zip" ? plan.bytes / 2 : plan.bytes;
}

function drawOptions() {
  $("space-options").replaceChildren(
    ...ACTIONS.map((a) => {
      const label = el("label", "space-option");
      label.classList.toggle("selected", a.id === action);
      label.classList.toggle("danger", a.danger);
      const radio = el("input");
      radio.type = "radio";
      radio.name = "space-action";
      radio.checked = a.id === action;
      radio.addEventListener("change", () => {
        action = a.id;
        drawOptions();
      });
      const body = el("span", "space-option-body");
      body.append(el("b", "", a.name), el("span", "muted small", a.note), el("span", a.danger ? "space-cost danger" : "space-cost", a.cost));
      const frees = el("span", "space-frees", plan.count ? `frees ${a.id === "zip" ? "≈ " : ""}${fmtBytes(freed(a.id))}` : "");
      label.append(radio, body, frees);
      return label;
    }),
  );
  $("space-folder").hidden = action !== "move";
  const go = $<HTMLButtonElement>("space-go");
  const n = plural(plan.count, "recording");
  go.textContent = finished ? "Done" : action === "zip" ? `Zip ${n}` : action === "move" ? `Move ${n}` : `Delete ${n}`;
  go.classList.toggle("danger", action === "delete" && !finished);
  go.disabled = working || (!finished && (plan.count === 0 || (action === "move" && !$<HTMLInputElement>("space-path").value.trim())));
  $<HTMLButtonElement>("space-cancel").hidden = finished;
}

async function refreshPlan() {
  const days = Number(daysSelect.value);
  try {
    plan = await invoke<Candidates>("audio_candidates", { days });
  } catch {
    plan = { count: 0, bytes: 0, kept: 0 };
  }
  $("space-count").textContent = plan.count
    ? `${plural(plan.count, "recording")} · ${fmtBytes(plan.bytes)}`
    : "nothing that old";
  $("space-kept").hidden = plan.kept === 0;
  $("space-kept").textContent = `${plural(plan.kept, "dictation")} you corrected keep their audio, because the speed test on Models uses it as the answer key.`;
  drawOptions();
}

function spaceStatus(message: string, bad = false) {
  const line = $("space-status");
  line.textContent = message;
  line.classList.toggle("bad", bad);
}

export function openFreeSpace() {
  finished = false;
  working = false;
  spaceStatus("Nothing changes until you press the button.");
  $("space-sub").textContent = usage
    ? `Audio is using ${fmtBytes(usage.bytes)}. Your words, search and stats stay whatever you choose.`
    : "Your words, search and stats stay whatever you choose.";
  dialog.showModal();
  void refreshPlan();
}

daysSelect.addEventListener("change", () => void refreshPlan());
$("space-path").addEventListener("input", drawOptions);
$("space-browse").addEventListener("click", async () => {
  try {
    const picked = await invoke<string | null>("choose_folder");
    if (picked) {
      $<HTMLInputElement>("space-path").value = picked;
      drawOptions();
    }
  } catch (e) {
    spaceStatus(String(e), true);
  }
});
$("space-cancel").addEventListener("click", () => dialog.close());
$("space-go").addEventListener("click", async () => {
  if (finished) {
    dialog.close();
    return;
  }
  const days = Number(daysSelect.value);
  working = true;
  drawOptions();
  spaceStatus(action === "delete" ? "Deleting…" : action === "zip" ? "Zipping and checking the zip…" : "Copying and checking each file…");
  try {
    let done: string;
    if (action === "zip") {
      const r = await invoke<Archived>("zip_audio", { days });
      done = `Zipped ${plural(r.count, "recording")} from ${fmtBytes(r.bytes_before)} to ${fmtBytes(r.bytes_after)}: ${r.path}`;
    } else if (action === "move") {
      const r = await invoke<Archived>("move_audio", { days, folder: $<HTMLInputElement>("space-path").value });
      done = `Moved ${plural(r.count, "recording")} (${fmtBytes(r.bytes_before)}) to ${r.path}`;
    } else {
      const r = await invoke<Candidates>("delete_audio", { days });
      done = `Deleted the audio of ${plural(r.count, "recording")} and freed ${fmtBytes(r.bytes)}.`;
    }
    finished = true;
    spaceStatus(done);
  } catch (e) {
    spaceStatus(String(e), true);
  } finally {
    working = false;
    await refreshUsage(true);
    await refreshPlan();
  }
});

// ---------------------------------------------------------------------------
// Settings controls and the Home reminder

async function setKeep(days: number | null) {
  if (days !== null) {
    let affected: Candidates;
    try {
      affected = await invoke<Candidates>("audio_candidates", { days });
    } catch {
      affected = { count: 0, bytes: 0, kept: 0 };
    }
    if (affected.count > 0) {
      const what = days === 0 ? "all of it" : `everything older than ${plural(days, "day")}`;
      const ok = await hooks.askConfirm(
        days === 0 ? "Stop keeping audio?" : `Keep audio for ${plural(days, "day")}?`,
        `This deletes the audio of ${plural(affected.count, "recording")} (${what}, ${fmtBytes(affected.bytes)}). Their words stay. This can't be undone.`,
        "Delete audio",
      );
      if (!ok) {
        render();
        return;
      }
    }
  }
  try {
    await invoke("set_keep_audio", { days });
  } catch (e) {
    say(String(e), true);
  }
  await refreshUsage(true);
}

export function initStorage(h: Hooks) {
  hooks = h;
  $("limit-seg").replaceChildren(
    ...LIMITS.map((l) => {
      const b = el("button", "seg-btn", l.label);
      b.type = "button";
      b.addEventListener("click", async () => {
        await invoke("set_audio_limit", { mb: l.mb });
        // The Home reminder comes back when the level changes.
        try {
          localStorage.removeItem("audioNudgeUntil");
        } catch {
          // Optional.
        }
        render();
      });
      return b;
    }),
  );
  $("keep-list").replaceChildren(
    ...KEEP.map((k) => {
      const row = el("label", "keep-row");
      const radio = el("input");
      radio.type = "radio";
      radio.name = "keep-audio";
      radio.addEventListener("change", () => void setKeep(k.days));
      const text = el("span", "keep-text");
      text.append(el("b", "", k.name), el("span", "muted small", k.note));
      row.append(radio, text);
      return row;
    }),
  );
  $("free-space-open").addEventListener("click", openFreeSpace);
  $("nudge-free").addEventListener("click", openFreeSpace);
  $("nudge-later").addEventListener("click", () => {
    try {
      localStorage.setItem("audioNudgeUntil", String(Date.now() + 7 * 86_400_000));
    } catch {
      // Optional.
    }
    render();
  });
  $("empty-audio").addEventListener("click", async () => {
    let all: Candidates;
    try {
      all = await invoke<Candidates>("audio_candidates", { days: 0 });
    } catch {
      return;
    }
    if (all.count === 0) {
      say("There is no audio in the recordings folder to delete.");
      return;
    }
    const ok = await h.askConfirm(
      "Empty all audio?",
      `This deletes the audio of ${plural(all.count, "recording")} (${fmtBytes(all.bytes)}). Words, search and stats stay${all.kept ? `, and ${plural(all.kept, "corrected dictation")} keep their audio` : ""}. This can't be undone.`,
      "Delete audio",
    );
    if (!ok) return;
    try {
      const r = await invoke<Candidates>("delete_audio", { days: 0 });
      say(`Deleted the audio of ${plural(r.count, "recording")} and freed ${fmtBytes(r.bytes)}.`);
    } catch (e) {
      say(String(e), true);
    }
    await refreshUsage(true);
  });
}
