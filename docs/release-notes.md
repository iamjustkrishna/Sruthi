**Sruthi 0.3.0 — on the graphics card, and instant from the first press.**

- **Whisper on the GPU.** The Whisper models now run on the Vulkan card. The engine is a ~10 MB one-time download, is size-checked before use, and falls back to the CPU with a visible note when the card or runtime is not available.
- **It says where it runs.** The window shows *Runs on* — the card by name, or the CPU — and the pill carries the model badge.
- **A gate that stays out of the way.** The card path asks only when free RAM is under 400 MB; the CPU path asks at model + 512 MB. The gate can also offer the card when the CPU path is the tight one.
- **Instant start.** The microphone stays warm, the pill is up before the caret lookup, and `timings.log` records what the press spent (`start ... lock / settings / mic`).
- **Live work without live text.** While the key is held, the tail is transcribed in the background and the freshest result is reused at release — so the paste is near-instant — but the pill never shows words while you speak.
- **Steady pill wave.** Levels start with the pill, settle when audio stalls, and WebView2 no longer throttles the unfocused overlay.

Measured on the reference machine: a 1.4 s dictation lands in ~1.7 s end to end on the card, and a 48 s dictation is transcribed in pieces while the key is held.

**First run on the card** downloads the Vulkan engine once. Windows needs the VC++ runtime; see `docs/GPU.md`. The macOS build is unsigned: right-click the app and choose Open on first launch.
