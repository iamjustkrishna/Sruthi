**Sruthi 0.9.0: see what your PC is doing.**

- **Performance screen.** A live, one-second view of the graphics card, dedicated video memory, processor and system memory. GPU activity and its engine split come from the same Windows counters Task Manager uses; video memory comes from DXGI; CPU and RAM come from Windows system calls. Sruthi's own share is measured separately so you can see what it costs.
- **A chart that means something.** The last 60 seconds are drawn from real samples held in memory. There is no sample data, no simulated load and no uploaded telemetry. If a counter is missing, the card shows an em dash and says why instead of showing a made-up zero.
- **Heat and runtime, side by side.** The Performance screen shows the heat guard's load/clock pressure, effective clock and cooldown, plus the active model's measured speed, wait per audio second and successful-run count. Recent dictations show their own peak GPU, VRAM and RAM.
- **Glanceable anywhere.** Home now has a compact System pulse with the same live numbers, and Settings has a System health card. Settings also gained free-text search and category filters, so "hotkey", "mic" or "heat" find the right card immediately.
- **Honest platforms.** Windows gets the full counter view. macOS builds show explicit "unavailable" states rather than borrowing Windows values. Windows still does not expose a normal-app temperature sensor, so the heat display keeps using load and effective clock — never an invented °C.

Everything from 0.8.0 is included: Indic Conformer, the Latin/Native script toggle, paste-a-link models and the runtime model registry. From 0.7.0: the model runtime registry and safe link removal. From 0.6.0: native Indic scripts. From 0.5.0: the heat guard and measured wait estimates.

Downloads are published for Windows and Apple Silicon macOS, with `SHA256SUMS.txt` beside the installers.
