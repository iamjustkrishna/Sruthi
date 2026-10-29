// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

// The "Share your stats" sheet: pick a card, choose what is on it, then save
// the picture or copy it. The picture is drawn here (see share.ts) and handed
// to the app only to be written to Pictures or put on the clipboard.

import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  CARD_NAMES,
  OPTIONS_FOR,
  type CardKind,
  type ShareData,
  type ShareOptions,
  caption,
  drawCard,
  unavailable,
} from "./share";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const KINDS: CardKind[] = ["time", "week", "month", "alltime", "speed", "milestone"];
const IDLE = "Drawn on this PC. Nothing is uploaded, and no dictated text is on the card.";

const dialog = $<HTMLDialogElement>("share-dialog");
const stage = $("share-stage");
const status = $("share-status");
const optStreak = $<HTMLInputElement>("share-opt-streak");
const optSpeed = $<HTMLInputElement>("share-opt-speed");
const optApps = $<HTMLInputElement>("share-opt-apps");

let data: ShareData;
let kind: CardKind = "week";
let current: HTMLCanvasElement | null = null;
let drawing = 0;

/** Only the switches that apply to this card count; the rest are hidden. */
const options = (): ShareOptions => {
  const on = OPTIONS_FOR[kind];
  return {
    streak: on.streak && optStreak.checked,
    speed: on.speed && optSpeed.checked,
    apps: on.apps && optApps.checked,
  };
};

function syncOptions() {
  const on = OPTIONS_FOR[kind];
  for (const row of document.querySelectorAll<HTMLElement>(".share-check[data-opt]")) {
    row.hidden = !on[row.dataset.opt as keyof ShareOptions];
  }
  $("share-opts-none").hidden = Object.values(on).some(Boolean);
}

function say(message: string, bad = false) {
  status.textContent = message;
  status.classList.toggle("bad", bad);
}

function buildKinds() {
  $("share-kinds").replaceChildren(
    ...KINDS.map((k) => {
      const why = unavailable(k, data);
      const b = document.createElement("button");
      b.type = "button";
      b.className = "seg-btn";
      b.textContent = CARD_NAMES[k];
      b.disabled = why !== null;
      b.title = why ?? "";
      b.setAttribute("aria-pressed", String(k === kind));
      b.addEventListener("click", () => {
        kind = k;
        buildKinds();
        syncOptions();
        void redraw();
      });
      return b;
    }),
  );
}

async function redraw() {
  const mine = ++drawing;
  say(IDLE);
  const opts = options();
  try {
    const canvas = await drawCard(kind, data, opts);
    if (mine !== drawing) return;
    current = canvas;
    canvas.setAttribute("role", "img");
    canvas.setAttribute("aria-label", `${CARD_NAMES[kind]} card preview`);
    stage.replaceChildren(canvas);
    $("share-caption").textContent = caption(kind, data, opts);
  } catch (e) {
    if (mine !== drawing) return;
    current = null;
    const p = document.createElement("p");
    p.className = "muted small";
    p.textContent = e instanceof Error ? e.message : String(e);
    stage.replaceChildren(p);
    $("share-caption").textContent = "";
  }
  const have = current !== null;
  for (const b of document.querySelectorAll<HTMLButtonElement>(".post-btn")) b.disabled = !have;
  $<HTMLButtonElement>("share-save").disabled = !have;
  $<HTMLButtonElement>("share-copy").disabled = !have;
  $<HTMLButtonElement>("share-copy-caption").disabled = !have;
}

function toPng(canvas: HTMLCanvasElement) {
  return new Promise<Uint8Array>((resolve, reject) => {
    canvas.toBlob(async (blob) => {
      if (!blob) return reject(new Error("The picture could not be made"));
      resolve(new Uint8Array(await blob.arrayBuffer()));
    }, "image/png");
  });
}

async function guard(button: string, work: () => Promise<void>) {
  const b = $<HTMLButtonElement>(button);
  b.disabled = true;
  try {
    await work();
  } catch (e) {
    say(String(e instanceof Error ? e.message : e), true);
  } finally {
    b.disabled = current === null;
  }
}

$("share-save").addEventListener("click", () =>
  guard("share-save", async () => {
    if (!current) return;
    const png = await toPng(current);
    const path = await invoke<string>("save_share_image", png, { headers: { "x-name": `sruthi-${kind}` } });
    say(`Saved to ${path}`);
  }),
);

/** Puts the picture on the clipboard so it can be pasted into a post. */
async function copyImage() {
  if (!current) return;
  const { width, height } = current;
  const pixels = current.getContext("2d")!.getImageData(0, 0, width, height).data;
  await invoke("copy_share_image", new Uint8Array(pixels.buffer), {
    headers: { "x-width": String(width), "x-height": String(height) },
  });
}

$("share-copy").addEventListener("click", () =>
  guard("share-copy", async () => {
    await copyImage();
    say("Copied. Paste it into your post.");
  }),
);

// One click to a post: the picture goes on the clipboard, the site opens with
// the caption filled in, and the person pastes the picture and presses Post.
const POST_TO: { name: string; url: (text: string) => string }[] = [
  { name: "X", url: (t) => `https://x.com/intent/post?text=${encodeURIComponent(t)}` },
  { name: "LinkedIn", url: (t) => `https://www.linkedin.com/feed/?shareActive=true&text=${encodeURIComponent(t)}` },
  { name: "WhatsApp", url: (t) => `https://wa.me/?text=${encodeURIComponent(t)}` },
];

$("share-post").replaceChildren(
  ...POST_TO.map((target) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "ghost post-btn";
    b.textContent = target.name;
    b.addEventListener("click", async () => {
      b.disabled = true;
      try {
        await copyImage();
        await openUrl(target.url($("share-caption").textContent ?? ""));
        say(`Picture copied. Paste it into your ${target.name} post.`);
      } catch (e) {
        say(String(e instanceof Error ? e.message : e), true);
      } finally {
        b.disabled = current === null;
      }
    });
    return b;
  }),
);

$("share-copy-caption").addEventListener("click", () =>
  guard("share-copy-caption", async () => {
    await invoke("copy_text", { text: $("share-caption").textContent ?? "" });
    say("Caption copied.");
  }),
);

for (const box of [optStreak, optSpeed, optApps]) box.addEventListener("change", () => void redraw());
$("share-close").addEventListener("click", () => dialog.close());

export function openShare(d: ShareData) {
  data = d;
  if (unavailable(kind, d)) kind = KINDS.find((k) => !unavailable(k, d)) ?? "time";
  buildKinds();
  syncOptions();
  dialog.showModal();
  void redraw();
}
