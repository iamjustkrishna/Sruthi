# GPU transcription on the user's card

Date: 2026-10-24 · Status: awaiting review · Branch: `feature/gpu-transcription`

## Problem

Whisper models (hinglish-apex in particular) transcribe on the CPU by spawning
`whisper-cli.exe` fresh for every piece and loading the 834 MB model from disk
each time — a fixed 2–4 s per piece. A 1.4 s dictation takes ~4.4 s; 48 s of
speech takes ~32 s. A persistent CPU-resident server was considered and
**rejected by product decision**: CPU RAM is contested by other applications.
The card is the right residence.

Separately, "pressing the hotkey sometimes takes a moment to start" has never
been measured: the always-ready mic silently falls back to a cold microphone
open per press, and the only trace is a log line nobody reads.

## Goal

Whisper runs on the user's graphics card (NVIDIA, AMD, Intel — one Vulkan
engine covers all three) by default; the main window shows where the model
runs, lets the user choose card vs CPU, download the card runtime, and warns
before loading a heavy model when the PC's memory is already tight. Every
dictation logs how long key-press → mic-live took, and instant-mic failure is
surfaced in the window instead of being silent.

## Decisions already made with the owner

- Device preference lives in settings; default is use-the-card (auto means
  "card, fall back to CPU").
- No CPU-resident server. CPU keeps today's per-call `whisper-cli` behavior.
- The Vulkan runtime zip is built locally per `docs/GPU.md` (with
  `GGML_NATIVE=OFF` so it is portable) and hosted once by the owner as an
  asset on a permanent `runtimes` release of `iamjustkrishna/Sruthi`; the app
  downloads it like every other asset it fetches today.
- No CUDA build (a second engine) for now; no macOS GPU path; Smart formatting
  (llama.cpp) stays on CPU — future work.

## Components

### 1. `settings.rs` — the device choice

New field `pub device: Device` on `Settings`; `Device` is a small serde enum
`auto | gpu | cpu` (`#[default] auto`, `rename_all = "lowercase"`). Stored in
`settings.json` exactly like `theme` / `overlay`. Round-trip test added.

### 2. `gpu.rs` — tell the user which card

`dedicated()` becomes `info() -> Option<(usize, String)>` (index + card name),
still cached in a `OnceLock`. Callers get both; the log line stays. No new
dependencies; vendor-agnostic Vulkan query as today.

### 3. `whisper.rs` — one server for every kind, runtime download

- Constants next to the existing ones: `GPU_RUNTIME_URL` (the `runtimes`
  release asset), `GPU_RUNTIME_MB` (measured at build time).
- `runtime_vulkan_installed()` — `gpu_server_path().is_file()`.
- `download_gpu_runtime(progress)` — fetch to `root()/gpu-runtime.zip.part`
  via the existing `fetch_resumable`, extract to staging, `find_dir_with` the
  server exe, rename into `runtime-vulkan`. Same shape as `install_runtime`.
- `WhisperServer::start(kind)` — parameterize today's turbo-only start with
  `model_path(kind)` and `language_args(kind)`. Full 30 s window, as today and
  as Whisper was trained. Existing restart-on-stuck and Drop kill behavior
  unchanged.
- `load(kind, device)` rules:
  - `device == Cpu` → always the CPU `WhisperCli` path (today's behavior).
  - otherwise, if the Vulkan runtime is installed (and
    `SRUTHI_WHISPER_CPU` is not set) → try the GPU server;
    on start failure → CPU fallback with a warning log (existing pattern),
    and the failure is exposed to the UI (see 5).
  - Applies to turbo, apex, swift, tiny — including the hybrid second-opinion
    turbo, which gains the card automatically.
- Timing stage stays `whisper-gpu`; the info log gains the kind label.

### 4. `memory.rs` (new, small) — know when the PC is squeezed

- `free_ram_mb() -> Option<u64>` via `GlobalMemoryStatusEx`
  (`windows-sys` already carries the `Win32_System_SystemInformation`
  feature — no new dependency). `None` on non-Windows → gate skipped.
- Threshold helper `is_tight(free_mb, model_mb, will_use_gpu)`: the load waits
  when (GPU path) `free < model_mb + 1 GB` — the model lands in VRAM, but
  staging needs headroom — or (CPU path) `free < model_mb × 1.5 + 1 GB` —
  the model must live in system RAM. A `SRUTHI_GATE_MIN_FREE_MB` dev override
  forces the popup in testing.
- GPU VRAM is *not* pre-measured (fragile); a card that can't take the model
  fails to start and the load falls back to CPU with a visible explanation.

### 5. `lib.rs` — one load path with a gate; instrumentation

- `Status` gains: `device`, `gpu { name, runtime_installed }`,
  `engine_on_gpu` (active model is served by `WhisperServer`),
  `memory_issue: Option<String>` (the popup text), `mic_fallback`
  (why recording started cold, if any).
- `load_model` before loading: if the memory gate says tight → set
  `model_state = Loading` with detail "Waiting — memory is tight", emit, and
  block on a one-shot channel in `AppState`. Choice command
  `memory_decision(app, "anyway" | "later")`: *anyway* resumes the load,
  *later* leaves the model unloaded with an explanatory detail. The strip and
  window behavior: interactive loads (Download / Use / device change in the
  window) show the window; a silent background startup load just shows the
  strip once the window opens.
- Commands: `set_device(device)` (save → reload active model → emit) and
  `download_gpu_runtime` (one at a time, progress events reused with id
  `"gpu-runtime"`, error path mirrors `download_model`).
- Instant-mic truth: `sync_mic` failure stores the reason in
  `mic_fallback`; `on_hotkey` marks a press "cold" when the ready mic is
  absent despite instant start. Success clears it.
- Slow-start numbers: `dictation::Session` records press time and mic mode at
  creation; `transcribe_and_paste` adds a `start {ms}` stage into the existing
  `timings.log` line, tagged `instant` / `cold:{reason}`.

### 6. UI (`index.html`, `src/main.ts`, `src/styles.css`)

- In the SPEECH MODEL card, under the model list: a **Runs on** row —
  segmented `Graphics card | CPU`, the card's name under it
  ("AMD Radeon RX 6500M"), and either "Ready" or a "Download runtime
  (~N MB)" pill with live percent. Model badge of the active model shows
  "· on the card" when `engine_on_gpu`.
- Memory-gate popup = the existing warning-strip pattern above the model
  card, with three actions: **Load anyway**, **Stay unloaded**, **Switch to
  CPU** (shown when the pending load was headed for the card). Switching to
  CPU saves `device: cpu` and resumes the same load on the CPU path, so the
  answer STICKS for future loads too, not just this attempt.
  `mic_fallback` reuses the same strip style.
- `main.ts` wires the progress id `gpu-runtime` through the existing
  progress map; no new event types.

## Data flow (happy path)

1. Startup: `load_model(active)` → memory OK → device auto → runtime
   installed → `WhisperServer::start(apex)` → model resident on the card;
   badge "hinglish-apex · on the card".
2. Hold key → mic records instantly; release → piece streamed to the card →
   ~1.5 s later text pastes. `timings.log`: `start 12 instant · whisper-gpu
   1500 · ai …`.
3. Tight PC: strip asks; user says "Load anyway" (or CPU/never) — the model
   loads accordingly.

## Failure behavior

- Vulkan runtime missing (or download interrupted): CPU continues this
  session; the Runs-on row offers the download again.
- Card start fails (driver/OOM): CPU fallback + strip explains; every attempt
  is logged. Dictation never breaks.
- Stuck GPU run: existing server restart logic, now per kind.
- Gate left unanswered: dictation hotkey opens the main window (existing
  not-ready path) where the strip waits; nothing loads until a choice, and
  the choice is remembered only for that attempt.

## Testing

- Rust: `Device` round-trip; threshold math; `free_ram_mb() > 0` on the
  Windows CI runner; language-args-per-kind mapping; `find_dir_with` edge.
  Baseline: current 97 passed / 0 failed stays green; `cargo fmt` clean.
- Frontend: `npm run build` (tsc); visual pass on the Runs-on row and the
  three strips (browser shell test like the stats view).
- Manual on this machine: rebuild the engine per `docs/GPU.md`
  (`GGML_NATIVE=OFF`), delete local `runtime-vulkan`, download via the new
  button, gauge apex end-to-end; check `timings.log` gains `start` and
  `whisper-gpu` lines; force the gate popup with `SRUTHI_GATE_MIN_FREE_MB`.

## Success criteria

- Apex, 1.4 s of speech: text lands in ≤ ~2.5 s (was ~4.4 s).
- 48 s dictation: ≤ ~8 s end-to-end (was ~32 s).
- The window names the card, the setting, and the reason for every fallback;
  the mystery of slow starts is measurable instead of folklore.

## Build & host checklist (pre-implementation prerequisite)

1. Build portable engine per `docs/GPU.md`, verify it runs from a fresh
   folder (no toolchain assumptions).
2. Zip server + DLLs, measure size.
3. Owner uploads to the `runtimes` release; URL + size pinned in
   `whisper.rs`.
4. End-to-end download test through the app.
