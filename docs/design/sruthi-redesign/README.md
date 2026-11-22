# Sruthi interface preview

A high-fidelity, interactive design preview for the next Sruthi release: every main screen, a real resource dashboard, and richer operational detail while keeping the existing graphite-and-blue language.

**Open the preview:** [`index.html`](./index.html)  
**Open the screen board:** [`preview.html`](./preview.html) or [`preview.png`](./preview.png)

> This is a design artifact. The numbers are deterministic **sample data** for review, not readings from your PC. The production app must use measured values and honest unavailable states.

## What is here

| Screen | What it adds | Open |
| --- | --- | --- |
| **Performance** | Exact GPU / VRAM / CPU / RAM, a 60-second interactive chart, GPU engine split, heat guard, model runtime, recent runs, and data provenance | [`?screen=performance`](./index.html?screen=performance) |
| **Home** | A clearer daily banner, word count and rhythm, where you dictate, recent dictations, live system pulse, and the hold-to-dictate action | [`?screen=home`](./index.html?screen=home) |
| **Dictionary** | Word-bank health, hit rate, corrections caught, suggestions, and what gets fixed automatically | [`?screen=dictionary`](./index.html?screen=dictionary) |
| **Snippets** | Phrase list with usage, words saved, editor preview, and the "when I say" contract | [`?screen=snippets`](./index.html?screen=snippets) |
| **Models** | Active model, memory fit, measured speed/accuracy scatter, run history, downloaded models, and custom-model links | [`?screen=models`](./index.html?screen=models) |
| **Settings** | A system-health strip, searchable and filterable cards, microphone input meter, storage, send rules, pipeline status, privacy, and About | [`?screen=settings`](./index.html?screen=settings) |
| **Surfaces** | Preview-only first run, listening pill, result box, and share card | [`?screen=surfaces`](./index.html?screen=surfaces) |

`?clean=1` hides the preview chrome for clean screenshots. `?full=1` unlocks the page for full-length captures. `?theme=dark` opens in dark mode; the theme toggle is available in the preview chrome.

## The resource story

The Performance screen is the main addition. It answers four questions without hand-waving:

1. **What is the GPU doing right now?** Exact utilization, dedicated VRAM, and the 3D / compute / copy engine split.
2. **What is the rest of the machine doing?** CPU, system memory, Sruthi's share, other apps' share, and heat pressure.
3. **What did this run cost?** Peak and average GPU, peak VRAM, duration, and model, per dictation.
4. **Where did the number come from?** Windows counters, measured on this PC; macOS shows unavailable instead of inventing values.

The chart is a dark instrument stage inside the light app: blue GPU, teal VRAM, graphite CPU. Hover it to read the crosshair values. The 18.4-second dictation band and the peak/average figures are synchronized.

### Production mapping

| Display | Production source |
| --- | --- |
| GPU utilization and engine split | Windows PDH GPU Engine counters; aggregate across engines |
| GPU name and dedicated VRAM | Existing `src-tauri/src/gpu.rs` (Vulkan) |
| CPU load | Existing `thermal` sampling / `GetSystemTimes` |
| System and process memory | `GlobalMemoryStatusEx` plus the process working set |
| Heat pressure | Existing load and effective-clock guard; **no invented °C** |
| Model runtime and ETA | Existing `src-tauri/src/perf.rs` measured samples |
| Run history | Local SQLite/run records; never uploaded |

The chart must report a measured sample or an explicit `Unavailable` / `Sampling…` state. It must not fill gaps with fake data. Everything stays local; there is no telemetry.

## Visual direction

- Graphite shell, warm paper surfaces, one confident blue accent, teal for VRAM/secondary data.
- Dense but calm: 10.5–13 px labels, tabular numerals, clear section eyebrows, and generous card padding.
- The Performance chart is the only dark stage, so the resource story is the focal point.
- All screens share the same sidebar, page-head rhythm, chips, cards, buttons, and focus states.
- Light is the default; dark mode is included and tested on Performance.
- Reduced-motion preferences are respected.
- The **Surfaces** page is marked preview-only and is hidden from the clean navigation except when it is open.

## Screenshots

Top-fold captures at **1440 × 900**:

- [`screens/home.png`](./screens/home.png)
- [`screens/dictionary.png`](./screens/dictionary.png)
- [`screens/snippets.png`](./screens/snippets.png)
- [`screens/models.png`](./screens/models.png)
- [`screens/performance.png`](./screens/performance.png)
- [`screens/settings.png`](./screens/settings.png)
- [`screens/surfaces.png`](./screens/surfaces.png)
- [`screens/performance-dark.png`](./screens/performance-dark.png)

Full-length versions are in [`screens/`](./screens/) as `*-full.png`.

## Preserved behavior

This preview does **not** remove or regress:

- Heat warning and stop behavior.
- Latin ⇄ Native script toggle.
- IndicConformer support.
- Custom model links.
- Measured ETAs.
- The local-only privacy model.
- Never deleting downloaded speech models; only regenerable build cache may be cleared.

## Review checklist

1. Open Performance and hover across the 60-second chart.
2. Open Settings and search for `GPU`; try the category chips.
3. Open Models; inspect memory fit and the benchmark labels.
4. Open Home; check the dictation feed and the system pulse.
5. Open Surfaces; try the listening pill and share card.
6. Toggle dark mode in the preview chrome.
7. Confirm the sample-data labels are honest.

## Status and next step

This is still a **design artifact**. No production app files have been changed yet.

After the direction is approved:

1. Write the production spec and implementation plan.
2. Add the Windows resource sampler and Tauri commands.
3. Replace sample data with measured values and explicit unavailable states.
4. Port the approved screens into the existing vanilla TypeScript/Tauri UI.
5. Validate M1–M4 on the target machine.
6. Keep tag/version consistency, Windows/macOS installers, and `SHA256SUMS.txt` correct for the next release.
7. Track `src-tauri/DirectML.dll` and clear the cached target DLL before Windows builds.

## Static checks

- `node --check docs/design/sruthi-redesign/app.js`
- No duplicate HTML IDs.
- Balanced `<section>` / `<div>` tags.
- Chrome renders every route without console-breaking errors.
