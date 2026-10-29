# Contributing to Sruthi

Thanks for helping make offline dictation better. This is a small project, so the
process is deliberately light.

## Project layout

```
index.html            main window markup (rail + four pages)
overlay.html          floating pill markup
result.html           floating result box markup
src/                  TypeScript + CSS for the three windows above
src/theme.css         the design tokens (palette, type scale, spacing, radii)
src-tauri/src/        the Rust core
  lib.rs              Tauri commands and app wiring
  model.rs            the speech-model catalog and downloads
  whisper.rs          the Whisper runtime family
  chunks.rs           listened-chunk corrections (no, nope, actually…)
  format/             number formatting and the LLM/cloud cleanup engines
  context.rs          what app is focused, for stats
  stats.rs            usage statistics and backfill
  settings.rs         settings.json and the app data folder
  secrets.rs          API keys in the OS credential store
  capture.rs          microphone selection and probing
  bench/              the real-audio benchmark runner and results
.github/workflows/    CI and the release/packaging workflow
```

## Development setup

Prerequisites: Rust (stable, MSVC on Windows), Node 20+, the Visual Studio C++
build tools on Windows (or Xcode command-line tools on macOS).

```bash
npm install
npm run tauri dev          # run the app against a dev server
npm run build              # type-check and build the frontend only
cd src-tauri
cargo test --release --lib # the test suite
```

The first `tauri dev` run downloads a speech model (~478 MB) into
`%LOCALAPPDATA%\Sruthi\models` — that is expected and one-time.

## How the code is organized (and what goes where)

- **Anything with logic or edge cases belongs in Rust, with tests.** The Rust
  crate carries the project's tests; a bug fix or a new formatting rule should
  come with a `cargo test` case that fails without it.
- **TypeScript is thin glue** for moving commands and events between HTML and
  Rust; avoid growing business logic there.
- **Styling uses the token system.** Reference the roles in `src/theme.css`
  (surfaces, ink, spacing steps, type roles, radius ladder) rather than raw
  values, so light, dark and system themes stay in sync.

## Commit and pull-request conventions

- One-line, sentence-style commit messages: first word capitalized, the rest
  lowercase, e.g. `Keep the clipboard restore honest when paste fails`.
- Keep pull requests focused; one behavior or fix per PR.
- CI runs the frontend build and the Rust test suite on Windows and macOS;
  both need to pass before merge.

## Releasing

The maintainer tags a version and the release workflow builds the installers:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

The workflow (`.github/workflows/release.yml`) builds Windows and macOS
packages with [tauri-action](https://github.com/tauri-apps/tauri-action) and
opens a **draft release** with the artifacts attached. Review and publish it
by hand — macOS binaries are unsigned, so the release notes tell people to
right-click → Open on first launch.

## Reporting issues

Use the bug report or feature request templates. Include your OS, app version
and the active speech model — most dictation complaints are model choice, and
some are microphones, so the details usually solve it quickly.

## License and contributor agreement

Sruthi is released under the [GNU GPL v3 or later](LICENSE). By opening a pull
request you agree that:

1. **You wrote it, or have the right to submit it.** It is your own work, or
   it comes from something whose license allows this.
2. **It is licensed under the GPL.** Your contribution is released under
   GPL-3.0-or-later with the rest of the project.
3. **The maintainer may also relicense it.** You grant the project owner a
   perpetual, worldwide, non-exclusive, irrevocable license to use, change and
   distribute your contribution under other terms as well, including commercial
   licenses. You keep the copyright in your contribution.

Put "I agree to the contributor agreement in CONTRIBUTING.md" in your pull
request description (the template has a checkbox). If you can't agree to point
3, please open an issue to talk before sending code.
