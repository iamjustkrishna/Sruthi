# Live preview in the pill + instant press — implementation plan

Spec: `docs/superpowers/specs/2026-10-25-live-preview-instant-start-design.md`

## Task 1 — Backend previews (`src-tauri/src/dictation.rs`)

1. Add pure decisions with tests first:
   - `preview_due(done: usize, len: usize, preview_end: usize, rate: u32) -> bool`:
     true when `len - max(done, preview_end) >= 0.6 s * rate`.
   - `preview_reusable(len: usize, preview_end: usize, rate: u32) -> bool`:
     true when `len - preview_end <= 0.3 s * rate`.
2. `Early` gains `on_partial: Arc<dyn Fn(String) + Send + Sync>`, `preview: Option<Preview>`
   (`{ end: usize, text: String }`), and a preview branch in its loop:
   - after the completed-piece attempt fails to find a cut, and `preview_due(...)`,
     transcribe `slice_16k(done, len)` windowed to the last 6 s, then emit
     `join(texts + preview)` and record `preview = { end: len, text }`.
   - `transcribe_piece` returning empty (no speech) still advances `preview.end` and emits
     nothing.
   - commit of a completed piece clears `preview`.
3. `Early::finish()` returns `Progress` plus the last preview; `Session::finish()`:
   - if `preview_reusable(len, preview.end, rate)`, push the preview text and advance
     `done` to `preview.end` before transcribing the remainder.
4. Keep timings honest: preview work adds to a new `live` stage (sum), committed pieces
   keep `early`.
5. `Session::start` takes the partial callback; tests use a no-op callback.

## Task 2 — Events + press path (`src-tauri/src/lib.rs`)

1. `Dictation::Partial { text: String }` (serde phase `partial`).
2. `on_hotkey` pressed:
   - build `on_partial` that emits `Dictation::Partial` to the app.
   - replace `context::active_app()` on the press path: create `Arc<Mutex<AppKind>>`
     (default `Other`), pass it to `Session`, and spawn a thread that fills it and
     `inner.app_target` (logging the target as today).
   - measure press stages (lock, settings, mic, session) and store them on the session
     as a note string.
   - emit `Dictation::Recording` + `show_overlay` immediately after the session is
     created; `place_overlay` and `stream_levels` follow in the same background thread.
3. `transcribe_and_paste`: `start_detail()` returns `mode + " " + stages` for the `start`
   timing row.

## Task 3 — Overlay (`src/overlay.ts`)

- Add `{ phase: "partial"; text: string }` to the type and a `case "partial": setLive(text)`
  that keeps the pace line running and does not enter/leave.
- `recording` clears the live line (already) — previews rebuild it.

## Task 4 — Verify

1. `cargo test --release -p sruthi --lib` (env: `CARGO_TARGET_DIR=F:\sruthi-target`,
   `TMP/TEMP=F:\tmp`) — 106 tests + 2-4 new.
2. `cargo fmt --check`, `npm run build`.
3. Build `npx tauri build --no-bundle`, restart the app.
4. E2E: hold-to-talk, watch the pill fill; release right after a preview (fast paste) and
   after speaking into a preview (≤ one request); check `timings.log` for `live`, `start`
   with stage notes; dictations still paste correctly.
5. Update the ledger; commit per task (gates first, commit separately).
