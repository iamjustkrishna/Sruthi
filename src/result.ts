import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const card = document.getElementById("card")!;
const text = document.getElementById("text")!;
const copy = document.getElementById("copy") as HTMLButtonElement;
const timer = document.getElementById("timer")!;

/** Empty space around the card inside the window (body padding). */
const WINDOW_PADDING = 12;

let current = "";
let hideTimer: number | undefined;

function hide() {
  window.clearTimeout(hideTimer);
  card.classList.remove("show");
  // Let the fade-out finish before the window goes away.
  window.setTimeout(() => invoke("hide_result"), 180);
}

/** The card's height with the text unconstrained (no scrolling). */
function naturalHeight() {
  card.classList.add("measuring");
  const height = card.offsetHeight + WINDOW_PADDING;
  card.classList.remove("measuring");
  return height;
}

function restartCountdown() {
  timer.classList.remove("running");
  void timer.offsetWidth; // restart the CSS animation
  timer.classList.add("running");
}

listen<string>("result-show", async ({ payload }) => {
  window.clearTimeout(hideTimer);
  current = payload;
  text.textContent = payload;
  copy.textContent = "Copy";
  copy.classList.remove("done");
  card.classList.remove("show");
  // Fit the window to the text, then reveal it.
  await invoke("place_result", { height: naturalHeight() });
  void card.offsetWidth;
  card.classList.add("show");
  restartCountdown();
});

listen("result-hide", () => {
  window.clearTimeout(hideTimer);
  timer.classList.remove("running");
  card.classList.remove("show");
});

// The countdown bar is the timeout: it pauses while the pointer is over the
// box (CSS), and the box closes when it runs out.
timer.addEventListener("animationend", hide);

copy.addEventListener("click", async () => {
  await invoke("copy_text", { text: current });
  copy.textContent = "Copied ✓";
  copy.classList.add("done");
  timer.classList.remove("running");
  hideTimer = window.setTimeout(hide, 1200);
});

document.getElementById("close")!.addEventListener("click", hide);
