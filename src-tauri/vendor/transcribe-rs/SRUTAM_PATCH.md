# Srutam patch to transcribe-rs 0.3.12

Vendored from crates.io (MIT, https://github.com/cjpais/transcribe-rs).
Changes are marked `SRUTAM PATCH` in `src/onnx/parakeet/mod.rs`:

1. **Duration-aware TDT decoding.** Parakeet TDT models output token logits
   followed by duration logits (how many encoder frames to skip, 0..=4).
   Upstream ignored the durations and stayed on a frame until it predicted
   blank, so repeated characters in spoken codes were emitted many times:
   "T four four A A B C" became `T44AAAAAAAAAAABC`. The loop now advances by
   the predicted duration, like NeMo / onnx-asr greedy TDT decoding.
2. **Near-tie durations take the smaller forward skip** (within 1.0
   logits). With the int8 model a too-big skip could jump over a repeated
   character ("eleventh" came out as `1th`). Duration 0 ("stay on this
   frame") is only used when it clearly wins, since choosing it on a tie
   lets the decoder loop on quiet audio.

3. **Hotword boosting** (`src/onnx/parakeet/hotwords.rs`, plus a default
   no-op `SpeechModel::set_hotwords` in `src/lib.rs`). Dictionary words
   are split into vocabulary pieces in a prefix tree; while decoding,
   pieces that start (+1.0) or continue (+2.5) a word get a logit bonus. A
   new word may not override a confident blank. Doubling the bonuses fixed
   one more word but distorted others ("Srutam" -> "Sruutam"), so the
   gentle values stay; the app's sound-alike correction does most of the
   work.

Set `SRUTAM_TDT_DEBUG=1` to print every emitted token with its logits.

Also: `ort` is pinned to `=2.0.0-rc.12`, since rc.14 changed the session
API and breaks this version.

Measured on synthetic speech (two voices): spoken codes correct went from
8/16 to 14/16, with no change on general sentences. The remaining misses
are recognition errors ("Z" heard as "C").

Examples, tests and sample audio were left out of the vendored copy. Drop
this folder once these fixes are available upstream.
