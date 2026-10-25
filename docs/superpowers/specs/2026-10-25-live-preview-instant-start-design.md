# Live preview in the pill + instant press

2026-10-25

The user's direction: "Fix it go all in. Live transcription thing and key test recording
instant. Just go all in fix it." This spec is the record of that scope.

## Goal

1. Words appear in the pill **while the user is still speaking**, not only after release.
2. Pressing the key starts recording **immediately**: no UI Automation, settings or overlay
   work stands between the press and the microphone; the pill is shown before it is placed.
3. The final text lands as soon as possible after release: when the last preview is fresh,
   reuse it instead of transcribing the tail again.

## Non-goals

- Engine-side streaming ASR (word-by-word). Previews are periodic snapshots of the current
  unfinished piece, produced by the same engine.
- `audio_ctx` trimming. Measured: a 14 s clip goes 1.6 s → 0.64 s at `audio_ctx=768`, but
  specific values (e.g. 896) send this model into a comma loop, and the fixed cost is the
  padded 30 s encoder window. Making that safe needs a validated context map plus a loop
  guard; its own task.

## Design

### Previews (`dictation.rs`)

The `Early` thread already transcribes completed pieces while the key is held. It gains a
preview path:

- When no piece is finished (`chunks::next_cut` is empty) and at least **0.6 s** of
  untranscribed audio exists since `max(done, preview_end)`, transcribe the growing piece
  `[done, len]` (windowed to its last **6 s**) and report `texts.join(" ") + preview`.
- One request in flight at a time (the thread is the serializer); completed pieces always
  take priority over previews.
- Previews drive a new `dictation` phase, `partial`, rendered on the pill's existing live
  line (which already truncates to the newest ~54 chars).
- **Release reuse**: a preview remembers the sample index it covered. At release, if
  `len - preview_end <= 0.3 s`, the preview text *is* the tail: it is used instead of
  re-transcribing, so a release right after a preview pastes in ~250 ms.
- A stale preview cannot show up after release: the pill ignores `partial` outside the
  `recording` phase.

### Instant press (`lib.rs`)

- `context::active_app()` (UI Automation) moves off the press path into a background
  thread. The session holds `Arc<Mutex<AppKind>>`; formatting reads it at finish time.
- The press path becomes: inner lock → model check → settings → `mic.record()` → session →
  emit `Recording` → show pill. `place_overlay` (caret lookup, the slow UI Automation part)
  runs **after** the pill is up.
- Press stages (lock, settings, mic, session) are measured and appended to the `start`
  timings line when the press took more than 50 ms, so the next 2 s outlier is diagnosable.

## Errors and edges

- A preview failure is logged and ignored; the final pass still transcribes the tail.
- A preview that contains no speech (`enhance::prepare` returns None) emits nothing and
  moves the watermark, so a silent stretch does not spin requests.
- A preview older than 0.3 s at release is discarded; the normal piece path runs.
- Very long stretches without a pause: previews cover the last 6 s; the final transcript
  still covers everything not reused.
- Model not ready: unchanged (the press opens the window).

## Testing

- Unit tests for the pure decisions: `preview_due(...)` and `preview_reusable(...)`.
- Unit test for the start-detail label (mode + stage notes).
- Gates: `cargo test --release -p sruthi --lib`, `cargo fmt --check`, `npm run build`.
- Manual E2E in the built app: hold-to-talk shows words within ~2 s; release right after a
  preview lands in ~0.3-0.5 s; press → pill visible in well under 150 ms; the 106-test
  suite stays green and dictation text is unchanged.
