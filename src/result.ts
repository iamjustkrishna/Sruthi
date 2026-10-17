import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Hide on its own after this long, unless the pointer is on the box. */
const AUTO_HIDE_MS = 20_000;

const card = document.getElementById("card")!;
const text = document.getElementById("text")!;
const copy = document.getElementById("copy") as HTMLButtonElement;

let current = "";
let hideTimer: number | undefined;
let hovered = false;

function hide() {
  window.clearTimeout(hideTimer);
  card.classList.remove("show");
  // Let the fade-out finish before the window goes away.
  window.setTimeout(() => invoke("hide_result"), 180);
}

function scheduleHide(ms: number) {
  window.clearTimeout(hideTimer);
  hideTimer = window.setTimeout(() => {
    if (hovered) scheduleHide(2000);
    else hide();
  }, ms);
}

listen<string>("result-show", ({ payload }) => {
  current = payload;
  text.textContent = payload;
  copy.textContent = "Copy";
  copy.classList.remove("done");
  card.classList.remove("show");
  void card.offsetWidth; // restart the entrance animation
  card.classList.add("show");
  scheduleHide(AUTO_HIDE_MS);
});

listen("result-hide", () => {
  window.clearTimeout(hideTimer);
  card.classList.remove("show");
});

copy.addEventListener("click", async () => {
  await invoke("copy_text", { text: current });
  copy.textContent = "Copied ✓";
  copy.classList.add("done");
  scheduleHide(1500);
});

document.getElementById("close")!.addEventListener("click", hide);
card.addEventListener("mouseenter", () => (hovered = true));
card.addEventListener("mouseleave", () => (hovered = false));
