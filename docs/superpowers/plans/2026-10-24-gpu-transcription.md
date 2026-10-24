# GPU Transcription Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Whisper models run on the user's graphics card by default with a visible device choice, a downloadable Vulkan runtime, a memory gate, and measured slow starts.

**Architecture:** One settings enum routes every whisper-model load onto a generalized persistent `whisper-server` (Vulkan) or the existing per-call `whisper-cli`; the runtime zip is fetched through the existing downloader; a memory gate pauses permissive loads until the user decides; press→mic timing rides the existing `timings.log`.

**Tech Stack:** Tauri 2, Rust, windows-sys (`GlobalMemoryStatusEx`), ash (Vulkan), existing `zip`/`reqwest` download plumbing, TS/HTML main window.

**Spec:** `docs/superpowers/specs/2026-10-24-gpu-transcription-design.md`

## Global Constraints

- Branch: `feature/gpu-transcription`; work happens inside `src-tauri`, the main window, and `docs/GPU.md`.
- No CPU-resident whisper server; CPU keeps per-call `whisper-cli` behavior exactly.
- Only whisper.cpp models may run on the card (turbo, tiny, apex, swift; hybrid via its turbo part). Parakeet/Moonshine untouched; Smart formatting (llama.cpp) untouched.
- Windows-only GPU path; `cfg(not(windows))` must keep compiling.
- Baseline test gate: `cargo test --release --lib` currently 97 passed / 0 failed; `cargo fmt --check` clean; `npm run build` green after UI tasks.
- Existing env escape `SRUTHI_WHISPER_CPU=1` keeps forcing CPU over everything.
- New env override only: `SRUTHI_GATE_MIN_FREE_MB` for testing the gate.

## Review Focus

1. **GPU server dies mid-session** (driver reset, enrollment of another app onto the card): the per-request restart path must respawn with the same model and language args, and the user must see a reason strip, not silence. Test: unit-test the server-kind arg mapping; failing serves in the manual task (`kill` the process during a dictation).
2. **Card can't fit the model** (tiny VRAMD): load must fall back to CPU with `mic_fallback`-style UI text naming the card. Test: manual task (unit hook where the fallback message is produced).
3. **Gate left unanswered**: user closes the window with the gate pending; a later hotkey press must open the main window (existing not-ready path) and the strip must still be there; no deadlock. Test: gate is a one-shot channel with a stored sender — covered by compile + manual check.
4. **Interrupted runtime download**:`.part` resume like models; staging dirs removed on failure. Test: manual; code mirrors `fetch_resumable` usage.
5. **Hybrid model switch while on card**: replacing the engine drops `WhisperServer` (kills child) — no orphan whisper-server.exe. Test: manual task (check Task Manager after switching).

---

### Task 1: Device setting in `settings.rs`

**Files:**
- Modify: `src-tauri/src/settings.rs`

**Interfaces:**
- Produces:
```rust
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Device { #[default] Auto, Gpu, Cpu }
// + pub device: Device on Settings, initialized in Default::default() as Device::Auto
```

- [ ] **Step 1: Write the failing test** (in settings.rs `mod tests`)
```rust
#[test]
fn device_defaults_to_auto_and_survives_a_roundtrip() {
    let settings: Settings = Default::default();
    assert_eq!(settings.device, Device::Auto);
    // Stored lowercase, like theme.
    assert_eq!(serde_json::to_string(&settings.device).unwrap(), "\"auto\"");
    settings.device = Device::Gpu;
    assert_eq!(serde_json::from_str::<Settings>(r#"{"device":"gpu"}"#).unwrap().device, Device::Gpu);
}
```
- [ ] **Step 2: Run** `cargo test -p sruthi --lib settings` → FAIL (no `Device`)
- [ ] **Step 3: Implement** — exact enum from Interfaces; add `pub device: Device` to `Settings` and `Device::default()` in `Default`.
- [ ] **Step 4: Run again** → PASS (old saved settings.json files without `device` keep loading because `#[serde(default)]` stays on the struct).
- [ ] **Step 5: Commit** `git commit -m "Settings gain the device choice (auto/gpu/cpu)"`

### Task 2: `gpu.rs` exposes the card name

**Files:**
- Modify: `src-tauri/src/gpu.rs`

**Interfaces:**
- Produces: `pub fn device_name() -> Option<String>` (human-readable card name, e.g. "AMD Radeon RX 6500M"), unchanged `pub fn dedicated() -> Option<usize>`.
- Consumes: existing `OnceLock` cache — refactor internals to `static INFO: OnceLock<Option<(usize, String)>>`; `dedicated()` becomes `.map(|(i,_)| i)` over `info()`.

- [ ] **Step 1: Refactor** to the cached pair; keep the existing log line exactly.
- [ ] **Step 2:** `cargo test -p sruthi --lib` → baseline green (no behavior change), `cargo fmt --check` clean.
- [ ] **Step 3: Commit** `git commit -m "gpu.rs exposes the dedicated card's name"`

### Task 3: `memory.rs` — free RAM + gate math

**Files:**
- Create: `src-tauri/src/memory.rs`; Modify: `src-tauri/src/lib.rs` (declare `mod memory;`)

**Interfaces:**
```rust
#[cfg(windows)] pub fn free_ram_mb() -> Option<u64>            // ullAvailPhys / 1_048_576; None on API failure
pub fn needed_mb(model_mb: u32, will_use_gpu: bool) -> u64     // gpu: model+1024 | cpu: model*3/2+1024 (u64)
pub fn is_tight(free: Option<u64>, model_mb: u32, will_use_gpu: bool) -> bool
// SRUTHI_GATE_MIN_FREE_MB set => tight iff free < that value (parsed via std::env::var)
```

- [ ] **Step 1: Write the failing tests**
```rust
#[cfg(windows)] #[test] fn free_ram_is_reported() { assert!(memory::free_ram_mb().is_some_and(|m| m > 0)); }
#[test] fn gpu_needs_less_headroom_than_cpu() { assert!(memory::needed_mb(834, true) < memory::needed_mb(834, false)); }
#[test] fn override_forces_tight() { assert!(memory::is_tight(Some(64_000), 834, true)); }  // no override, lots free
// override test uses std::env::set_var("SRUTHI_GATE_MIN_FREE_MB", "999999") then clears it
```
- [ ] **Step 2: Run** `cargo test -p sruthi --lib memory` → FAIL
- [ ] **Step 3: Implement** — `GlobalMemoryStatusEx` from `windows_sys::Win32::System::SystemInformation` (`#[cfg(windows)]`), helpers as above.
- [ ] **Step 4: Run** → PASS
- [ ] **Step 5: Commit** `git commit -m "Memory module: free RAM and the load-gate threshold"`

### Task 4: `whisper.rs` — runtime download + server for every kind + device-aware load

**Files:**
- Modify: `src-tauri/src/whisper.rs`

**Interfaces:**
```rust
pub const GPU_RUNTIME_URL: &str = "https://github.com/iamjustkrishna/Sruthi/releases/download/runtimes/whisper-vulkan-win.zip";
pub const GPU_RUNTIME_MB: u32 = 45;              // exact value set in Task 8 after measuring the built zip
pub fn runtime_vulkan_installed() -> bool
pub fn download_gpu_runtime(progress: &mut dyn FnMut(u64, u64)) -> Result<()>   // zip -> staging -> find_dir_with(whisper-server file) -> rename to gpu_runtime_dir(); skips when already installed
pub fn gpu_in_use() -> bool                      // AtomicBool; true iff the ACTIVE model's whisper load used WhisperServer
pub fn device_name_configured() // NOT whisper's job: callers use settings directly
// WhisperServer::start(kind: WhisperKind)   — model_path(kind) + language_args(kind); pruning of turbo-only assumption
// load(kind) unchanged signature; internally: device from settings::load().device decides
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn language_args_match_kinds() {
    assert_eq!(WhisperKind::HinglishApex.language_args(), &["-l", "en"]);
    assert!(WhisperKind::Tiny.language_args().is_empty());
} // guard while generalizing
#[test] fn finds_the_dir_containing_a_file() {
    // temp/t/a/x/, temp/t/b/f.txt -> find_dir_with(root, "f.txt") == Some(temp/t/b)
} // uses std::env::temp_dir + process id like the settings tests do
```
- [ ] **Step 2: Run** → FAIL
- [ ] **Step 3: Implement** the constants + fns; `load()` reads `settings::load().device`:
  - `Cpu` → straight to `WhisperCli` (existing behavior).
  - `Auto`/`Gpu`, runtime present, no `SRUTHI_WHISPER_CPU` → `WhisperServer::start(kind)`; on error the existing CPU fallback logs and continues; set `GPU_IN_USE` accordingly (stale-clear at the top of `load()`); on fallback, stash the reason via `LOAD_NOTE: Mutex<Option<String>>` with `pub fn take_load_note() -> Option<String>` (consumed exactly once per load by `lib.rs`).
  - `Auto` with no card/runtime → CPU silently (existing path).
- [ ] **Step 4: Run** → PASS; full `cargo test --release --lib` → 97+green (whisper paths compile for `cfg(not(windows))` too).
- [ ] **Step 5: Commit** `git commit -m "whisper: generalizable card server, runtime download, device-aware load"`

### Task 5: `model.rs` + `dictation.rs` — whisper marker and press timing

**Files:**
- Modify: `src-tauri/src/model.rs`, `src-tauri/src/dictation.rs`

**Interfaces:**
- Produces: `impl ModelInfo { pub fn uses_whisper(&self) -> bool }` (true for `Engine::Whisper(_)` and `Engine::Hybrid`).
- `dictation::Session::start(...)` gains `press: Instant, start_mode: StartMode`; `StartMode` is `{ Instant, Cold(String) }`; session stores `mic_ready: Duration` (press→recording-live measured immediately in `start`); `pub fn start_delay(&self) -> Duration`.

- [ ] **Step 1: Write the failing test**
```rust
// model.rs test: uses_whisper flags the right rows
assert!(model::find("hinglish-apex").unwrap().uses_whisper());
assert!(model::find("hybrid").unwrap().uses_whisper());
assert!(!model::find("parakeet-v3").unwrap().uses_whisper());
```
- [ ] **Step 2: Run** → FAIL → implement → PASS. Update the ONE caller `lib.rs on_hotkey` to pass `Instant::now()` (captured at entry, before mic acquisition) and the mode (`Instant` when `State<AppState>.mic` was Some, `Cold(reason)` when it fell back live).
- [ ] **Step 3: Commit** `git commit -m "ModelInfo::uses_whisper + Session records press timing and mic mode"`

### Task 6: `lib.rs` — gate, commands, status fields, timing log line

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
```rust
struct MemoryGateStatus { message: String, gpu_path: bool }   // Status.memory_gate: Option<_>
Status gains: device: settings::Device, gpu: GpuStatus { name: Option<String>, runtime_installed: bool },
  engine_on_gpu: bool, gpu_note: Option<String>,            // take_load_note() result, cleared next load
  memory_gate: Option<MemoryGateStatus>, mic_fallback: Option<String>, gpu_downloading: bool
enum GateChoice { Anyway, Later, PreferCpu }
// AppState.gate: Mutex<Option<std::sync::mpsc::Sender<GateChoice>>>
#[tauri::command] fn memory_decision(app, decision: String) -> Result<(), String>   // "anyway"|"later"|"cpu"
#[tauri::command] fn set_device(app, device: settings::Device) -> Result<(), String> // save + load_model(active) + emit
#[tauri::command] fn download_gpu_runtime(app) -> Result<(), String>                 // background thread, per-percent "download-progress" id "gpu-runtime", on success reload active whisper model
```

- [ ] **Step 1: `load_model` checks the gate before spawning** — cpu-or-gpu-proxy: `let will_use_gpu = settings::load().device != Cpu && whisper::runtime_vulkan_installed();` — when `memory::is_tight(...)` for `info.size_mb`: set inner `gate = Some(tx)`, state Loading with detail "Waiting because memory is tight", emit status, show main window only if the load was user-initiated (pass an `interactive: bool` from `select_model`/`download_model`; startup/boot path passes false), then `rx.recv()` and dispatch:
  - `Anyway` → continue the load on the current thread.
  - `Later` → state Missing, detail "Not loaded: memory is tight", clear gate, emit, return.
  - `PreferCpu` → save `device = Cpu`, clear gate, continue on this thread (whisper load now CPU).
  - `inner.memory_gate` (the text shown) cleared in all three paths.
- [ ] **Step 2: `status()` gains the new fields**; `engine_on_gpu = inner.active.uses_whisper() && whisper::gpu_in_use()`; `mic_fallback` set in `sync_mic`'s error branch (and on cold press when unset), cleared when the ready mic next succeeds.
- [ ] **Step 3: `transcribe_and_paste`** — after `timing::reset()`: `timing::add("start", session.start_delay());` and append `" · cold:<reason>"` via the log path (start mode carried in the session; format the stage only when `start_delay > 50 ms` to keep the log readable: `start 12 instant` / `start 640 cold:mic-failed`).
- [ ] **Step 4:** `cargo test --release --lib` → all green (baseline + tasks 1–5), `cargo fmt --check` clean.
- [ ] **Step 5: Commit** `git commit -m "Gate, device/runtime commands, status truth, press timing in timings.log"`

### Task 7: Main window — Runs-on row, gate strip, fallback strip

**Files:**
- Modify: `index.html` (inside the SPEECH MODEL card, after `#models`), `src/main.ts`, `src/styles.css`

**Interfaces:**
- `Status` additions mirrored in the TS interface: `device: "auto" | "gpu" | "cpu"`, `gpu: { name: string | null; runtime_installed: boolean }`, `engine_on_gpu: boolean`, `memory_gate: { message: string; gpu_path: boolean } | null`, `mic_fallback: string | null`, `gpu_downloading: boolean`.
- `renderDevice(s)` — segmented `Graphics card | CPU` (reuse `.view-switch` styling as `.device-switch`), note line (`s.gpu.name ?? "No graphics card"` + runtime state + `s.gpu_note` when present, e.g. "Card didn't start: … — on CPU for now"), disabled GPU button when `!s.gpu.name`; download pill `#gpu-runtime-download` with percent from the existing `download-progress` map (id `"gpu-runtime"`); model badge appends `" · on the card"` when `engine_on_gpu`.
- Warning strips above the model card: `#memory-warning` (with `#gate-anyway`, `#gate-later`, `#gate-cpu` shown when `memory_gate.gpu_path`) and `#mic-fallback-warning` (text only). Both use `.card.warning` styling.
- Commands wired: `run("set_device", {device})`, `run("memory_decision", {decision})`, `run("download_gpu_runtime")`.

- [ ] **Step 1:** Markup + `renderDevice` + strips + wiring; `render()` calls `renderDevice(s)`.
- [ ] **Step 2:** `npm run build` → green.
- [ ] **Step 3:** Browser-shell check (agent-browser at 520×640 like the stats task): tabs + strips render; GPU button toggles call path (invoke errors expected outside Tauri).
- [ ] **Step 4: Commit** `git commit -m "Main window: device choice, runtime download, gate and fallback strips"`

### Task 8: Build + host the portable Vulkan engine; end-to-end proof

**Files:**
- Modify: `docs/GPU.md` (portable build recipe becomes the canonical one; `GGML_NATIVE=OFF`), `whisper.rs` (pin exact `GPU_RUNTIME_MB`)

**Steps (human-assisted where marked):**

- [ ] **Step 1: Build** per `docs/GPU.md` with `GGML_NATIVE=OFF` on this machine (toolchain: VS, CMake, Ninja, glslc — all present). Target `whisper-server`, `whisper-cli`.
- [ ] **Step 2: Portable check** — copy server + DLLs to a fresh folder, run `whisper-server.exe -m <apex model> --host 127.0.0.1 --port 0` briefly; it must start (Vulkan loader is system-wide on Win10+; note the VC++ redist requirement in GPU.md).
- [ ] **Step 3: Zip + measure** — `whisper-server.exe`, `whisper.dll`, `ggml*.dll` → `whisper-vulkan-win.zip`; fix `GPU_RUNTIME_MB` to the measured size; `cargo test --release --lib` again after the constant change.
- [ ] **Step 4 (owner, ~3 min): Upload** `whisper-vulkan-win.zip` to a new `runtimes` release on `iamjustkrishna/Sruthi`.
- [ ] **Step 5: End-to-end** — delete local `runtime-vulkan`, in Sruthi click Download runtime (progress visible), apex ready with badge "· on the card"; dictate: 1.4 s speech lands ≤ ~2.5 s; 48 s ≤ ~8 s; `timings.log` shows `start … instant/cold` + `whisper-gpu` stages. Force the gate via `SRUTHI_GATE_MIN_FREE_MB=1e9` (unable-to-load popup, three choices incl. stuck switching to CPU), then unset.
- [ ] **Step 6: Manual edge pass** — kill whisper-server during a recording (fallback strip + restart); switch models while on card (no orphan process); second-opinion turbo inside Hybrid uses the card.
- [ ] **Step 7: Commit** `git commit -m "Portable Vulkan engine: docs, pinned runtime size, verified end-to-end"`

## Self-review notes (already checked)

- Spec → task map: setting (T1), card name (T2), memory module (T3), runtime+server+load (T4), markers+press timing (T5), gate/commands/status/timing line (T6), UI (T7), build/host/proof (T8). Hybrid GPU gain and CPU-unchanged behavior covered by T4; LLM/CUDA/macOS explicitly out (spec).
- Interfaces carried forward by exact names: `Device`, `memory::*`, `whisper::*`, `MemoryGateStatus`, `uses_whisper`, `start_delay`, `download_gpu_runtime`, `gpu-runtime` event id.
- Types consistent across tasks; `GPU_RUNTIME_MB` exact value deferred to T8 by design (measured artifact).
