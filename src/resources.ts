// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type ThermalState = "normal" | "warm" | "cooling";

export interface ResourceSnapshot {
  supported: boolean;
  source: string;
  note: string | null;
  sampled_at_ms: number;
  gpu: {
    name: string | null;
    load_pct: number | null;
    process_pct: number | null;
    engines: { name: string; pct: number }[];
  } | null;
  vram: {
    used_mb: number | null;
    total_mb: number | null;
    process_mb: number | null;
  } | null;
  cpu: {
    load_pct: number | null;
    threads: number;
    clock_pct: number;
  } | null;
  ram: {
    used_mb: number | null;
    total_mb: number | null;
    process_mb: number | null;
  } | null;
  history: ResourceSample[];
  thermal: {
    state: ThermalState;
    load: number;
    clock_pct: number;
    cooling_left_s: number;
    message: string | null;
    guard: boolean;
  };
}

interface ResourceSample {
  ago_s: number;
  gpu_pct: number | null;
  vram_pct: number | null;
  cpu_pct: number | null;
  ram_pct: number | null;
}

export interface ResourceStatus {
  activeModel: string | null;
  modelReady: boolean;
  engineOnGpu: boolean;
  estimateSecsPerMin: number | null;
  estimateMeasured: boolean;
  estimateRuns: number;
  heat: "light" | "moderate" | "heavy";
}

interface RecordedRun {
  at: number;
  model: string | null;
  duration_s: number;
  gpu: number | null;
  vram_mb: number | null;
  ram_mb: number | null;
  cpu: number | null;
}

interface ActiveRun {
  started: number;
  model: string | null;
  gpu: number | null;
  vram_mb: number | null;
  ram_mb: number | null;
  cpu: number | null;
}

type DictationEvent =
  | { phase: "recording" }
  | { phase: "transcribing" }
  | { phase: "done"; text: string }
  | { phase: "error"; message: string };

const RUNS_KEY = "sruthi.performance.runs.v1";
const MAX_RUNS = 20;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T | null;

let latest: ResourceSnapshot | null = null;
let status: ResourceStatus = {
  activeModel: null,
  modelReady: false,
  engineOnGpu: false,
  estimateSecsPerMin: null,
  estimateMeasured: false,
  estimateRuns: 0,
  heat: "moderate",
};
let page = "home";
let paused = false;
let runs: RecordedRun[] = loadRuns();
let currentRun: ActiveRun | null = null;
let chartHistory: ResourceSample[] = [];
let lastChartPointer: { clientX: number; clientY: number } | null = null;
let settingsQuery = "";
let settingsFilter = "all";
let polling: number | null = null;

function loadRuns(): RecordedRun[] {
  try {
    const raw = localStorage.getItem(RUNS_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((run): run is RecordedRun => {
        if (!run || typeof run !== "object") return false;
        const item = run as Partial<RecordedRun>;
        return (
          typeof item.at === "number" &&
          typeof item.duration_s === "number" &&
          (item.model === null || typeof item.model === "string") &&
          isNumberOrNull(item.gpu) &&
          isNumberOrNull(item.vram_mb) &&
          isNumberOrNull(item.ram_mb) &&
          isNumberOrNull(item.cpu)
        );
      })
      .slice(0, MAX_RUNS);
  } catch {
    return [];
  }
}

function isNumberOrNull(value: unknown): value is number | null {
  return value === null || (typeof value === "number" && Number.isFinite(value));
}

function saveRuns() {
  try {
    localStorage.setItem(RUNS_KEY, JSON.stringify(runs.slice(0, MAX_RUNS)));
  } catch {
    // Storage can be blocked; the live page still works.
  }
}

function text(id: string, value: string) {
  const node = $(id);
  if (node) node.textContent = value;
}

function width(id: string, pctValue: number | null) {
  const node = $(id);
  if (node) node.style.width = pctValue === null ? "0%" : `${Math.max(0, Math.min(100, pctValue))}%`;
}

function metricText(id: string, value: number | null, unit = "%") {
  text(id, value === null ? "—" : String(Math.round(value)));
  const unitNode = $(`${id}-unit`);
  if (unitNode) unitNode.textContent = value === null ? "" : unit;
}

function pctText(value: number | null | undefined, digits = 0) {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  return `${value.toFixed(digits)}%`;
}

function gbValue(mb: number | null | undefined) {
  if (mb === null || mb === undefined || !Number.isFinite(mb)) return "—";
  const gb = mb / 1024;
  return gb >= 10 ? gb.toFixed(1).replace(/\.0$/, "") : gb.toFixed(1);
}

function gbPair(used: number | null | undefined, total: number | null | undefined) {
  if (used === null || used === undefined) return "—";
  if (total === null || total === undefined) return gbValue(used);
  return `${gbValue(used)} / ${gbValue(total)}`;
}

function secondsText(seconds: number) {
  if (seconds >= 60) return `${(seconds / 60).toFixed(1).replace(/\.0$/, "")} min`;
  return `${seconds.toFixed(1).replace(/\.0$/, "")} s`;
}

function timeAgo(ms: number) {
  const seconds = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return seconds <= 1 ? "now" : `${seconds}s ago`;
}

function historyValues(key: keyof Omit<ResourceSample, "ago_s">, history = latest?.history ?? []) {
  return history.map((sample) => sample[key]);
}

function numbers(values: (number | null)[]) {
  return values.filter((value): value is number => value !== null && Number.isFinite(value));
}

function peak(values: (number | null)[]) {
  const present = numbers(values);
  return present.length ? Math.max(...present) : null;
}

function lastIndex(values: (number | null)[]) {
  for (let i = values.length - 1; i >= 0; i -= 1) {
    const value = values[i];
    if (value !== null && Number.isFinite(value)) return i;
  }
  return -1;
}

function setPill(id: string, label: string, tone: "ok" | "warn" | "off" = "ok") {
  const pill = $(id);
  if (!pill) return;
  pill.dataset.tone = tone;
  const labelNode = pill.querySelector("[data-pill-label]");
  if (labelNode) labelNode.textContent = label;
}

function updateLiveLabel() {
  if (!latest) {
    setPill("perf-live", "Waiting for counters", "off");
    setPill("pulse-live", "Waiting", "off");
    return;
  }
  if (!latest.supported) {
    setPill("perf-live", "Unavailable", "off");
    setPill("pulse-live", "Unavailable", "off");
    return;
  }
  if (paused) {
    setPill("perf-live", "Paused", "warn");
    setPill("pulse-live", "Paused", "warn");
    return;
  }
  const age = Date.now() - latest.sampled_at_ms;
  if (age > 4000) {
    setPill("perf-live", `Stale ${Math.round(age / 1000)}s`, "warn");
    setPill("pulse-live", "Stale", "warn");
    return;
  }
  setPill("perf-live", "Live", "ok");
  setPill("pulse-live", "Live", "ok");
}

function sparkPath(values: (number | null)[]) {
  const points: { x: number; y: number }[] = [];
  values.forEach((value, index) => {
    if (value === null || !Number.isFinite(value)) return;
    const denominator = Math.max(1, values.length - 1);
    points.push({
      x: (index / denominator) * 100,
      y: 26 - Math.max(0, Math.min(100, value)) * 0.22,
    });
  });
  if (points.length < 2) return "";
  return points.map((point, index) => `${index === 0 ? "M" : "L"}${point.x.toFixed(2)},${point.y.toFixed(2)}`).join(" ");
}

function drawSpark(kind: "gpu" | "vram" | "cpu" | "ram", values: (number | null)[]) {
  const path = sparkPath(values);
  document.querySelectorAll<HTMLElement>(`[data-spark="${kind}"]`).forEach((host) => {
    if (!path) {
      host.innerHTML = `<span class="spark-empty">—</span>`;
      return;
    }
    host.innerHTML = `<svg viewBox="0 0 100 28" preserveAspectRatio="none" aria-hidden="true"><path d="${path}" /></svg>`;
  });
}

function drawAllSparks(snapshot: ResourceSnapshot) {
  drawSpark("gpu", historyValues("gpu_pct", snapshot.history));
  drawSpark("vram", historyValues("vram_pct", snapshot.history));
  drawSpark("cpu", historyValues("cpu_pct", snapshot.history));
  drawSpark("ram", historyValues("ram_pct", snapshot.history));
}

function renderMetrics(snapshot: ResourceSnapshot) {
  const gpuPeak = peak(historyValues("gpu_pct", snapshot.history));
  const cpuPeak = peak(historyValues("cpu_pct", snapshot.history));
  const vramPeak = peak(historyValues("vram_pct", snapshot.history));
  const ramPeak = peak(historyValues("ram_pct", snapshot.history));
  const vramUsed = snapshot.vram?.used_mb ?? null;
  const vramTotal = snapshot.vram?.total_mb ?? null;
  const ramUsed = snapshot.ram?.used_mb ?? null;
  const ramTotal = snapshot.ram?.total_mb ?? null;
  const vramFree = vramUsed !== null && vramTotal !== null ? vramTotal - vramUsed : null;
  const ramFree = ramUsed !== null && ramTotal !== null ? ramTotal - ramUsed : null;
  const waiting = snapshot.supported ? "Waiting for samples" : "Not measured here";

  metricText("perf-gpu", snapshot.gpu?.load_pct ?? null);
  text(
    "perf-gpu-sub",
    snapshot.gpu
      ? `${snapshot.gpu.name ?? "Graphics adapter"}${snapshot.gpu.process_pct === null ? "" : ` · Sruthi ${pctText(snapshot.gpu.process_pct)}`}`
      : snapshot.supported
        ? "GPU engine counters are unavailable on this PC."
        : "Live GPU counters are unavailable here.",
  );
  text(
    "perf-gpu-foot",
    gpuPeak === null ? waiting : `Peak ${pctText(gpuPeak)} · ${snapshot.history.length}s window`,
  );

  text("perf-vram", gbPair(snapshot.vram?.used_mb, snapshot.vram?.total_mb));
  text("perf-vram-unit", snapshot.vram?.used_mb === null || snapshot.vram?.used_mb === undefined ? "" : "GB");
  text(
    "perf-vram-sub",
    snapshot.vram?.process_mb !== null && snapshot.vram?.process_mb !== undefined
      ? `Sruthi ${gbValue(snapshot.vram.process_mb)} GB · dedicated memory`
      : "Dedicated video memory",
  );
  text(
    "perf-vram-foot",
    vramPeak === null
      ? waiting
      : `Peak ${pctText(vramPeak)}${vramFree === null ? "" : ` · ${gbValue(vramFree)} GB free`}`,
  );

  metricText("perf-cpu", snapshot.cpu?.load_pct ?? null);
  text(
    "perf-cpu-sub",
    snapshot.cpu
      ? `${snapshot.cpu.threads.toLocaleString()} logical · ${snapshot.cpu.clock_pct}% effective clock`
      : "CPU counters are unavailable here.",
  );
  text(
    "perf-cpu-foot",
    cpuPeak === null ? waiting : `Peak ${pctText(cpuPeak)} · ${snapshot.cpu?.clock_pct ?? "—"}% clock`,
  );

  text("perf-ram", gbPair(snapshot.ram?.used_mb, snapshot.ram?.total_mb));
  text("perf-ram-unit", snapshot.ram?.used_mb === null || snapshot.ram?.used_mb === undefined ? "" : "GB");
  text(
    "perf-ram-sub",
    snapshot.ram?.process_mb !== null && snapshot.ram?.process_mb !== undefined
      ? `Sruthi ${gbValue(snapshot.ram.process_mb)} GB · system memory`
      : "System memory",
  );
  text(
    "perf-ram-foot",
    ramPeak === null
      ? waiting
      : `Peak ${pctText(ramPeak)}${ramFree === null ? "" : ` · ${gbValue(ramFree)} GB free`}`,
  );

  text(
    "perf-chart-peak",
    gpuPeak === null ? "Peak GPU —" : `Peak GPU ${pctText(gpuPeak)}`,
  );
  text("perf-chart-window", `${snapshot.history.length}s in memory`);
  drawAllSparks(snapshot);
}

function barRow(id: string, value: number | null) {
  const node = $(id);
  if (node) node.style.width = value === null ? "0%" : `${Math.max(0, Math.min(100, value))}%`;
}

function renderEngines(snapshot: ResourceSnapshot) {
  const load = snapshot.gpu?.load_pct ?? null;
  const own = snapshot.gpu?.process_pct ?? null;
  const others = load === null ? null : Math.max(0, load - (own ?? 0));
  barRow("perf-engine-process", own);
  barRow("perf-engine-others", others);
  text("perf-engine-total", load === null ? "—" : `${pctText(load)} total`);
  text("perf-engine-own", own === null ? "—" : `${pctText(own)} Sruthi`);
  text("perf-engine-other", others === null ? "—" : `${pctText(others)} other apps`);

  const engines = snapshot.gpu?.engines ?? [];
  for (const engine of engines) {
    const key = engine.name.toLowerCase();
    barRow(`engine-${key}-bar`, engine.pct);
    text(`engine-${key}`, pctText(engine.pct));
  }
  text(
    "perf-engine-note",
    snapshot.gpu
      ? "Engine percentages can overlap. They are a split of activity, not a second total."
      : snapshot.supported
        ? "Windows did not report a GPU engine counter for this adapter."
        : "GPU engine counters are unavailable on this platform.",
  );
}

function renderHeat(snapshot: ResourceSnapshot) {
  const thermal = snapshot.thermal;
  const pressure = Math.min(100, Math.max(thermal.load, 100 - thermal.clock_pct));
  const gauge = $("perf-heat-gauge");
  if (gauge) gauge.style.setProperty("--p", `${pressure}%`);
  text("perf-heat-pct", `${pressure}% pressure`);
  text("perf-heat-load", `${thermal.load}%`);
  text("perf-heat-clock", `${thermal.clock_pct}%`);
  text(
    "perf-heat-cooldown",
    thermal.cooling_left_s > 0 ? `${thermal.cooling_left_s}s left` : thermal.state === "cooling" ? "Cooling" : "None",
  );
  const toggle = $<HTMLInputElement>("perf-thermal-toggle");
  if (toggle) toggle.checked = thermal.guard;
  const state = thermal.guard ? thermal.state : "off";
  setPill("perf-heat-pill", state === "off" ? "Off" : state[0].toUpperCase() + state.slice(1), state === "normal" ? "ok" : state === "off" ? "off" : "warn");
  text(
    "perf-heat-message",
    thermal.message ??
      (thermal.guard
        ? "Windows does not expose a temperature sensor to normal apps. Load and effective clock are the measured proxy."
        : "The heat guard is off. Sruthi will not stop a long, heavy run."),
  );
}

function renderRuntime() {
  const chip = $("perf-runtime-chip");
  const model = status.activeModel;
  if (chip) {
    chip.textContent = !model ? "No model" : !status.modelReady ? "Not ready" : status.engineOnGpu ? "On the card" : "On CPU";
    chip.dataset.tone = !model ? "off" : !status.modelReady ? "warn" : status.engineOnGpu ? "ok" : "off";
  }
  text("perf-runtime-model", model ?? "No speech model selected");
  text(
    "perf-runtime-sub",
    !model
      ? "Pick a model on Models to see its runtime."
      : !status.modelReady
        ? "The model is not ready yet. Its measured speed appears after a run."
        : status.engineOnGpu
          ? "Running on the graphics card selected for dictation."
          : "Running on the CPU.",
  );
  const estimate = status.estimateSecsPerMin;
  text(
    "perf-runtime-speed",
    estimate !== null && estimate > 0 ? `${(60 / estimate).toFixed(1)}× realtime` : "—",
  );
  text("perf-runtime-wait", estimate !== null && estimate > 0 ? `${Math.round(estimate)} ms` : "—");
  text("perf-runtime-runs", status.estimateMeasured ? status.estimateRuns.toLocaleString() : "0");
  text(
    "perf-runtime-source",
    status.estimateMeasured
      ? "Measured from completed runs on this PC."
      : "Fallback estimate; no measured runs yet.",
  );
  renderRuntimeBars();
  updateRunLabels();
}

function renderRuntimeBars() {
  const host = $("perf-runtime-bars");
  if (!host) return;
  if (!runs.length) {
    host.innerHTML = `<p class="muted small runtime-empty">Finish a dictation to see its wait next to the measured estimate.</p>`;
    return;
  }
  const recent = runs.slice(0, 10).reverse();
  const maxDuration = Math.max(...recent.map((run) => run.duration_s), 1);
  host.innerHTML = recent
    .map((run) => {
      const height = Math.max(8, Math.round((run.duration_s / maxDuration) * 100));
      return `<span class="runtime-bar" title="${secondsText(run.duration_s)} · ${timeAgo(run.at)}" aria-label="${secondsText(run.duration_s)}"><i style="height:${height}%"></i></span>`;
    })
    .join("");
}

function formatWhen(ms: number) {
  const date = new Date(ms);
  const today = new Date();
  const sameDay =
    date.getFullYear() === today.getFullYear() &&
    date.getMonth() === today.getMonth() &&
    date.getDate() === today.getDate();
  const time = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return sameDay ? time : `${date.toLocaleDateString([], { month: "short", day: "numeric" })} ${time}`;
}

function renderRuns() {
  const body = $("perf-runs");
  if (!body) return;
  if (!runs.length) {
    const row = document.createElement("tr");
    const cell = document.createElement("td");
    cell.colSpan = 6;
    cell.className = "muted small runs-empty";
    cell.textContent = "No runs recorded in this session yet. Peaks appear after you finish a dictation.";
    row.append(cell);
    body.replaceChildren(row);
  } else {
    const rows = runs.map((run) => {
      const row = document.createElement("tr");
      const values = [
        formatWhen(run.at),
        run.model ?? "Unknown",
        secondsText(run.duration_s),
        pctText(run.gpu),
        run.vram_mb === null ? "—" : `${gbValue(run.vram_mb)} GB`,
        run.ram_mb === null ? "—" : `${gbValue(run.ram_mb)} GB`,
      ];
      for (const value of values) {
        const cell = document.createElement("td");
        cell.textContent = value;
        row.append(cell);
      }
      return row;
    });
    body.replaceChildren(...rows);
  }
  updateRunLabels();
}

function updateRunLabels() {
  text("perf-chart-run", runs[0] ? `Last run ${secondsText(runs[0].duration_s)}` : "No run recorded yet");
}

function renderHomePulse(snapshot: ResourceSnapshot) {
  const gpu = snapshot.gpu?.load_pct ?? null;
  const cpu = snapshot.cpu?.load_pct ?? null;
  const vramPct =
    snapshot.vram?.used_mb !== null &&
    snapshot.vram?.used_mb !== undefined &&
    snapshot.vram?.total_mb !== null &&
    snapshot.vram?.total_mb !== undefined &&
    snapshot.vram.total_mb > 0
      ? (snapshot.vram.used_mb / snapshot.vram.total_mb) * 100
      : null;
  const ramPct =
    snapshot.ram?.used_mb !== null &&
    snapshot.ram?.used_mb !== undefined &&
    snapshot.ram?.total_mb !== null &&
    snapshot.ram?.total_mb !== undefined &&
    snapshot.ram.total_mb > 0
      ? (snapshot.ram.used_mb / snapshot.ram.total_mb) * 100
      : null;

  text("pulse-gpu", gpu === null ? "—" : pctText(gpu));
  text("pulse-cpu", cpu === null ? "—" : pctText(cpu));
  text("pulse-vram", snapshot.vram?.used_mb === null || snapshot.vram?.used_mb === undefined ? "—" : `${gbValue(snapshot.vram.used_mb)}G`);
  text("pulse-ram", snapshot.ram?.used_mb === null || snapshot.ram?.used_mb === undefined ? "—" : `${gbValue(snapshot.ram.used_mb)}G`);
  width("pulse-gpu-bar", gpu);
  width("pulse-cpu-bar", cpu);
  width("pulse-vram-bar", vramPct);
  width("pulse-ram-bar", ramPct);
  text(
    "pulse-foot",
    snapshot.supported
      ? "1 s samples · measured on this PC"
      : "Live counters are unavailable on this platform",
  );
}

function renderHealth(snapshot: ResourceSnapshot) {
  const gpu = snapshot.gpu?.load_pct ?? null;
  const cpu = snapshot.cpu?.load_pct ?? null;
  text("health-gpu", gpu === null ? "—" : pctText(gpu));
  text(
    "health-gpu-sub",
    snapshot.gpu?.name ?? (snapshot.supported ? "GPU counters unavailable" : "Not measured here"),
  );
  text("health-vram", gbPair(snapshot.vram?.used_mb, snapshot.vram?.total_mb));
  text(
    "health-vram-sub",
    snapshot.vram?.process_mb !== null && snapshot.vram?.process_mb !== undefined
      ? `Sruthi ${gbValue(snapshot.vram.process_mb)} GB`
      : "Dedicated memory",
  );
  text("health-cpu", cpu === null ? "—" : pctText(cpu));
  text(
    "health-cpu-sub",
    snapshot.cpu ? `${snapshot.cpu.threads.toLocaleString()} logical · ${snapshot.cpu.clock_pct}% clock` : "Not measured here",
  );
  text("health-ram", gbPair(snapshot.ram?.used_mb, snapshot.ram?.total_mb));
  text(
    "health-ram-sub",
    snapshot.ram?.process_mb !== null && snapshot.ram?.process_mb !== undefined
      ? `Sruthi ${gbValue(snapshot.ram.process_mb)} GB`
      : "System memory",
  );
  text("health-guard", snapshot.thermal.guard ? "Heat guard watching" : "Heat guard off");
  text("health-clock", `${snapshot.thermal.clock_pct}% effective clock`);
  text(
    "health-cooldown",
    snapshot.thermal.cooling_left_s > 0 ? `${snapshot.thermal.cooling_left_s}s cooldown` : "No cooldown",
  );
  text("health-source", snapshot.supported ? "Measured locally · never uploaded" : "No sample values shown");
  const state = !snapshot.thermal.guard
    ? "Guard off"
    : snapshot.thermal.state === "cooling"
      ? "Cooling"
      : snapshot.thermal.state === "warm"
        ? "Warm"
        : "All normal";
  setPill(
    "health-pill",
    state,
    !snapshot.thermal.guard || snapshot.thermal.state === "normal" ? "ok" : "warn",
  );
  drawAllSparks(snapshot);
}

function renderSource(snapshot: ResourceSnapshot) {
  const bits = [snapshot.source];
  if (snapshot.supported) bits.push("sampled once a second", "held in memory only");
  if (snapshot.note) bits.push(snapshot.note);
  text("perf-source", bits.join(" · "));
}

function chartSegments(values: (number | null)[]) {
  const segments: { index: number; value: number }[][] = [];
  let current: { index: number; value: number }[] = [];
  values.forEach((value, index) => {
    if (value === null || !Number.isFinite(value)) {
      if (current.length) segments.push(current);
      current = [];
      return;
    }
    current.push({ index, value: Math.max(0, Math.min(100, value)) });
  });
  if (current.length) segments.push(current);
  return segments;
}

function renderChart(snapshot: ResourceSnapshot) {
  const plot = $("perf-chart-plot");
  const empty = $("perf-chart-empty");
  const tip = $("perf-chart-tip");
  if (!plot) return;
  const history = snapshot.history;
  chartHistory = history;
  if (!history.length) {
    lastChartPointer = null;
    if (tip) tip.hidden = true;
    plot.replaceChildren();
    if (empty) {
      empty.hidden = false;
      empty.textContent = snapshot.supported
        ? "Waiting for the first one-second sample…"
        : snapshot.note ?? "Live samples are unavailable on this platform.";
    }
    return;
  }
  if (empty) empty.hidden = true;

  const W = 760;
  const H = 230;
  const left = 54;
  const right = 16;
  const top = 16;
  const bottom = 34;
  const plotWidth = W - left - right;
  const plotHeight = H - top - bottom;
  const denominator = Math.max(1, history.length - 1);
  const x = (index: number) => left + (index / denominator) * plotWidth;
  const y = (value: number) => top + (1 - Math.max(0, Math.min(100, value)) / 100) * plotHeight;
  const pathFor = (points: { index: number; value: number }[]) =>
    points
      .map((point, index) => `${index === 0 ? "M" : "L"}${x(point.index).toFixed(1)},${y(point.value).toFixed(1)}`)
      .join(" ");

  const parts: string[] = [`<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" aria-hidden="true">`];
  for (const value of [0, 25, 50, 75, 100]) {
    parts.push(
      `<line class="chart-grid-line" x1="${left}" x2="${W - right}" y1="${y(value)}" y2="${y(value)}" />`,
      `<text class="chart-axis-label" x="${left - 9}" y="${y(value) + 3}" text-anchor="end">${value}%</text>`,
    );
  }
  const labelIndexes = [...new Set([0, Math.round(denominator / 4), Math.round(denominator / 2), Math.round((denominator * 3) / 4), history.length - 1])];
  for (const index of labelIndexes) {
    const ago = history[index]?.ago_s ?? 0;
    parts.push(
      `<text class="chart-axis-label" x="${x(index)}" y="${H - 10}" text-anchor="${index === 0 ? "start" : index === history.length - 1 ? "end" : "middle"}">${ago === 0 ? "now" : `-${ago}s`}</text>`,
    );
  }

  const gpuSegments = chartSegments(historyValues("gpu_pct", history));
  for (const segment of gpuSegments) {
    if (segment.length < 2) continue;
    parts.push(
      `<path class="chart-area" d="${pathFor(segment)} L${x(segment[segment.length - 1].index).toFixed(1)},${y(0)} L${x(segment[0].index).toFixed(1)},${y(0)} Z" />`,
    );
  }
  parts.push(
    gpuSegments
      .map((segment) => `<path class="chart-line chart-gpu" d="${pathFor(segment)}" />`)
      .join(""),
    chartSegments(historyValues("vram_pct", history))
      .map((segment) => `<path class="chart-line chart-vram" d="${pathFor(segment)}" />`)
      .join(""),
    chartSegments(historyValues("cpu_pct", history))
      .map((segment) => `<path class="chart-line chart-cpu" d="${pathFor(segment)}" />`)
      .join(""),
    chartSegments(historyValues("ram_pct", history))
      .map((segment) => `<path class="chart-line chart-ram" d="${pathFor(segment)}" />`)
      .join(""),
  );

  for (const [key, css] of [
    ["gpu_pct", "chart-dot-gpu"],
    ["vram_pct", "chart-dot-vram"],
    ["cpu_pct", "chart-dot-cpu"],
    ["ram_pct", "chart-dot-ram"],
  ] as const) {
    const values = historyValues(key, history);
    const index = lastIndex(values);
    if (index >= 0 && values[index] !== null) {
      parts.push(
        `<circle class="chart-dot ${css}" cx="${x(index)}" cy="${y(values[index] as number)}" r="3.5" />`,
      );
    }
  }
  parts.push(
    `<line id="perf-chart-hover" class="chart-hover-line" x1="${left}" x2="${left}" y1="${top}" y2="${top + plotHeight}" hidden />`,
    `</svg>`,
  );
  plot.innerHTML = parts.join("");
  if (lastChartPointer) showChartTooltip(lastChartPointer);
}

function showChartTooltip(point: { clientX: number; clientY: number }) {
  const stage = $("perf-chart");
  const tip = $("perf-chart-tip");
  const hover = $("perf-chart-hover");
  if (!stage || !tip || !chartHistory.length) return;
  const rect = stage.getBoundingClientRect();
  const fraction = Math.max(0, Math.min(1, (point.clientX - rect.left) / Math.max(1, rect.width)));
  const index = Math.round(fraction * (chartHistory.length - 1));
  const sample = chartHistory[index];
  if (!sample) return;
  if (hover) {
    hover.hidden = false;
    const plotLeft = rect.width * (54 / 760);
    const plotRight = rect.width * ((760 - 16) / 760);
    const x = plotLeft + fraction * (plotRight - plotLeft);
    hover.setAttribute("x1", String((x / rect.width) * 760));
    hover.setAttribute("x2", String((x / rect.width) * 760));
  }
  const vram =
    sample.vram_pct !== null && latest?.vram?.used_mb !== null && latest?.vram?.used_mb !== undefined
      ? `${pctText(sample.vram_pct)} (${gbValue(latest.vram.used_mb)} GB)`
      : pctText(sample.vram_pct);
  tip.textContent = [
    sample.ago_s === 0 ? "now" : `${sample.ago_s}s ago`,
    `GPU  ${pctText(sample.gpu_pct)}`,
    `VRAM ${vram}`,
    `CPU  ${pctText(sample.cpu_pct)}`,
    `RAM  ${pctText(sample.ram_pct)}`,
  ].join("\n");
  tip.hidden = false;
  const tipWidth = 168;
  const left = Math.min(rect.width - tipWidth - 8, Math.max(8, point.clientX - rect.left + 12));
  const top = Math.min(rect.height - 120, Math.max(8, point.clientY - rect.top - 24));
  tip.style.left = `${left}px`;
  tip.style.top = `${top}px`;
}

function chartPointerMove(event: PointerEvent) {
  lastChartPointer = { clientX: event.clientX, clientY: event.clientY };
  showChartTooltip(lastChartPointer);
}

function chartPointerLeave() {
  lastChartPointer = null;
  const tip = $("perf-chart-tip");
  const hover = $("perf-chart-hover");
  if (tip) tip.hidden = true;
  if (hover) hover.hidden = true;
}

function applySnapshot(snapshot: ResourceSnapshot, force = false) {
  latest = snapshot;
  updateCurrentRunPeaks(snapshot);
  if (paused && !force) {
    updateLiveLabel();
    return;
  }
  updateLiveLabel();
  renderMetrics(snapshot);
  renderEngines(snapshot);
  renderHeat(snapshot);
  renderRuntime();
  renderHomePulse(snapshot);
  renderHealth(snapshot);
  renderSource(snapshot);
  if (page === "performance") renderChart(snapshot);
}

function updateCurrentRunPeaks(snapshot: ResourceSnapshot) {
  if (!currentRun) return;
  const max = (old: number | null, next: number | null) =>
    next === null ? old : old === null ? next : Math.max(old, next);
  currentRun.gpu = max(currentRun.gpu, snapshot.gpu?.load_pct ?? null);
  currentRun.vram_mb = max(currentRun.vram_mb, snapshot.vram?.process_mb ?? null);
  currentRun.ram_mb = max(currentRun.ram_mb, snapshot.ram?.process_mb ?? null);
  currentRun.cpu = max(currentRun.cpu, snapshot.cpu?.load_pct ?? null);
}

function finishRun() {
  if (!currentRun) return;
  const duration = Math.max(0, (Date.now() - currentRun.started) / 1000);
  runs.unshift({
    at: Date.now(),
    model: currentRun.model,
    duration_s: duration,
    gpu: currentRun.gpu,
    vram_mb: currentRun.vram_mb,
    ram_mb: currentRun.ram_mb,
    cpu: currentRun.cpu,
  });
  runs = runs.slice(0, MAX_RUNS);
  currentRun = null;
  saveRuns();
  renderRuns();
  renderRuntimeBars();
}

async function refresh() {
  try {
    const snapshot = await invoke<ResourceSnapshot>("resource_snapshot");
    applySnapshot(snapshot, true);
  } catch {
    text("perf-source", "Live counters are only available in the Sruthi desktop app.");
    text("pulse-foot", "Live counters need the desktop app");
  }
}

function setPaused(next: boolean) {
  paused = next;
  const button = $<HTMLButtonElement>("perf-pause");
  if (button) {
    button.textContent = next ? "Resume" : "Pause";
    button.setAttribute("aria-pressed", String(next));
  }
  updateLiveLabel();
  if (!next && latest) applySnapshot(latest, true);
}

function bindControls() {
  $("perf-pause")?.addEventListener("click", () => setPaused(!paused));
  $("perf-refresh")?.addEventListener("click", () => void refresh());
  $("perf-chart")?.addEventListener("pointermove", chartPointerMove);
  $("perf-chart")?.addEventListener("pointerleave", chartPointerLeave);

  $("perf-thermal-toggle")?.addEventListener("change", async (event) => {
    const input = event.currentTarget as HTMLInputElement;
    try {
      await invoke("set_thermal", { enabled: input.checked });
    } catch {
      input.checked = !input.checked;
      text("perf-heat-message", "Could not save the heat guard setting.");
    }
  });

  document.querySelectorAll<HTMLElement>("[data-open-performance]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      document.getElementById("nav-performance")?.click();
    });
  });

  $("settings-search")?.addEventListener("input", (event) => {
    settingsQuery = (event.currentTarget as HTMLInputElement).value.trim().toLowerCase();
    applySettingsFilter();
  });

  document.querySelectorAll<HTMLButtonElement>("#settings-filters [data-filter]").forEach((button) => {
    button.addEventListener("click", () => {
      settingsFilter = button.dataset.filter ?? "all";
      document.querySelectorAll<HTMLButtonElement>("#settings-filters [data-filter]").forEach((other) => {
        other.setAttribute("aria-pressed", String(other === button));
      });
      applySettingsFilter();
    });
  });
}

function applySettingsFilter() {
  const cards = [...document.querySelectorAll<HTMLElement>("#page-settings .page-grid > *")];
  if (!cards.length) return;
  let visible = 0;
  for (const card of cards) {
    const categories = (card.dataset.category ?? "").split(/\s+/);
    const categoryMatch = settingsFilter === "all" || categories.includes(settingsFilter);
    const haystack = `${card.textContent ?? ""} ${card.dataset.settings ?? ""}`.toLowerCase();
    const textMatch = !settingsQuery || haystack.includes(settingsQuery);
    const show = categoryMatch && textMatch;
    card.hidden = !show;
    if (show) visible += 1;
  }
  const empty = $("settings-empty");
  if (empty) {
    empty.hidden = visible > 0;
    if (!visible) {
      empty.textContent = settingsQuery
        ? `No settings match “${settingsQuery}”. Try “microphone”, “heat”, or “privacy”.`
        : "No settings in this section yet.";
    }
  }
  text("settings-count", visible === cards.length ? `${cards.length} settings` : `${visible} of ${cards.length} settings`);
}

function listenForDictation() {
  listen<DictationEvent>("dictation", ({ payload }) => {
    if (payload.phase === "recording") {
      currentRun = {
        started: Date.now(),
        model: status.activeModel,
        gpu: null,
        vram_mb: null,
        ram_mb: null,
        cpu: null,
      };
      return;
    }
    if (payload.phase === "done") {
      finishRun();
      return;
    }
    if (payload.phase === "error") {
      currentRun = null;
    }
  }).catch(() => {
    // The live counters still update through the command fallback.
  });
}

export function initResources() {
  const visiblePage = [...document.querySelectorAll<HTMLElement>(".page")].find((section) => !section.hidden);
  page = visiblePage?.id.replace(/^page-/, "") ?? "home";
  runs = loadRuns();
  bindControls();
  applySettingsFilter();
  renderRuns();
  renderRuntime();
  updateLiveLabel();
  listenForDictation();
  void listen<ResourceSnapshot>("resource-changed", ({ payload }) => applySnapshot(payload)).catch(() => {
    polling = window.setInterval(() => void refresh(), 2500);
  });
  void refresh();

  return {
    setStatus(next: ResourceStatus) {
      status = next;
      renderRuntime();
    },
    setPage(next: string) {
      page = next;
      if (next === "performance" && latest && !paused) renderChart(latest);
    },
    stop() {
      if (polling !== null) window.clearInterval(polling);
    },
  };
}
