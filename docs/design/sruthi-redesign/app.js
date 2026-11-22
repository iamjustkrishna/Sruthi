/* Sruthi interface preview — sample data + interaction.
   Real values will come from Windows counters and Sruthi's own perf/thermal
   modules; this artifact deliberately labels its numbers as a snapshot. */

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

const SCREENS = ["home", "dictionary", "snippets", "models", "performance", "surfaces", "settings"];
const state = { screen: "performance", theme: "light", paused: false };

/* ---------- sample resource window: 60 one-second samples ---------- */
const N = 60;
const noise = (i) => [0, 1, -1, 2, -2, 1, 0][i % 7] * 0.55;
const gpu = Array.from({ length: N }, (_, i) => {
  if (i < 26) return Math.round(17 + 3 * Math.sin(i / 2.7) + noise(i));
  if (i < 30) return [30, 43, 54, 61][i - 26] + Math.round(noise(i));
  if (i <= 52) {
    const bump = 7 * Math.exp(-Math.pow((i - 38) / 7, 2));
    return Math.round(64 + 4 * Math.sin((i - 32) / 2) + bump + noise(i));
  }
  return [62, 52, 44, 38, 34, 32, 33, 34][i - 52];
}).map((v) => (v > 60 ? Math.round(60 + (v - 60) * 0.73) : v));
const vram = Array.from({ length: N }, (_, i) => {
  if (i < 26) return 36 + Math.round(1.4 * Math.sin(i / 3));
  if (i < 32) return Math.round(37 + (i - 26) * 2.05);
  if (i <= 52) return 49 + Math.round(1.1 * Math.sin(i / 2));
  return 40 + Math.round(0.9 * Math.sin(i / 2));
});
const cpu = Array.from({ length: N }, (_, i) => {
  if (i < 26) return Math.round(11 + 2.2 * Math.sin(i / 2.4) + noise(i));
  if (i < 31) return Math.round(11 + (i - 26) * 10.2);
  if (i <= 52) return Math.round(58 + 4 * Math.sin((i - 31) / 2.1) + noise(i));
  return Math.round(13 + 1.6 * Math.sin(i / 2.8));
});
const ram = Array.from({ length: N }, (_, i) => 29.2 + 0.4 * Math.sin(i / 5) + i * 0.003);
const timeLabels = ["−60s", "−45s", "−30s", "−15s", "now"];
const peakIndex = gpu.indexOf(Math.max(...gpu));

/* ---------- tiny generated histories for stats cards ---------- */
const histories = {
  gpu, cpu, vram, ram,
  dictations: [6, 9, 8, 13, 11, 17, 15, 21, 18, 24, 20, 27, 31, 28, 34, 31, 39, 37, 42],
  words: [120, 210, 190, 320, 280, 410, 390, 520, 480, 610, 570, 690, 640, 760, 720, 840, 790, 910, 880],
  uses: [2, 3, 3, 5, 4, 6, 5, 8, 7, 9, 8, 11, 10, 12, 11, 14, 12, 15, 14],
  matches: [8, 10, 9, 14, 12, 16, 15, 19, 17, 21, 18, 24, 22, 26, 23, 28, 25, 30, 27],
  saved: [4, 6, 5, 8, 7, 10, 9, 12, 11, 14, 12, 16, 15, 18, 16, 20, 18, 22, 21],
};

function sparkline(el, values, color = "var(--sound)", fill = true) {
  if (!el) return;
  const w = 120, h = 30, pad = 2;
  const min = Math.min(...values), max = Math.max(...values);
  const span = max - min || 1;
  const pts = values.map((v, i) => [
    pad + (i / (values.length - 1)) * (w - pad * 2),
    h - pad - ((v - min) / span) * (h - pad * 2),
  ]);
  const d = pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`).join(" ");
  const area = `${d} L${pts[pts.length - 1][0].toFixed(1)} ${h - pad} L${pts[0][0].toFixed(1)} ${h - pad} Z`;
  const id = "sg" + Math.random().toString(36).slice(2, 8);
  el.innerHTML = `<svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" aria-hidden="true">
    <defs><linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="${color}" stop-opacity="${fill ? ".22" : "0"}"/>
      <stop offset="1" stop-color="${color}" stop-opacity="0"/>
    </linearGradient></defs>
    ${fill ? `<path d="${area}" fill="url(#${id})"/>` : ""}
    <path d="${d}" fill="none" stroke="${color}" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>
    <circle cx="${pts[pts.length - 1][0].toFixed(1)}" cy="${pts[pts.length - 1][1].toFixed(1)}" r="1.7" fill="${color}"/>
  </svg>`;
}

function renderSparks() {
  $$("[data-spark]").forEach((el) => {
    const key = el.dataset.spark;
    const values = histories[key] || histories.gpu;
    const colors = {
      gpu: "var(--sound)", cpu: "var(--muted)", vram: "var(--teal)", ram: "var(--amber)",
      dictations: "var(--sound)", words: "var(--sound)", uses: "var(--teal)", matches: "var(--violet)", saved: "var(--ok)",
    };
    sparkline(el, values, colors[key] || "var(--sound)", el.dataset.fill !== "0");
  });
}

/* ---------- navigation, theme, demo feedback ---------- */
function showScreen(name, updateHash = true) {
  if (!SCREENS.includes(name)) name = "performance";
  state.screen = name;
  document.body.dataset.currentScreen = name;
  $$(".screen").forEach((s) => { s.hidden = s.id !== `screen-${name}`; });
  $$("[data-screen]").forEach((b) => {
    if (b.classList.contains("nav-item")) b.setAttribute("aria-current", b.dataset.screen === name ? "page" : "false");
  });
  const main = $("#main");
  if (main) main.scrollTop = 0;
  if (updateHash) {
    try { history.replaceState(null, "", `#${name}`); } catch (_) { /* file:// is allowed to refuse */ }
  }
  const nav = $(`.nav-item[data-screen="${name}"]`);
  document.title = `Sruthi — ${(nav?.querySelector(".nav-label")?.textContent || (name[0].toUpperCase() + name.slice(1)))}`;
  requestAnimationFrame(() => {
    if (name === "performance") renderGpuChart();
    if (name === "models") renderBenchmark();
  });
}

function initRoute() {
  const params = new URLSearchParams(location.search);
  if (params.get("clean") === "1") document.body.classList.add("clean");
  if (params.get("full") === "1") document.body.classList.add("full");
  const theme = params.get("theme") || localStorage.getItem("sruthi-preview-theme") || "light";
  setTheme(theme === "dark" ? "dark" : "light");
  const fromQuery = params.get("screen");
  const fromHash = location.hash.replace("#", "");
  showScreen(SCREENS.includes(fromQuery) ? fromQuery : SCREENS.includes(fromHash) ? fromHash : "performance", false);
}

function setTheme(theme) {
  state.theme = theme;
  document.documentElement.dataset.theme = theme;
  try { localStorage.setItem("sruthi-preview-theme", theme); } catch (_) { /* preview still works */ }
  const btn = $("#theme-toggle");
  if (btn) {
    btn.setAttribute("aria-label", theme === "dark" ? "Switch to light theme" : "Switch to dark theme");
    btn.innerHTML = theme === "dark"
      ? '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>'
      : '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M20 14.5A8.5 8.5 0 0 1 9.5 4 8.5 8.5 0 1 0 20 14.5z"/></svg>';
  }
  renderSparks();
}

function toast(message) {
  const el = $("#toast");
  if (!el) return;
  el.textContent = message;
  el.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { el.hidden = true; }, 2600);
}

function initControls() {
  document.addEventListener("click", (event) => {
    const screenBtn = event.target.closest("[data-screen]");
    if (screenBtn) {
      event.preventDefault();
      showScreen(screenBtn.dataset.screen);
      return;
    }
    const demo = event.target.closest("[data-toast]");
    if (demo && !event.target.closest("input, textarea, select")) {
      event.preventDefault();
      toast(demo.dataset.toast);
      return;
    }
    const themeBtn = event.target.closest("#theme-toggle");
    if (themeBtn) {
      setTheme(state.theme === "dark" ? "light" : "dark");
      return;
    }
    const pause = event.target.closest("#perf-pause");
    if (pause) {
      state.paused = !state.paused;
      pause.innerHTML = state.paused
        ? '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5l11 7-11 7z"/></svg> Resume'
        : '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/></svg> Pause';
      toast(state.paused ? "Snapshot paused — values stay where they are." : "Snapshot resumed. Live sampling arrives with the production wiring.");
      return;
    }
    const seg = event.target.closest(".segmented button");
    if (seg) {
      $$("button", seg.parentElement).forEach((b) => b.setAttribute("aria-pressed", b === seg ? "true" : "false"));
      return;
    }
    const choice = event.target.closest("[data-choice-group] button");
    if (choice) {
      const group = choice.closest("[data-choice-group]");
      $$("button", group).forEach((b) => b.classList.toggle("active", b === choice));
      return;
    }
    const snippet = event.target.closest(".snippet-item");
    if (snippet) {
      $$(".snippet-item").forEach((b) => b.classList.toggle("active", b === snippet));
      const trigger = $("#snippet-trigger");
      const text = $("#snippet-text");
      const preview = $("#snippet-preview-text");
      if (trigger) trigger.value = snippet.dataset.trigger || trigger.value;
      if (text) text.value = snippet.dataset.text || text.value;
      if (preview) preview.textContent = snippet.dataset.text || preview.textContent;
      return;
    }
    const settingsChip = event.target.closest(".settings-chips button");
    if (settingsChip) filterSettings(settingsChip.dataset.filter || "all");
  });

  const search = $("#settings-search");
  if (search) search.addEventListener("input", () => {
    const active = $(".settings-chips button[aria-pressed='true']");
    filterSettings(active?.dataset.filter || "all", search.value);
  });

  addEventListener("hashchange", () => showScreen(location.hash.replace("#", ""), false));
}

function filterSettings(category, text = $("#settings-search")?.value || "") {
  $$(".settings-chips button").forEach((b) => b.setAttribute("aria-pressed", b.dataset.filter === category ? "true" : "false"));
  const query = text.trim().toLowerCase();
  let shown = 0;
  $$("#screen-settings .setting-card").forEach((card) => {
    const cat = card.dataset.category || "";
    const matchesCat = category === "all" || cat === category || (category === "dictation" && cat === "words");
    const matchesText = !query || card.textContent.toLowerCase().includes(query);
    const show = matchesCat && matchesText;
    card.hidden = !show;
    if (show) shown++;
  });
  const empty = $("#settings-empty");
  if (empty) empty.hidden = shown > 0;
  const count = $("#settings-count");
  if (count) count.textContent = shown;
}

/* ---------- GPU chart ---------- */
function pathD(points) {
  if (!points.length) return "";
  let d = `M ${points[0][0].toFixed(1)} ${points[0][1].toFixed(1)}`;
  for (let i = 1; i < points.length; i++) {
    const [x0, y0] = points[i - 1], [x1, y1] = points[i];
    const cx = (x0 + x1) / 2;
    d += ` C ${cx.toFixed(1)} ${y0.toFixed(1)}, ${cx.toFixed(1)} ${y1.toFixed(1)}, ${x1.toFixed(1)} ${y1.toFixed(1)}`;
  }
  return d;
}

function renderGpuChart() {
  const host = $("#gpu-chart");
  if (!host || host.clientWidth < 80) return;
  const W = Math.max(540, host.clientWidth || 900);
  const H = Math.max(210, host.clientHeight || 258);
  const pad = { l: 42, r: 16, t: 22, b: 25 };
  const plotW = W - pad.l - pad.r;
  const plotH = H - pad.t - pad.b;
  const x = (i) => pad.l + (i / (N - 1)) * plotW;
  const y = (v) => pad.t + (1 - v / 100) * plotH;
  const pts = (arr) => arr.map((v, i) => [x(i), y(v)]);
  const gpuPts = pts(gpu), vramPts = pts(vram), cpuPts = pts(cpu);
  const gpuLine = pathD(gpuPts), vramLine = pathD(vramPts), cpuLine = pathD(cpuPts);
  const area = `${gpuLine} L${x(N - 1).toFixed(1)} ${y(0).toFixed(1)} L${x(0).toFixed(1)} ${y(0).toFixed(1)} Z`;
  const eventStart = 26, eventEnd = 52;
  const peak = gpuPts[peakIndex];
  const peakLabelX = peak[0] > W - 130 ? peak[0] - 12 : peak[0] + 10;
  const peakAnchor = peak[0] > W - 130 ? "end" : "start";

  const grid = [0, 25, 50, 75, 100].map((v) => `
    <line x1="${pad.l}" x2="${W - pad.r}" y1="${y(v)}" y2="${y(v)}" stroke="var(--chart-grid)" stroke-width="1"/>
    <text x="${pad.l - 8}" y="${y(v) + 3}" fill="var(--chart-text)" font-size="8.5" text-anchor="end">${v}%</text>`).join("");
  const xTicks = [0, 15, 30, 45, 59].map((i, n) => `
    <text x="${x(i)}" y="${H - 7}" fill="var(--chart-text)" font-size="8.5" text-anchor="${n === 0 ? "start" : n === 4 ? "end" : "middle"}">${timeLabels[n]}</text>`).join("");

  host.innerHTML = `
    <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="Sample 60-second GPU activity chart. GPU peaks at ${Math.max(...gpu)} percent during a dictation, VRAM reaches ${Math.max(...vram)} percent, CPU reaches ${Math.max(...cpu)} percent.">
      <defs>
        <linearGradient id="gpuFill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="var(--chart-gpu)" stop-opacity=".33"/>
          <stop offset="1" stop-color="var(--chart-gpu)" stop-opacity="0"/>
        </linearGradient>
        <filter id="gpuGlow" x="-20%" y="-20%" width="140%" height="140%">
          <feGaussianBlur stdDeviation="4" result="blur"/><feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge>
        </filter>
      </defs>
      ${grid}
      <rect x="${x(eventStart)}" y="${pad.t}" width="${x(eventEnd) - x(eventStart)}" height="${plotH}" fill="rgba(138,168,255,.065)" stroke="rgba(138,168,255,.16)" stroke-dasharray="3 4"/>
      <text x="${x(eventStart) + 6}" y="${pad.t + 13}" fill="#b9c8ff" font-size="8.5" font-weight="600">Dictation · 18.4 s</text>
      <line x1="${peak[0]}" x2="${peak[0]}" y1="${pad.t}" y2="${y(0)}" stroke="rgba(255,255,255,.14)" stroke-dasharray="2 4"/>
      <circle cx="${peak[0]}" cy="${peak[1]}" r="4.2" fill="var(--chart-gpu)" stroke="#fff" stroke-width="1.4"/>
      <text x="${peakLabelX}" y="${peak[1] - 10}" fill="#dbe2ff" font-size="8.5" font-weight="650" text-anchor="${peakAnchor}">Peak ${gpu[peakIndex]}%</text>
      <path d="${area}" fill="url(#gpuFill)"/>
      <path d="${cpuLine}" fill="none" stroke="var(--chart-cpu)" stroke-width="1.15" stroke-linecap="round" opacity=".72"/>
      <path d="${vramLine}" fill="none" stroke="var(--chart-vram)" stroke-width="1.5" stroke-linecap="round" opacity=".9"/>
      <path d="${gpuLine}" fill="none" stroke="var(--chart-gpu)" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" filter="url(#gpuGlow)"/>
      <circle cx="${gpuPts[N - 1][0]}" cy="${gpuPts[N - 1][1]}" r="8" fill="none" stroke="var(--chart-gpu)" stroke-width="1" opacity=".35" class="chart-live-halo"/>
      <circle cx="${gpuPts[N - 1][0]}" cy="${gpuPts[N - 1][1]}" r="3" fill="var(--chart-gpu)" stroke="#101114" stroke-width="1.5"/>
      ${xTicks}
      <g class="hover-layer" visibility="hidden">
        <line y1="${pad.t}" y2="${y(0)}" stroke="rgba(255,255,255,.25)" stroke-dasharray="3 3"/>
        <circle r="3" fill="var(--chart-gpu)" stroke="#fff" stroke-width="1.2"/>
        <circle r="3" fill="var(--chart-vram)" stroke="#101114" stroke-width="1.2"/>
      </g>
    </svg>
    <div class="chart-tip" id="chart-tip" hidden></div>`;

  const svg = host.querySelector("svg");
  const layer = host.querySelector(".hover-layer");
  const tip = host.querySelector("#chart-tip");
  const hoverLine = layer.querySelector("line");
  const [gpuDot, vramDot] = layer.querySelectorAll("circle");
  svg.addEventListener("pointermove", (event) => {
    const rect = svg.getBoundingClientRect();
    const px = (event.clientX - rect.left) * (W / rect.width);
    const i = Math.max(0, Math.min(N - 1, Math.round(((px - pad.l) / plotW) * (N - 1))));
    const gx = x(i);
    layer.setAttribute("visibility", "visible");
    hoverLine.setAttribute("x1", gx); hoverLine.setAttribute("x2", gx);
    gpuDot.setAttribute("cx", gx); gpuDot.setAttribute("cy", y(gpu[i]));
    vramDot.setAttribute("cx", gx); vramDot.setAttribute("cy", y(vram[i]));
    const label = i === N - 1 ? "now" : `−${N - 1 - i}s`;
    tip.innerHTML = `<b>${label}</b><div class="tip-row"><span>GPU busy</span><b>${gpu[i]}%</b></div><div class="tip-row"><span>VRAM</span><b>${(vram[i] * 0.08).toFixed(1)} GB</b></div><div class="tip-row"><span>CPU</span><b>${cpu[i]}%</b></div>`;
    tip.hidden = false;
    const left = Math.min(rect.width - 146, Math.max(8, (gx / W) * rect.width + 12));
    tip.style.left = `${left}px`;
    tip.style.top = "14px";
  });
  svg.addEventListener("pointerleave", () => { layer.setAttribute("visibility", "hidden"); tip.hidden = true; });
}

/* ---------- benchmark scatter ---------- */
function renderBenchmark() {
  const host = $("#bench-chart");
  if (!host || host.clientWidth < 80) return;
  const W = Math.max(640, host.clientWidth || 920);
  const H = Math.max(220, host.clientHeight || 258);
  const pad = { l: 45, r: 28, t: 18, b: 38 };
  const plotW = W - pad.l - pad.r, plotH = H - pad.t - pad.b;
  const x = (v) => pad.l + ((v - 0.5) / 4.5) * plotW;
  const y = (v) => pad.t + ((100 - v) / 9) * plotH;
  const points = [
    { name: "Parakeet TDT", speed: 3.4, acc: 98.2, color: "var(--sound)", r: 6 },
    { name: "Whisper Large Turbo", speed: 2.1, acc: 97.8, color: "var(--violet)", r: 5 },
    { name: "IndicConformer", speed: 1.8, acc: 96.4, color: "var(--teal)", r: 5 },
    { name: "Distil-Whisper", speed: 4.1, acc: 95.1, color: "var(--muted)", r: 5 },
  ];
  const xTicks = [1, 2, 3, 4, 5].map((v) => `<line x1="${x(v)}" x2="${x(v)}" y1="${pad.t}" y2="${H - pad.b}" stroke="var(--border)" stroke-width="1" opacity=".7"/><text x="${x(v)}" y="${H - 18}" fill="var(--muted)" font-size="8.5" text-anchor="middle">${v}×</text>`).join("");
  const yTicks = [91, 93, 95, 97, 99].map((v) => `<line x1="${pad.l}" x2="${W - pad.r}" y1="${y(v)}" y2="${y(v)}" stroke="var(--border)" stroke-width="1" opacity=".7"/><text x="${pad.l - 8}" y="${y(v) + 3}" fill="var(--muted)" font-size="8.5" text-anchor="end">${v}%</text>`).join("");
  const dots = points.map((p) => `
    <circle cx="${x(p.speed)}" cy="${y(p.acc)}" r="${p.r + 5}" fill="${p.color}" opacity=".12"/>
    <circle cx="${x(p.speed)}" cy="${y(p.acc)}" r="${p.r}" fill="${p.color}" stroke="var(--card)" stroke-width="2"/>
    <text x="${x(p.speed) + 9}" y="${y(p.acc) - 7}" fill="var(--text)" font-size="8.5" font-weight="600">${p.name}</text>
    <text x="${x(p.speed) + 9}" y="${y(p.acc) + 4}" fill="var(--muted)" font-size="8">${p.acc}% · ${p.speed}×</text>`).join("");
  host.innerHTML = `<svg viewBox="0 0 ${W} ${H}" role="img" aria-label="Sample benchmark scatter plot. Parakeet TDT leads at 98.2 percent word accuracy and 3.4 times realtime.">
    <rect x="${pad.l}" y="${pad.t}" width="${plotW}" height="${plotH}" rx="8" fill="transparent"/>
    ${xTicks}${yTicks}${dots}
    <text x="${pad.l + plotW / 2}" y="${H - 4}" fill="var(--muted)" font-size="8.5" text-anchor="middle">Faster →</text>
    <text x="11" y="${pad.t + plotH / 2}" fill="var(--muted)" font-size="8.5" text-anchor="middle" transform="rotate(-90 11 ${pad.t + plotH / 2})">Word accuracy →</text>
  </svg>`;
}

/* ---------- boot ---------- */
function boot() {
  initRoute();
  initControls();
  renderSparks();
  requestAnimationFrame(() => {
    renderGpuChart();
    renderBenchmark();
    filterSettings("all");
  });
  let resizeTimer;
  addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => { renderGpuChart(); renderBenchmark(); }, 80);
  });
}

document.addEventListener("DOMContentLoaded", boot);
