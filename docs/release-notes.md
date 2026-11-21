**Sruthi 0.5.0: a heat guard for hot laptops.**

- **The heat guard.** While a model runs, Sruthi now watches CPU load and the effective clock. If the PC stays pinned or starts throttling, it stops the run, drops the local AI server, and pauses heavy models for a cooldown. A pill explains what happened; a strip in the window stays until the machine recovers. `thermal.log` keeps the evidence. Windows does not give normal apps a temperature sensor, so this uses load, clock and run duration instead. The guard is on by default and can be switched off in Settings.
- **Heavy jobs have a hard stop.** A heavy model is stopped after 90 seconds even if the clock looks healthy. That is a safety limit, not a speed target.
- **Know the cost before you choose.** Every model row now shows a heat class (cool, warms up, runs hot) and an estimate of how many seconds a minute of speech could take. The estimate learns from this PC's own successful runs; heavy models ask for confirmation first.
- **A real wait time while it works.** The pill shows an estimated countdown after you let go, and the estimate gets better with use.
- **Cooler by default.** The guard caps Whisper at four CPU threads, puts model processes below normal priority, and skips live previews for heavy models so the keyboard does not heat the machine twice.

Everything from 0.4.5 is included: the splash on every launch, no flash of Home, and the finer wordmark. From 0.4.3: first-run setup with a PC check and model recommendation, models limited to what your system can run, and the My month and All time share cards.

Windows is the primary platform. The macOS build is Apple Silicon only and unsigned (right-click the app and choose Open on first launch), and the app-aware features need Windows. The Windows installer is not code-signed, so Windows may show "Windows protected your PC"; choose More info, then Run anyway.
