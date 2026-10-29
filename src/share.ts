// Share cards: your totals drawn as a picture, entirely on this PC.
// Each card is painted onto a canvas at the size it is posted at (the look is
// the "Shareable cards" section of the design canvas), then saved or copied.
// Nothing here talks to the network, and no dictated text is ever drawn:
// only counts, speeds and, if allowed, app names.

import sruthiLogo from "./assets/sruthi-logo.png";

export type CardKind = "time" | "week" | "month" | "alltime" | "speed" | "milestone";

export interface ShareData {
  words: number;
  savedMin: number;
  wpm: number;
  streak: number;
  dictations: number;
  week: { date: string; words: number }[];
  /** Every day with dictations, oldest first, for the month and all-time cards. */
  days: { date: string; words: number }[];
  /** The first day anything was dictated ("2026-10-03"). */
  firstDay: string | null;
  longestStreak: number;
  /** Apps with a readable name, most words first. */
  apps: { name: string; words: number }[];
}

/** What the person lets onto the card. */
export interface ShareOptions {
  streak: boolean;
  speed: boolean;
  apps: boolean;
}

/** The same typing speed the rest of the app compares against. */
export const TYPING_WPM = 40;
export const MILESTONES = [500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000];

export const CARD_NAMES: Record<CardKind, string> = {
  time: "Time saved",
  week: "My week",
  month: "My month",
  alltime: "All time",
  speed: "Speed",
  milestone: "Milestone",
};

const FONT = '"Segoe UI Variable Display", "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif';

type Ctx = CanvasRenderingContext2D;

/** Which of the "show on the card" switches change anything on each card, so
 *  the sheet only offers the ones that do. */
export const OPTIONS_FOR: Record<CardKind, ShareOptions> = {
  time: { streak: true, speed: true, apps: false },
  week: { streak: true, speed: true, apps: true },
  month: { streak: true, speed: true, apps: true },
  alltime: { streak: true, speed: true, apps: false },
  speed: { streak: false, speed: false, apps: false },
  milestone: { streak: true, speed: true, apps: false },
};

/** The biggest round number of words you have passed, if any. */
export function milestoneReached(words: number): number | null {
  return [...MILESTONES].reverse().find((m) => m <= words) ?? null;
}

export function cardSize(kind: CardKind): { w: number; h: number } {
  switch (kind) {
    case "time":
      return { w: 1080, h: 1080 };
    case "speed":
      return { w: 1200, h: 630 };
    default:
      return { w: 1080, h: 1350 };
  }
}

/** Why a card can't be made yet, or null when it can. */
export function unavailable(kind: CardKind, d: ShareData): string | null {
  switch (kind) {
    case "time":
      return d.words > 0 ? null : "Dictate something first";
    case "week":
      return d.week.some((x) => x.words > 0) ? null : "No dictations this week yet";
    case "month":
      return monthInfo(d).words > 0 ? null : "No dictations this month yet";
    case "alltime":
      return d.days.length >= 2 ? null : "Needs at least two days of dictating";
    case "speed":
      return d.wpm > 0 ? null : "Needs a few longer dictations to measure your speed";
    case "milestone":
      return milestoneReached(d.words) ? null : `Unlocks at ${MILESTONES[0].toLocaleString()} words`;
  }
}

function typed(min: number): { v: number; u: string }[] {
  if (min >= 60) {
    const h = Math.floor(min / 60);
    const m = Math.round(min - h * 60);
    return m > 0 && m < 60 ? [{ v: h, u: "h" }, { v: m, u: "m" }] : [{ v: h + (m === 60 ? 1 : 0), u: "h" }];
  }
  return min >= 1 ? [{ v: Math.round(min), u: "m" }] : [{ v: Math.max(1, Math.round(min * 60)), u: "s" }];
}

function typedText(min: number) {
  return typed(min)
    .map((p) => `${p.v}${p.u}`)
    .join(" ");
}

function times(wpm: number) {
  return (wpm / TYPING_WPM).toFixed(1).replace(/\.0$/, "");
}

/** "Oct 3" from "2026-10-03". */
function shortDate(date: string) {
  return new Date(`${date}T12:00:00`).toLocaleDateString([], { month: "short", day: "numeric" });
}

/** This calendar month's share of `days`. */
function monthInfo(d: ShareData) {
  const now = new Date();
  const prefix = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;
  const mine = d.days.filter((x) => x.date.startsWith(prefix));
  return {
    now,
    prefix,
    mine,
    words: mine.reduce((n, x) => n + x.words, 0),
    name: now.toLocaleDateString([], { month: "long" }),
    label: now.toLocaleDateString([], { month: "long", year: "numeric" }),
    length: new Date(now.getFullYear(), now.getMonth() + 1, 0).getDate(),
    today: now.getDate(),
  };
}

function weekWords(d: ShareData) {
  return d.week.reduce((n, x) => n + x.words, 0);
}

/** A caption to post with the picture, in the first person. */
export function caption(kind: CardKind, d: ShareData, o: ShareOptions): string {
  switch (kind) {
    case "time": {
      const tail = o.speed && d.wpm > 0 ? ` ${Math.round(d.wpm)} words a minute, all of it offline.` : " All of it offline.";
      return `Skipped ${typedText(d.savedMin)} of typing by talking to my PC.${tail}`;
    }
    case "week": {
      const streak = o.streak && d.streak > 1 ? ` ${d.streak}-day streak.` : "";
      return `${weekWords(d).toLocaleString()} words this week, spoken instead of typed.${streak} Offline, on my PC.`;
    }
    case "month": {
      const m = monthInfo(d);
      const streak = o.streak && d.streak > 1 ? ` ${d.streak}-day streak.` : "";
      return `${m.words.toLocaleString()} words in ${m.name}, spoken instead of typed.${streak} Offline, on my PC.`;
    }
    case "alltime": {
      const since = d.firstDay ? ` since ${shortDate(d.firstDay)}` : "";
      return `${d.words.toLocaleString()} words${since}, none of them typed. Offline, on my PC.`;
    }
    case "speed":
      return `I dictate at ${Math.round(d.wpm)} words a minute, ${times(d.wpm)}× my typing speed. Offline, on my PC.`;
    case "milestone": {
      const m = milestoneReached(d.words) ?? d.words;
      return `${m.toLocaleString()} words dictated, none of them typed. Offline, on my PC.`;
    }
  }
}

// ---------------------------------------------------------------------------
// Drawing helpers

let logoImage: Promise<HTMLImageElement> | null = null;
function loadLogo() {
  logoImage ??= new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = sruthiLogo;
  });
  return logoImage;
}

function newCanvas(w: number, h: number) {
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d")!;
  return { canvas, ctx };
}

interface TextOpts {
  size: number;
  weight?: number;
  fill: string | CanvasGradient;
  align?: CanvasTextAlign;
  /** Letter spacing in px (negative to tighten big numbers). */
  spacing?: number;
  alpha?: number;
}

function text(ctx: Ctx, s: string, x: number, y: number, o: TextOpts) {
  ctx.save();
  ctx.font = `${o.weight ?? 600} ${o.size}px ${FONT}`;
  ctx.letterSpacing = `${o.spacing ?? 0}px`;
  ctx.fillStyle = o.fill;
  ctx.textAlign = o.align ?? "left";
  ctx.textBaseline = "alphabetic";
  ctx.globalAlpha = o.alpha ?? 1;
  ctx.fillText(s, x, y);
  const width = ctx.measureText(s).width;
  ctx.restore();
  return width;
}

function measure(ctx: Ctx, s: string, size: number, weight: number, spacing = 0) {
  ctx.save();
  ctx.font = `${weight} ${size}px ${FONT}`;
  ctx.letterSpacing = `${spacing}px`;
  const width = ctx.measureText(s).width;
  ctx.restore();
  return width;
}

/** The largest size, up to `size`, at which `s` fits in `max` px. */
function fit(ctx: Ctx, s: string, size: number, max: number, weight = 600, spacingEm = 0) {
  const w = measure(ctx, s, size, weight, size * spacingEm);
  return w > max ? Math.floor((size * max) / w) : size;
}

function vertical(ctx: Ctx, top: number, bottom: number, stops: [number, string][]) {
  const g = ctx.createLinearGradient(0, top, 0, bottom);
  for (const [at, color] of stops) g.addColorStop(at, color);
  return g;
}

function blob(ctx: Ctx, cx: number, cy: number, rx: number, ry: number, color: string, alpha: number, blur: number) {
  ctx.save();
  ctx.filter = `blur(${blur}px)`;
  ctx.globalAlpha = alpha;
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}

/** A fine dot grid that fades along one axis. */
function dots(ctx: Ctx, w: number, h: number, step: number, alpha: number, fade: CanvasGradient) {
  const { canvas, ctx: g } = newCanvas(w, h);
  g.fillStyle = "#ffffff";
  for (let y = step / 2; y < h; y += step) {
    for (let x = step / 2; x < w; x += step) {
      g.beginPath();
      g.arc(x, y, 1.3, 0, Math.PI * 2);
      g.fill();
    }
  }
  g.globalCompositeOperation = "destination-in";
  g.fillStyle = fade;
  g.fillRect(0, 0, w, h);
  ctx.save();
  ctx.globalAlpha = alpha;
  ctx.drawImage(canvas, 0, 0);
  ctx.restore();
}

let grainTile: HTMLCanvasElement | null = null;
/** Film grain, so the gradients don't look like flat CSS. */
function grain(ctx: Ctx, w: number, h: number, alpha: number, mode: GlobalCompositeOperation) {
  if (!grainTile) {
    const { canvas, ctx: g } = newCanvas(256, 256);
    const img = g.createImageData(256, 256);
    let seed = 7;
    for (let i = 0; i < img.data.length; i += 4) {
      seed = (seed * 1664525 + 1013904223) >>> 0;
      const v = seed >>> 24;
      img.data[i] = img.data[i + 1] = img.data[i + 2] = v;
      img.data[i + 3] = 255;
    }
    g.putImageData(img, 0, 0);
    grainTile = canvas;
  }
  ctx.save();
  ctx.globalCompositeOperation = mode;
  ctx.globalAlpha = alpha;
  ctx.fillStyle = ctx.createPattern(grainTile, "repeat")!;
  ctx.fillRect(0, 0, w, h);
  ctx.restore();
}

/** A hairline round the edge with a brighter top, like glass. */
function edge(ctx: Ctx, w: number, h: number, a = 0.12) {
  ctx.save();
  ctx.strokeStyle = `rgba(255,255,255,${a})`;
  ctx.lineWidth = 2;
  ctx.strokeRect(1, 1, w - 2, h - 2);
  const g = ctx.createLinearGradient(0, 0, 0, 80);
  g.addColorStop(0, `rgba(255,255,255,${a * 1.8})`);
  g.addColorStop(1, "rgba(255,255,255,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, 3);
  ctx.restore();
}

function lock(ctx: Ctx, x: number, y: number, size: number, color: string) {
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = size / 11;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  const u = size / 24;
  ctx.strokeRect(x + 5 * u, y + 11 * u, 14 * u, 9 * u);
  ctx.beginPath();
  ctx.moveTo(x + 8 * u, y + 11 * u);
  ctx.lineTo(x + 8 * u, y + 8 * u);
  ctx.arc(x + 12 * u, y + 8 * u, 4 * u, Math.PI, 0);
  ctx.lineTo(x + 16 * u, y + 11 * u);
  ctx.stroke();
  ctx.restore();
}

function footer(ctx: Ctx, w: number, y: number, pad: number, color: string, size: number, right: string) {
  lock(ctx, pad, y - size + 2, size, color);
  text(ctx, "Dictated offline on my PC", pad + size + 10, y, { size, weight: 400, fill: color });
  text(ctx, right, w - pad, y, { size, weight: 400, fill: color, align: "right" });
}

async function brand(ctx: Ctx, x: number, y: number, size: number, color: string) {
  const logo = await loadLogo();
  ctx.drawImage(logo, x, y, size, size);
  text(ctx, "Sruthi", x + size + size * 0.33, y + size * 0.7, { size: size * 0.58, fill: color, spacing: -0.3 });
}

function pill(ctx: Ctx, label: string, right: number, midY: number, o: { size: number; fill: string; stroke: string; color: string; dot?: string; caps?: boolean }) {
  const padX = o.size * 0.9;
  const dotW = o.dot ? o.size * 1.1 : 0;
  const spacing = o.caps ? o.size * 0.14 : 0;
  const tw = measure(ctx, label, o.size, 600, spacing);
  const w = padX * 2 + dotW + tw;
  const h = o.size * 2.1;
  const x = right - w;
  const y = midY - h / 2;
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, h / 2);
  ctx.fillStyle = o.fill;
  ctx.fill();
  ctx.strokeStyle = o.stroke;
  ctx.lineWidth = 2;
  ctx.stroke();
  if (o.dot) {
    ctx.shadowColor = o.dot;
    ctx.shadowBlur = o.size * 0.8;
    ctx.fillStyle = o.dot;
    ctx.beginPath();
    ctx.arc(x + padX + o.size * 0.28, midY, o.size * 0.28, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.restore();
  text(ctx, label, x + padX + dotW, midY + o.size * 0.34, { size: o.size, fill: o.color, spacing });
}

function ribbon(ctx: Ctx, d: string, width: number, stops: [number, string][], alpha: number, glow = 0, w = 1080) {
  const g = ctx.createLinearGradient(0, 0, w, 0);
  for (const [at, color] of stops) g.addColorStop(at, color);
  ctx.save();
  ctx.strokeStyle = g;
  ctx.lineWidth = width;
  ctx.lineCap = "round";
  ctx.globalAlpha = alpha;
  if (glow) {
    ctx.shadowColor = "rgba(143,176,255,0.9)";
    ctx.shadowBlur = glow;
  }
  ctx.stroke(new Path2D(d));
  ctx.restore();
}

// ---------------------------------------------------------------------------
// The cards

const DARK_TO_WHITE = (ctx: Ctx, top: number, bottom: number) =>
  vertical(ctx, top, bottom, [[0.25, "#ffffff"], [1, "#b9ccff"]]);

async function drawTime(d: ShareData, o: ShareOptions) {
  const { w, h } = cardSize("time");
  const { canvas, ctx } = newCanvas(w, h);
  const pad = 72;
  const bg = ctx.createLinearGradient(0, 0, w * 0.5, h);
  bg.addColorStop(0, "#070d2b");
  bg.addColorStop(0.48, "#101f68");
  bg.addColorStop(1, "#2447c9");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  blob(ctx, 880, 250, 400, 230, "#3f6bff", 0.9, 78);
  blob(ctx, 1000, 720, 340, 250, "#8a5cff", 0.6, 78);
  blob(ctx, 520, 1010, 460, 170, "#1fc3ff", 0.42, 78);
  blob(ctx, 760, 40, 260, 110, "#c9a8ff", 0.4, 78);
  dots(ctx, w, h, 22, 0.14, vertical(ctx, 0, h, [[0.05, "rgba(255,255,255,0)"], [0.55, "rgba(255,255,255,0.9)"], [1, "rgba(255,255,255,0.2)"]]));

  const ra: [number, string][] = [[0, "rgba(127,155,255,0)"], [0.3, "#8fb0ff"], [0.65, "#a5f0ff"], [1, "rgba(200,170,255,0.35)"]];
  const rb: [number, string][] = [[0, "rgba(154,140,255,0)"], [0.4, "#a99bff"], [0.75, "#7fd3ff"], [1, "rgba(255,255,255,0.3)"]];
  ribbon(ctx, "M0 800 C 160 700, 300 700, 440 790 S 700 930, 860 810 S 1020 700, 1080 740", 14, ra, 0.45, 26);
  ribbon(ctx, "M0 800 C 160 700, 300 700, 440 790 S 700 930, 860 810 S 1020 700, 1080 740", 3, ra, 1);
  ribbon(ctx, "M0 840 C 180 770, 320 780, 460 836 S 710 900, 870 830 S 1030 770, 1080 790", 2.2, rb, 0.9);
  ribbon(ctx, "M0 770 C 190 840, 330 860, 470 780 S 720 700, 880 780 S 1030 860, 1080 830", 1.6, ra, 0.6);
  grain(ctx, w, h, 0.32, "overlay");
  edge(ctx, w, h);

  await brand(ctx, pad, pad, 48, "#ffffff");
  if (o.streak && d.streak > 0) {
    pill(ctx, `${d.streak}-day streak`, w - pad, pad + 24, {
      size: 22,
      fill: "rgba(255,255,255,0.12)",
      stroke: "rgba(255,255,255,0.2)",
      color: "#dbe5ff",
      dot: "#9be7ff",
    });
  }

  // The number: bottom-anchored like the design, so a long time still fits.
  const parts = typed(d.savedMin);
  const gap = 28;
  let big = 280;
  const widthAt = (s: number) =>
    parts.reduce((n, p, i) => n + measure(ctx, String(p.v), s, 600, -s * 0.05) + measure(ctx, p.u, s * 0.393, 600, -2) + 8 + (i ? gap : 0), 0);
  while (big > 120 && widthAt(big) > w - pad * 2) big -= 8;
  const base = 654;
  text(ctx, "TYPING I SKIPPED", pad, base - big * 0.72 - 28, { size: 24, fill: "#a9bcff", spacing: 3.8 });
  const fill = DARK_TO_WHITE(ctx, base - big * 0.75, base);
  let x = pad;
  parts.forEach((p, i) => {
    if (i) x += gap;
    x += text(ctx, String(p.v), x, base, { size: big, fill, spacing: -big * 0.05 }) + 8;
    x += text(ctx, p.u, x, base, { size: big * 0.393, fill, spacing: -2 });
  });
  text(ctx, "just by talking to my PC.", pad, 726, { size: 42, weight: 500, fill: "#ffffff", alpha: 0.88, spacing: -0.4 });

  // Totals.
  const stats: [string, string][] = [[d.words.toLocaleString(), "words dictated"]];
  if (o.speed && d.wpm > 0) {
    stats.push([String(Math.round(d.wpm)), "words a minute"], [`${times(d.wpm)}×`, "my typing speed"]);
  } else {
    stats.push([d.dictations.toLocaleString(), d.dictations === 1 ? "dictation" : "dictations"]);
    if (d.streak > 0 && o.streak) stats.push([String(d.streak), "day streak"]);
  }
  const lineY = 800;
  ctx.fillStyle = "rgba(255,255,255,0.2)";
  ctx.fillRect(pad, lineY, w - pad * 2, 1.5);
  const col = (w - pad * 2) / 3;
  stats.slice(0, 3).forEach(([v, label], i) => {
    const cx = pad + col * i + (i ? 40 : 0);
    if (i) {
      ctx.fillStyle = "rgba(255,255,255,0.16)";
      ctx.fillRect(pad + col * i, lineY + 36, 1.5, 104);
    }
    text(ctx, v, cx, lineY + 36 + 58, { size: 64, fill: "#ffffff", spacing: -1.9 });
    text(ctx, label, cx, lineY + 36 + 96, { size: 22, weight: 400, fill: "#ffffff", alpha: 0.64 });
  });
  footer(ctx, w, h - pad, pad, "rgba(255,255,255,0.6)", 20, "Skipped time counts typing at 40 wpm");
  return canvas;
}

const WEEK_COLORS = ["#2f5bea", "#8a5cff", "#1fb6e8", "#9fb0dc"];

async function drawWeek(d: ShareData, o: ShareOptions) {
  const { w, h } = cardSize("week");
  const { canvas, ctx } = newCanvas(w, h);
  const pad = 72;
  const bg = ctx.createLinearGradient(0, 0, w * 0.3, h);
  bg.addColorStop(0, "#eef3ff");
  bg.addColorStop(0.52, "#ffffff");
  bg.addColorStop(1, "#f1eeff");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  blob(ctx, 900, 140, 360, 220, "#9fb8ff", 0.7, 90);
  blob(ctx, 120, 1280, 360, 200, "#c9f0ff", 0.7, 90);
  blob(ctx, 1000, 1180, 320, 220, "#d6c8ff", 0.6, 90);
  grain(ctx, w, h, 0.1, "multiply");

  await brand(ctx, pad, pad, 48, "#0e1a3d");
  const first = d.week[0]?.date;
  const last = d.week[d.week.length - 1]?.date;
  if (first && last) {
    const f = (s: string) => new Date(`${s}T00:00:00`).toLocaleDateString([], { month: "short", day: "numeric" });
    text(ctx, `${f(first)} to ${f(last)}`, w - pad, pad + 34, { size: 22, fill: "#4a5a8c", align: "right" });
  }

  const total = weekWords(d);
  text(ctx, "MY WEEK, OUT LOUD", pad, 220, { size: 22, fill: "#2f5bea", spacing: 3.5 });
  const numText = total.toLocaleString();
  const numSize = fit(ctx, numText, 196, w - pad * 2, 600, -0.05);
  const numFill = ctx.createLinearGradient(pad, 0, pad + measure(ctx, numText, numSize, 600, -numSize * 0.05), 0);
  numFill.addColorStop(0.3, "#0e1a3d");
  numFill.addColorStop(1, "#2f5bea");
  text(ctx, numText, pad, 390, { size: numSize, fill: numFill, spacing: -numSize * 0.05 });
  text(ctx, "words spoken instead of typed.", pad, 448, { size: 38, weight: 500, fill: "#3d4a73", spacing: -0.4 });

  // The seven days.
  const cardX = pad;
  const cardY = 500;
  const cardW = w - pad * 2;
  const cardH = 410;
  ctx.save();
  ctx.shadowColor = "rgba(47,91,234,0.3)";
  ctx.shadowBlur = 50;
  ctx.shadowOffsetY = 28;
  ctx.beginPath();
  ctx.roundRect(cardX, cardY, cardW, cardH, 36);
  ctx.fillStyle = "rgba(255,255,255,0.82)";
  ctx.fill();
  ctx.restore();
  ctx.beginPath();
  ctx.roundRect(cardX, cardY, cardW, cardH, 36);
  ctx.strokeStyle = "#dfe7fb";
  ctx.lineWidth = 2;
  ctx.stroke();

  const days = d.week.slice(-7);
  const most = Math.max(1, ...days.map((x) => x.words));
  const inner = cardW - 88;
  const gapX = 22;
  const colW = (inner - gapX * (days.length - 1)) / days.length;
  const barBase = cardY + 40 + 300;
  days.forEach((day, i) => {
    const best = day.words === most && most > 0;
    const bx = cardX + 44 + i * (colW + gapX);
    const bh = Math.max(10, Math.round((day.words / most) * 220));
    const by = barBase - bh;
    ctx.save();
    ctx.beginPath();
    ctx.roundRect(bx, by, colW, bh, 16);
    const g = ctx.createLinearGradient(0, by, 0, barBase);
    if (best) {
      g.addColorStop(0, "#5f86ff");
      g.addColorStop(1, "#8a5cff");
      ctx.shadowColor = "rgba(95,134,255,0.6)";
      ctx.shadowBlur = 28;
      ctx.shadowOffsetY = 14;
    } else {
      g.addColorStop(0, "#d6e1ff");
      g.addColorStop(1, "#c2d2fb");
    }
    ctx.fillStyle = g;
    ctx.fill();
    ctx.restore();
    text(ctx, day.words.toLocaleString(), bx + colW / 2, by - 12, { size: 20, fill: best ? "#2f5bea" : "#6b7aa8", align: "center", spacing: 0 });
    const name = new Date(`${day.date}T00:00:00`).toLocaleDateString([], { weekday: "short" });
    text(ctx, name, bx + colW / 2, barBase + 36, { size: 20, fill: best ? "#0e1a3d" : "#7683ad", align: "center" });
  });

  // Three tiles.
  const best = days.reduce((m, x) => (x.words > m.words ? x : m), days[0]);
  const tiles: [string, string][] = [[typedText(total / TYPING_WPM), "typing skipped"]];
  tiles.push(
    o.speed && d.wpm > 0
      ? [String(Math.round(d.wpm)), "words a minute"]
      : [new Date(`${best.date}T00:00:00`).toLocaleDateString([], { weekday: "long" }), "busiest day"],
  );
  tiles.push(o.streak && d.streak > 0 ? [String(d.streak), "day streak"] : [d.dictations.toLocaleString(), "dictations in all"]);
  const tileY = cardY + cardH + 38;
  const tileGap = 20;
  const tileW = (cardW - tileGap * 2) / 3;
  tiles.forEach(([v, label], i) => {
    const tx = pad + i * (tileW + tileGap);
    ctx.beginPath();
    ctx.roundRect(tx, tileY, tileW, 132, 28);
    ctx.fillStyle = i === 0 ? "#0e1a3d" : "rgba(255,255,255,0.82)";
    ctx.fill();
    if (i) {
      ctx.strokeStyle = "#dfe7fb";
      ctx.lineWidth = 2;
      ctx.stroke();
    }
    const size = fit(ctx, v, 54, tileW - 56, 600, -0.03);
    text(ctx, v, tx + 28, tileY + 68, { size, fill: i === 0 ? "#ffffff" : "#0e1a3d", spacing: -size * 0.03 });
    text(ctx, label, tx + 28, tileY + 104, { size: 20, weight: 400, fill: i === 0 ? "#a9bcff" : "#5a6a99" });
  });

  // Where the words went, only if allowed.
  if (o.apps && d.apps.length > 0) {
    const top = d.apps.slice(0, 4);
    const sum = top.reduce((n, a) => n + a.words, 0) || 1;
    const stripY = tileY + 132 + 50;
    let x = pad;
    top.forEach((a, i) => {
      const bw = Math.max(18, ((cardW - 4 * (top.length - 1)) * a.words) / sum);
      ctx.beginPath();
      ctx.roundRect(x, stripY, bw, 18, 9);
      ctx.fillStyle = WEEK_COLORS[i];
      ctx.fill();
      x += bw + 4;
    });
    let lx = pad;
    top.forEach((a, i) => {
      const pct = Math.round((a.words / sum) * 100);
      ctx.fillStyle = WEEK_COLORS[i];
      ctx.beginPath();
      ctx.arc(lx + 6, stripY + 52, 6, 0, Math.PI * 2);
      ctx.fill();
      lx += 18;
      lx += text(ctx, a.name, lx, stripY + 58, { size: 21, weight: 500, fill: "#3d4a73" }) + 8;
      lx += text(ctx, `${pct}%`, lx, stripY + 58, { size: 21, weight: 500, fill: "#7683ad" }) + 30;
    });
  }
  footer(ctx, w, h - pad, pad, "#5a6a99", 20, "Skipped time counts typing at 40 wpm");
  return canvas;
}

async function drawSpeed(d: ShareData) {
  const { w, h } = cardSize("speed");
  const { canvas, ctx } = newCanvas(w, h);
  const padX = 64;
  const padY = 56;
  ctx.fillStyle = "#0a0c14";
  ctx.fillRect(0, 0, w, h);
  blob(ctx, 1010, 260, 300, 200, "#3f6bff", 0.75, 70);
  blob(ctx, 1120, 560, 240, 150, "#8a5cff", 0.5, 70);
  blob(ctx, 260, 150, 260, 90, "#1fc3ff", 0.18, 70);
  const fade = ctx.createLinearGradient(0, 0, w, 0);
  fade.addColorStop(0.4, "rgba(255,255,255,0)");
  fade.addColorStop(1, "rgba(255,255,255,0.9)");
  dots(ctx, w, h, 20, 0.12, fade);
  grain(ctx, w, h, 0.3, "overlay");
  edge(ctx, w, h, 0.1);

  await brand(ctx, padX, padY, 40, "#ffffff");
  text(ctx, "MY DICTATION SPEED", padX + 40 + 14 + 78 + 30, padY + 28, { size: 18, fill: "#8aa8ff", spacing: 2.5 });

  const colW = w - padX * 2 - 380 - 56;
  const max = Math.max(d.wpm, TYPING_WPM);
  const rows: [string, number, boolean][] = [["Talking", d.wpm, true], ["Typing", TYPING_WPM, false]];
  rows.forEach(([label, value, hot], i) => {
    const y = 262 + i * 86;
    text(ctx, label, padX, y, { size: 26, fill: hot ? "#ffffff" : "#9aa3bd" });
    const vw = text(ctx, "wpm", padX + colW, y, { size: 18, weight: 500, fill: hot ? "#7f93c9" : "#6d7691", align: "right" });
    text(ctx, String(Math.round(value)), padX + colW - vw - 8, y, { size: 26, fill: hot ? "#bcd0ff" : "#9aa3bd", align: "right" });
    ctx.beginPath();
    ctx.roundRect(padX, y + 14, colW, 26, 13);
    ctx.fillStyle = "rgba(255,255,255,0.08)";
    ctx.fill();
    const fw = Math.max(26, (colW * value) / max);
    ctx.save();
    ctx.beginPath();
    ctx.roundRect(padX, y + 14, fw, 26, 13);
    if (hot) {
      const g = ctx.createLinearGradient(padX, 0, padX + fw, 0);
      g.addColorStop(0, "#5f86ff");
      g.addColorStop(0.7, "#8a5cff");
      g.addColorStop(1, "#b79bff");
      ctx.fillStyle = g;
      ctx.shadowColor = "rgba(95,134,255,0.65)";
      ctx.shadowBlur = 36;
    } else {
      ctx.fillStyle = "#4a5068";
    }
    ctx.fill();
    ctx.restore();
  });

  const rx = padX + colW + 56;
  const ratio = `${times(d.wpm)}×`;
  const size = fit(ctx, ratio, 200, w - padX - rx, 600, -0.06);
  text(ctx, ratio, rx, 372, { size, fill: vertical(ctx, 372 - size * 0.75, 372, [[0.2, "#ffffff"], [1, "#aebfff"]]), spacing: -size * 0.06 });
  text(ctx, "faster than I type.", rx, 372 + 54, { size: 34, weight: 500, fill: "#ffffff", alpha: 0.88, spacing: -0.3 });

  footer(ctx, w, h - padY, padX, "rgba(255,255,255,0.56)", 18, "Typing speed assumed: 40 wpm");
  return canvas;
}

async function drawMilestone(d: ShareData, o: ShareOptions) {
  const { w, h } = cardSize("milestone");
  const { canvas, ctx } = newCanvas(w, h);
  const pad = 72;
  const m = milestoneReached(d.words) ?? d.words;
  ctx.fillStyle = "#060814";
  ctx.fillRect(0, 0, w, h);
  blob(ctx, 540, 620, 420, 330, "#2447c9", 0.8, 90);
  blob(ctx, 860, 330, 260, 200, "#8a5cff", 0.5, 90);
  blob(ctx, 240, 940, 280, 200, "#1fc3ff", 0.32, 90);
  blob(ctx, 540, 620, 190, 150, "#ffd79a", 0.28, 90);

  // A circular sound wave: a bar every 3 degrees, lengths from layered sines.
  const n = 120;
  const cx = 540;
  const cy = 640;
  const r0 = 292;
  const bars = Array.from({ length: n }, (_, i) => {
    const t = i / n;
    const a = t * Math.PI * 2 - Math.PI / 2;
    const swell = 0.55 + 0.45 * Math.sin(t * Math.PI * 2 * 3 + 0.6);
    const detail = 0.5 + 0.5 * Math.sin(t * Math.PI * 2 * 17) * Math.cos(t * Math.PI * 2 * 7);
    const len = 16 + 112 * swell * (0.35 + 0.65 * Math.abs(detail));
    const hue = 214 + 52 * Math.sin(t * Math.PI * 2 - 0.4);
    return {
      x1: cx + Math.cos(a) * r0,
      y1: cy + Math.sin(a) * r0,
      x2: cx + Math.cos(a) * (r0 + len),
      y2: cy + Math.sin(a) * (r0 + len),
      color: `hsl(${hue.toFixed(0)} 92% ${(66 + 14 * swell).toFixed(0)}%)`,
    };
  });
  for (const pass of [0, 1]) {
    ctx.save();
    ctx.lineCap = "round";
    ctx.lineWidth = pass ? 5 : 7;
    ctx.globalAlpha = pass ? 1 : 0.65;
    for (const b of bars) {
      ctx.strokeStyle = b.color;
      if (!pass) {
        ctx.shadowColor = b.color;
        ctx.shadowBlur = 14;
      }
      ctx.beginPath();
      ctx.moveTo(b.x1, b.y1);
      ctx.lineTo(b.x2, b.y2);
      ctx.stroke();
    }
    ctx.restore();
  }
  ctx.save();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "rgba(255,255,255,0.14)";
  ctx.beginPath();
  ctx.arc(cx, cy, 268, 0, Math.PI * 2);
  ctx.stroke();
  ctx.setLineDash([2, 10]);
  ctx.lineWidth = 1;
  ctx.strokeStyle = "rgba(255,215,154,0.35)";
  ctx.beginPath();
  ctx.arc(cx, cy, 250, 0, Math.PI * 2);
  ctx.stroke();
  ctx.restore();
  const vig = ctx.createRadialGradient(cx, 634, 330, cx, 634, 800);
  vig.addColorStop(0, "rgba(6,8,20,0)");
  vig.addColorStop(1, "rgba(6,8,20,0.85)");
  ctx.fillStyle = vig;
  ctx.fillRect(0, 0, w, h);
  grain(ctx, w, h, 0.34, "overlay");
  edge(ctx, w, h);

  await brand(ctx, pad, pad, 48, "#ffffff");
  pill(ctx, "MILESTONE", w - pad, pad + 24, {
    size: 20,
    fill: "rgba(255,215,154,0.1)",
    stroke: "rgba(255,215,154,0.5)",
    color: "#ffe2b0",
    caps: true,
  });

  const num = m.toLocaleString();
  const size = fit(ctx, num, 172, 540, 600, -0.05);
  const gold = vertical(ctx, 668 - size * 0.75, 668, [[0.15, "#ffffff"], [0.58, "#ffe2b0"], [1, "#e9b86c"]]);
  text(ctx, num, cx, 668, { size, fill: gold, align: "center", spacing: -size * 0.05 });
  text(ctx, "words, none of them typed.", cx, 668 + 62, { size: 36, weight: 500, fill: "#ffffff", alpha: 0.86, spacing: -0.3 });

  const facts: [string, string][] = [[d.dictations.toLocaleString(), d.dictations === 1 ? "dictation" : "dictations"], [typedText(m / TYPING_WPM), "of typing, at 40 wpm"]];
  if (o.speed && d.wpm > 0) facts.push([String(Math.round(d.wpm)), "words a minute"]);
  else if (o.streak && d.streak > 0) facts.push([String(d.streak), "day streak"]);
  const col = (w - pad * 2) / Math.max(facts.length, 1);
  facts.forEach(([v, label], i) => {
    const x = pad + col * (i + 0.5);
    text(ctx, v, x, 1150, { size: 52, fill: "#ffffff", align: "center", spacing: -1.5 });
    text(ctx, label, x, 1184, { size: 20, weight: 400, fill: "#ffffff", alpha: 0.6, align: "center" });
  });
  ctx.fillStyle = "rgba(255,255,255,0.16)";
  ctx.fillRect(pad, 1226, w - pad * 2, 1.5);
  footer(ctx, w, h - pad, pad, "rgba(255,255,255,0.6)", 20, "Skipped time counts typing at 40 wpm");
  return canvas;
}


/** The strip of where the words went, shared by the week and month cards. */
function appsStrip(ctx: Ctx, d: ShareData, x0: number, y: number, width: number) {
  const top = d.apps.slice(0, 4);
  const sum = top.reduce((n, a) => n + a.words, 0) || 1;
  let x = x0;
  top.forEach((a, i) => {
    const bw = Math.max(18, ((width - 4 * (top.length - 1)) * a.words) / sum);
    ctx.beginPath();
    ctx.roundRect(x, y, bw, 18, 9);
    ctx.fillStyle = WEEK_COLORS[i];
    ctx.fill();
    x += bw + 4;
  });
  let lx = x0;
  top.forEach((a, i) => {
    const pct = Math.round((a.words / sum) * 100);
    ctx.fillStyle = WEEK_COLORS[i];
    ctx.beginPath();
    ctx.arc(lx + 6, y + 52, 6, 0, Math.PI * 2);
    ctx.fill();
    lx += 18;
    lx += text(ctx, a.name, lx, y + 58, { size: 21, weight: 500, fill: "#3d4a73" }) + 8;
    lx += text(ctx, `${pct}%`, lx, y + 58, { size: 21, weight: 500, fill: "#7683ad" }) + 30;
  });
}

const HEAT = ["#e9edf9", "#d3defc", "#a9bef8", "#6f8ff2", "#2f5bea"];

async function drawMonth(d: ShareData, o: ShareOptions) {
  const { w, h } = cardSize("month");
  const { canvas, ctx } = newCanvas(w, h);
  const pad = 72;
  const m = monthInfo(d);
  const bg = ctx.createLinearGradient(0, 0, w * 0.3, h);
  bg.addColorStop(0, "#eef3ff");
  bg.addColorStop(0.52, "#ffffff");
  bg.addColorStop(1, "#f1eeff");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  blob(ctx, 900, 140, 360, 220, "#9fb8ff", 0.7, 90);
  blob(ctx, 120, 1280, 360, 200, "#c9f0ff", 0.7, 90);
  blob(ctx, 1000, 1180, 320, 220, "#d6c8ff", 0.6, 90);
  grain(ctx, w, h, 0.1, "multiply");

  await brand(ctx, pad, pad, 48, "#0e1a3d");
  text(ctx, m.label, w - pad, pad + 34, { size: 22, fill: "#4a5a8c", align: "right" });
  text(ctx, "MY MONTH, OUT LOUD", pad, 214, { size: 22, fill: "#2f5bea", spacing: 3.5 });
  const numText = m.words.toLocaleString();
  const numSize = fit(ctx, numText, 176, w - pad * 2, 600, -0.05);
  const numFill = ctx.createLinearGradient(pad, 0, pad + measure(ctx, numText, numSize, 600, -numSize * 0.05), 0);
  numFill.addColorStop(0.3, "#0e1a3d");
  numFill.addColorStop(1, "#2f5bea");
  text(ctx, numText, pad, 372, { size: numSize, fill: numFill, spacing: -numSize * 0.05 });
  text(ctx, "words spoken instead of typed.", pad, 424, { size: 38, weight: 500, fill: "#3d4a73", spacing: -0.4 });

  // The month as a heat calendar, Monday first.
  const lead = (new Date(m.now.getFullYear(), m.now.getMonth(), 1).getDay() + 6) % 7;
  const rows = Math.ceil((lead + m.length) / 7);
  const cardX = pad;
  const cardY = 466;
  const cardW = w - pad * 2;
  const gap = 10;
  const cellH = 64;
  const cardH = 128 + rows * cellH + (rows - 1) * gap + 30;
  ctx.save();
  ctx.shadowColor = "rgba(47,91,234,0.3)";
  ctx.shadowBlur = 50;
  ctx.shadowOffsetY = 28;
  ctx.beginPath();
  ctx.roundRect(cardX, cardY, cardW, cardH, 36);
  ctx.fillStyle = "rgba(255,255,255,0.82)";
  ctx.fill();
  ctx.restore();
  ctx.beginPath();
  ctx.roundRect(cardX, cardY, cardW, cardH, 36);
  ctx.strokeStyle = "#dfe7fb";
  ctx.lineWidth = 2;
  ctx.stroke();

  text(ctx, "Every day you dictated", cardX + 40, cardY + 58, { size: 22, fill: "#0e1a3d" });
  // Legend, drawn from the right edge inwards.
  let lx = cardX + cardW - 40;
  lx -= text(ctx, "More", lx, cardY + 56, { size: 18, weight: 400, fill: "#6b7aa8", align: "right" }) + 8;
  for (let i = HEAT.length - 1; i >= 0; i--) {
    ctx.beginPath();
    ctx.roundRect(lx - 16, cardY + 42, 16, 16, 4);
    ctx.fillStyle = HEAT[i];
    ctx.fill();
    lx -= 24;
  }
  text(ctx, "Less", lx - 4, cardY + 56, { size: 18, weight: 400, fill: "#6b7aa8", align: "right" });

  const cw = (cardW - 80 - gap * 6) / 7;
  ["M", "T", "W", "T", "F", "S", "S"].forEach((letter, i) => {
    text(ctx, letter, cardX + 40 + i * (cw + gap) + cw / 2, cardY + 106, { size: 18, fill: "#7683ad", align: "center" });
  });
  const byDay = new Map(m.mine.map((x) => [x.date, x.words]));
  const most = Math.max(1, ...m.mine.map((x) => x.words));
  const bestDate = [...m.mine].sort((a, b) => b.words - a.words)[0]?.date;
  for (let n = 1; n <= m.length; n++) {
    const date = `${m.prefix}-${String(n).padStart(2, "0")}`;
    const col = (lead + n - 1) % 7;
    const row = Math.floor((lead + n - 1) / 7);
    const x = cardX + 40 + col * (cw + gap);
    const y = cardY + 128 + row * (cellH + gap);
    const words = byDay.get(date) ?? 0;
    const future = n > m.today;
    const ratio = words / most;
    const level = words <= 0 ? 0 : ratio <= 0.25 ? 1 : ratio <= 0.5 ? 2 : ratio <= 0.75 ? 3 : 4;
    ctx.save();
    ctx.beginPath();
    ctx.roundRect(x, y, cw, cellH, 14);
    ctx.fillStyle = future ? "#f3f4f8" : HEAT[level];
    if (level === 4 && !future) {
      ctx.shadowColor = "rgba(47,91,234,0.55)";
      ctx.shadowBlur = 18;
      ctx.shadowOffsetY = 8;
    }
    ctx.fill();
    ctx.restore();
    if (date === bestDate) {
      ctx.beginPath();
      ctx.roundRect(x, y, cw, cellH, 14);
      ctx.strokeStyle = "#0e1a3d";
      ctx.lineWidth = 3;
      ctx.stroke();
    }
    text(ctx, String(n), x + cw / 2, y + cellH / 2 + 7, {
      size: 20,
      weight: date === bestDate ? 700 : 500,
      fill: future ? "#b8bfd4" : level >= 3 ? "#ffffff" : "#0e1a3d",
      align: "center",
    });
  }

  const best = m.mine.find((x) => x.date === bestDate);
  const tiles: [string, string][] = [[typedText(m.words / TYPING_WPM), "typing skipped"]];
  tiles.push(
    o.speed && d.wpm > 0
      ? [String(Math.round(d.wpm)), "words a minute"]
      : [best ? best.words.toLocaleString() : "0", best ? `on my best day, ${shortDate(best.date)}` : "on my best day"],
  );
  tiles.push(
    o.streak && d.streak > 0
      ? [String(d.streak), "day streak"]
      : [String(m.mine.filter((x) => x.words > 0).length), "days with dictation"],
  );
  const tileY = cardY + cardH + 24;
  const tileGap = 20;
  const tileW = (cardW - tileGap * 2) / 3;
  tiles.forEach(([v, label], i) => {
    const tx = pad + i * (tileW + tileGap);
    ctx.beginPath();
    ctx.roundRect(tx, tileY, tileW, 112, 28);
    ctx.fillStyle = i === 0 ? "#0e1a3d" : "rgba(255,255,255,0.82)";
    ctx.fill();
    if (i) {
      ctx.strokeStyle = "#dfe7fb";
      ctx.lineWidth = 2;
      ctx.stroke();
    }
    const size = fit(ctx, v, 50, tileW - 52, 600, -0.03);
    text(ctx, v, tx + 26, tileY + 58, { size, fill: i === 0 ? "#ffffff" : "#0e1a3d", spacing: -size * 0.03 });
    text(ctx, label, tx + 26, tileY + 92, { size: 20, weight: 400, fill: i === 0 ? "#a9bcff" : "#5a6a99" });
  });

  // The apps strip only when there is room (a five-row month).
  if (o.apps && d.apps.length > 0 && rows <= 5) appsStrip(ctx, d, pad, tileY + 112 + 36, cardW);
  footer(ctx, w, h - pad, pad, "#5a6a99", 20, "Skipped time counts typing at 40 wpm");
  return canvas;
}

async function drawAllTime(d: ShareData, o: ShareOptions) {
  const { w, h } = cardSize("alltime");
  const { canvas, ctx } = newCanvas(w, h);
  const pad = 72;
  const bg = ctx.createLinearGradient(0, 0, w * 0.45, h);
  bg.addColorStop(0, "#070d2b");
  bg.addColorStop(0.55, "#101f68");
  bg.addColorStop(1, "#2447c9");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  blob(ctx, 900, 220, 400, 240, "#3f6bff", 0.85, 80);
  blob(ctx, 1000, 760, 320, 240, "#8a5cff", 0.5, 80);
  blob(ctx, 480, 1300, 460, 170, "#1fc3ff", 0.4, 80);
  dots(ctx, w, h, 22, 0.12, vertical(ctx, 0, h, [[0, "rgba(255,255,255,0.9)"], [0.5, "rgba(255,255,255,0)"]]));
  grain(ctx, w, h, 0.32, "overlay");
  edge(ctx, w, h);

  const DAY = 86_400_000;
  const first = d.firstDay ?? d.days[0]?.date;
  const start = new Date(`${first}T12:00:00`).getTime();
  const end = new Date().setHours(12, 0, 0, 0);
  const span = Math.max(1, Math.round((end - start) / DAY));

  await brand(ctx, pad, pad, 48, "#ffffff");
  pill(ctx, `Since ${shortDate(first)} · ${span + 1} days`, w - pad, pad + 24, {
    size: 22,
    fill: "rgba(255,255,255,0.12)",
    stroke: "rgba(255,255,255,0.2)",
    color: "#dbe5ff",
  });

  const totalText = d.words.toLocaleString();
  const size = fit(ctx, totalText, 200, w - pad * 2, 600, -0.05);
  text(ctx, "SO FAR", pad, 384 - size * 0.74 - 28, { size: 22, fill: "#a9bcff", spacing: 3.8 });
  text(ctx, totalText, pad, 384, {
    size,
    fill: vertical(ctx, 384 - size * 0.75, 384, [[0.25, "#ffffff"], [1, "#b9ccff"]]),
    spacing: -size * 0.05,
  });
  text(ctx, "words, none of them typed.", pad, 384 + 64, { size: 40, weight: 500, fill: "#ffffff", alpha: 0.88, spacing: -0.4 });

  // The climb: your running total, day by day.
  const cw = w - pad * 2;
  const top = 520;
  const chartH = 410;
  const bottom = top + chartH;
  const plot = chartH - 44;
  const words = new Map(d.days.map((x) => [x.date, x.words]));
  const listed = d.days.reduce((n, x) => n + x.words, 0);
  let run = Math.max(0, d.words - listed); // words from days older than the list
  const pts: { x: number; y: number; run: number }[] = [];
  for (let i = 0; i <= span; i++) {
    const t = new Date(start + i * DAY);
    const key = `${t.getFullYear()}-${String(t.getMonth() + 1).padStart(2, "0")}-${String(t.getDate()).padStart(2, "0")}`;
    run += words.get(key) ?? 0;
    pts.push({ x: pad + (i / span) * cw, y: bottom - (run / Math.max(1, d.words)) * plot, run });
  }
  ctx.save();
  ctx.strokeStyle = "rgba(255,255,255,0.14)";
  ctx.lineWidth = 1.5;
  ctx.setLineDash([3, 8]);
  for (const f of [0.25, 0.5, 0.75]) {
    ctx.beginPath();
    ctx.moveTo(pad, bottom - plot * f);
    ctx.lineTo(pad + cw, bottom - plot * f);
    ctx.stroke();
  }
  ctx.restore();

  const trace = () => {
    ctx.beginPath();
    pts.forEach((p, i) => (i ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y)));
  };
  ctx.save();
  trace();
  ctx.lineTo(pad + cw, bottom);
  ctx.lineTo(pad, bottom);
  ctx.closePath();
  const area = ctx.createLinearGradient(0, top, 0, bottom);
  area.addColorStop(0, "rgba(143,176,255,0.55)");
  area.addColorStop(1, "rgba(143,176,255,0)");
  ctx.fillStyle = area;
  ctx.fill();
  ctx.restore();
  const lineFill = ctx.createLinearGradient(pad, 0, pad + cw, 0);
  lineFill.addColorStop(0, "#7f9bff");
  lineFill.addColorStop(0.6, "#a5f0ff");
  lineFill.addColorStop(1, "#c8aaff");
  ctx.save();
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  ctx.strokeStyle = lineFill;
  ctx.shadowColor = "rgba(143,176,255,0.9)";
  ctx.shadowBlur = 26;
  ctx.lineWidth = 6;
  ctx.globalAlpha = 0.6;
  trace();
  ctx.stroke();
  ctx.restore();
  ctx.save();
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  ctx.strokeStyle = lineFill;
  ctx.lineWidth = 4;
  trace();
  ctx.stroke();
  ctx.restore();

  // A ring where each round number was passed.
  const passed = MILESTONES.filter((m) => m >= 1_000 && m <= d.words).slice(-4);
  let lastLabelX = -999;
  for (const m of passed) {
    const p = pts.find((q) => q.run >= m);
    if (!p) continue;
    ctx.save();
    ctx.strokeStyle = "rgba(255,255,255,0.22)";
    ctx.lineWidth = 1.5;
    ctx.setLineDash([3, 6]);
    ctx.beginPath();
    ctx.moveTo(p.x, p.y);
    ctx.lineTo(p.x, bottom);
    ctx.stroke();
    ctx.restore();
    ctx.beginPath();
    ctx.arc(p.x, p.y, 11, 0, Math.PI * 2);
    ctx.fillStyle = "#070d2b";
    ctx.fill();
    ctx.strokeStyle = "#ffd79a";
    ctx.lineWidth = 4;
    ctx.stroke();
    if (Math.abs(p.x - lastLabelX) > 74) {
      const label = `${m / 1000}k`;
      const tw = measure(ctx, label, 18, 600);
      const lx = Math.max(pad, Math.min(pad + cw - tw - 24, p.x - tw / 2 - 12));
      ctx.beginPath();
      ctx.roundRect(lx, p.y - 58, tw + 24, 32, 16);
      ctx.fillStyle = "rgba(255,215,154,0.14)";
      ctx.fill();
      ctx.strokeStyle = "rgba(255,215,154,0.45)";
      ctx.lineWidth = 1.5;
      ctx.stroke();
      text(ctx, label, lx + 12, p.y - 36, { size: 18, fill: "#ffe2b0" });
      lastLabelX = p.x;
    }
  }
  const tip = pts[pts.length - 1];
  ctx.beginPath();
  ctx.arc(tip.x, tip.y, 14, 0, Math.PI * 2);
  ctx.fillStyle = "#ffffff";
  ctx.fill();
  ctx.beginPath();
  ctx.arc(tip.x, tip.y, 26, 0, Math.PI * 2);
  ctx.strokeStyle = "rgba(255,255,255,0.4)";
  ctx.lineWidth = 2;
  ctx.stroke();
  text(ctx, shortDate(first), pad, bottom + 34, { size: 18, weight: 400, fill: "#ffffff", alpha: 0.6 });
  text(ctx, "Today", pad + cw, bottom + 34, { size: 18, weight: 400, fill: "#ffffff", alpha: 0.6, align: "right" });

  // Totals.
  const active = d.days.filter((x) => x.words > 0).length;
  const stats: [string, string][] = [
    [typedText(d.words / TYPING_WPM), "typing skipped"],
    o.speed && d.wpm > 0 ? [String(Math.round(d.wpm)), "words a minute"] : [String(active), "days with dictation"],
    o.streak && d.longestStreak > 0 ? [String(d.longestStreak), "day best streak"] : [String(span + 1), "days since I started"],
    [d.dictations.toLocaleString(), d.dictations === 1 ? "dictation" : "dictations"],
  ];
  const lineY = 1010;
  ctx.fillStyle = "rgba(255,255,255,0.2)";
  ctx.fillRect(pad, lineY, cw, 1.5);
  const col = cw / 4;
  stats.forEach(([v, label], i) => {
    const x = pad + col * i + (i ? 28 : 0);
    if (i) {
      ctx.fillStyle = "rgba(255,255,255,0.16)";
      ctx.fillRect(pad + col * i, lineY + 30, 1.5, 100);
    }
    const sz = fit(ctx, v, 52, col - 44, 600, -0.03);
    text(ctx, v, x, lineY + 30 + 54, { size: sz, fill: "#ffffff", spacing: -sz * 0.03 });
    text(ctx, label, x, lineY + 30 + 92, { size: 20, weight: 400, fill: "#ffffff", alpha: 0.64 });
  });
  footer(ctx, w, h - pad, pad, "rgba(255,255,255,0.6)", 20, "Skipped time counts typing at 40 wpm");
  return canvas;
}

/** Paints one card at its posting size. Rejects if the card can't be made. */
export async function drawCard(kind: CardKind, d: ShareData, o: ShareOptions): Promise<HTMLCanvasElement> {
  const why = unavailable(kind, d);
  if (why) throw new Error(why);
  switch (kind) {
    case "time":
      return drawTime(d, o);
    case "week":
      return drawWeek(d, o);
    case "month":
      return drawMonth(d, o);
    case "alltime":
      return drawAllTime(d, o);
    case "speed":
      return drawSpeed(d);
    case "milestone":
      return drawMilestone(d, o);
  }
}
