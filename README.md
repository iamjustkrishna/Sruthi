# Srutam Dictate

Open-source, fully offline voice dictation for Windows and macOS, in the spirit of Wispr Flow.
Hold a hotkey, speak, release: the text is typed into whatever app has your cursor.
Your audio never leaves the device.

## How it works

```
Hold Ctrl+Shift+Space → mic capture (cpal) → 16 kHz mono
Release               → Parakeet TDT 0.6B v3 int8 (ONNX Runtime, on-device)
                      → paste into the focused app (clipboard + Ctrl/Cmd+V, clipboard restored)
```

- **Speech-to-text:** [transcribe-rs](https://github.com/cjpais/transcribe-rs) running NVIDIA Parakeet on CPU.
- **App shell:** Tauri 2 (Rust core + TypeScript UI). It closes to the system tray and shows a floating pill while you speak.
- **Model:** downloaded once (≈480 MB) into `%LOCALAPPDATA%\SrutamDictate\models` on Windows (the app data folder on macOS).

## Develop

Prerequisites: Rust (stable, MSVC on Windows), Node 20+, and the Visual Studio C++ build tools on Windows.

```bash
npm install
npm run tauri dev
```

Release build:

```bash
npm run tauri build
```

See [PLAN.md](PLAN.md) for the roadmap.
