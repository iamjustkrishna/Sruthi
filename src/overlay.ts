import { listen } from "@tauri-apps/api/event";

type Dictation =
  | { phase: "recording" }
  | { phase: "transcribing" }
  | { phase: "done"; text: string }
  | { phase: "error"; message: string };

const BAR_COUNT = 18;

const pill = document.getElementById("pill")!;
const mark = document.getElementById("mark")!;
const wave = document.getElementById("wave")!;
const label = document.getElementById("label")!;
const live = document.getElementById("live")!;

/** Fits text into the pill by keeping the end (the newest words). */
const LIVE_CHARS = 54;
function tail(text: string) {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= LIVE_CHARS) return flat;
  const cut = flat.slice(-LIVE_CHARS);
  const space = cut.indexOf(" ");
  return "…" + (space > 0 && space < 12 ? cut.slice(space + 1) : cut);
}

function setLive(text: string) {
  live.textContent = tail(text);
  pill.classList.toggle("has-text", live.textContent !== "");
}

const bars = Array.from({ length: BAR_COUNT }, (_, i) => {
  const bar = document.createElement("i");
  bar.style.setProperty("--i", String(i));
  wave.append(bar);
  return bar;
});

// Newest level on the right; older levels scroll left like a waveform.
const levels = new Array<number>(BAR_COUNT).fill(0);
let target = 0;
let smoothed = 0;

/** Maps raw mic RMS (speech is roughly 0.01–0.2) to 0–1. */
function normalize(rms: number) {
  const gated = Math.max(0, rms - 0.004);
  return Math.min(1, Math.sqrt(gated) * 2.6);
}

let lastShift = 0;
function frame(now: number) {
  if (pill.dataset.phase === "recording") {
    // Rise fast, fall slowly, so the motion feels responsive but calm.
    smoothed += (target - smoothed) * (target > smoothed ? 0.5 : 0.15);
    if (now - lastShift > 45) {
      levels.shift();
      levels.push(smoothed);
      lastShift = now;
    }
    bars.forEach((bar, i) => {
      // Taper the ends so the wave sits inside the pill nicely.
      const edge = Math.sin(((i + 0.5) / BAR_COUNT) * Math.PI);
      const h = 0.14 + levels[i] * (0.3 + 0.7 * edge);
      bar.style.transform = `scaleY(${Math.min(1, h)})`;
    });
    // The logo breathes and its halo glows with your voice.
    mark.style.setProperty("--level", smoothed.toFixed(3));
  }
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

function enter() {
  pill.classList.remove("exit", "enter");
  void pill.offsetWidth; // restart the animation
  pill.classList.add("enter");
}

function isShowing() {
  return pill.classList.contains("enter");
}

listen<Dictation>("dictation", ({ payload }) => {
  pill.dataset.phase = payload.phase;
  switch (payload.phase) {
    case "recording":
      levels.fill(0);
      target = smoothed = 0;
      mark.style.setProperty("--level", "0");
      bars.forEach((b) => (b.style.transform = ""));
      label.textContent = "";
      setLive("");
      enter();
      break;
    case "transcribing":
      mark.style.setProperty("--level", "0");
      bars.forEach((b) => (b.style.transform = ""));
      label.textContent = "";
      break;
    case "done":
      // Show the final, formatted text for a moment.
      setLive(payload.text);
      label.textContent = "";
      break;
    case "error":
      setLive("");
      label.textContent = payload.message;
      if (!isShowing()) enter();
      break;
  }
});

listen<number>("audio-level", ({ payload }) => {
  target = normalize(payload);
});

listen<string>("overlay-align", ({ payload }) => {
  document.body.dataset.align = payload;
});

listen("overlay-exit", () => {
  pill.classList.remove("enter");
  pill.classList.add("exit");
});
