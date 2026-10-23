# Srutam Dictate

Open-source, fully offline voice dictation for Windows and macOS, in the spirit of Wispr Flow.
Hold a hotkey, speak, release: the text is typed into whatever app has your cursor.
Your audio never leaves the device.

```
Hold Ctrl+Shift+Space (or press the in-window button)
  → mic capture (cpal), 16 kHz mono
  → speech model (Parakeet / Whisper / Hinglish, ONNX Runtime, on-device)
  → optional smart formatting (offline LLM, or your own Gemini / Groq / Claude key)
  → paste into the focused app (clipboard restored afterwards)
```

## Features

- **Hold-to-talk everywhere** — a global hotkey, or the hold button in the app itself.
- **Floating pill** — a small overlay near your text field shows the words as you speak, with a green check when they're ready to type.
- **Wispr-style stats** — words, speed vs. typing, time saved, day streaks, weekly bars and the apps you dictate into most.
- **Smart formatting** — numbers, dates, spoken punctuation and self-corrections cleaned up offline; or route it through a cloud AI with your own API key (text only, stored in Windows Credential Manager / the macOS keychain).
- **Personal dictionary** — names and product terms spelled exactly the way you say them.
- **Hinglish** — models that type romanized Hindi + English mixed speech directly ("Main aaj subah office ja raha hoon").
- **Microphone doctor** — probe each microphone to find out whether it actually receives sound.
- **Light, dark or system theme** — a graphite-mono interface that follows Windows' color mode.

## Installing

Download the latest **draft release** artifacts from the
[Releases page](https://github.com/OWNER/REPO/releases) once you have pushed and published one
(see [CONTRIBUTING.md](CONTRIBUTING.md) for producing them with the included workflow):

| Platform | File | Notes |
| --- | --- | --- |
| Windows 10/11 | `.exe` (NSIS) or `.msi` | Installer for 64-bit Windows; WebView2 is installed on demand. |
| macOS 12+ (Apple Silicon) | `.dmg` | **Unsigned** — first launch: right-click the app → **Open** (or allow it in System Settings → Privacy & Security), because the binary is not signed with an Apple Developer certificate. |

## The speech models

Models are **downloaded once at first run** into `%LOCALAPPDATA%\SrutamDictate\models` on Windows
(the equivalent application-data folder on macOS). They are never bundled with the app, so each
model keeps its own license — check the license on its original download page:

| Model | Size on disk | Good for |
| --- | --- | --- |
| Parakeet V3 | ≈478 MB | English + 24 European languages; best all-round accuracy, recommended default. |
| Parakeet V2 | ≈631 MB | English only, slightly more accurate on English than V3. |
| Hybrid: Parakeet + Whisper | ≈478 MB + 806 MB | Parakeet's speed, with a slow second pass for whispers. |
| Whisper Large v3 Turbo | ≈806 MB | Whispers, accents, rare words; a few seconds per dictation on CPU. |
| Whisper Hinglish Apex | ≈806 MB | Fine-tuned for Hindi + English mixed speech; types romanized Hinglish directly. |
| Whisper Hinglish Swift | ≈57 MB | Small and quick Hinglish dictation; the smallest good-enough choice for it. |
| Whisper Tiny | ≈75 MB | Multilingual experiment — quick everywhere, rough results. |
| Moonshine Base / Tiny | ≈55 / 31 MB | Smallest and fastest English models for busy or old PCs. |

A real-audio benchmark of the Hinglish candidates (recorded clips from the open HiACC / MUCS
datasets) lives in [`src-tauri/bench`](src-tauri/bench) with its committed result summaries.

## Build from source

Prerequisites: Rust (stable, MSVC on Windows), Node 20+, the Visual Studio C++ build tools on
Windows, and Xcode command-line tools on macOS.

```bash
npm install
npm run tauri dev      # run the app in development
npm run tauri build    # produce installers for your platform
```

Run the test suite (the project's tests live in the Rust crate):

```bash
cd src-tauri
cargo test --release --lib
```

## Privacy

- Recording, transcription and formatting all run on-device by default. Nothing is sent anywhere.
- Choosing a cloud engine sends only the *transcribed text* (never audio) to that provider, and
  only because you supplied the key. If it can't be reached, the local model takes over.
- Recordings are kept on disk so you can replay them; settings, stats and recordings never leave
  the machine. There is no telemetry of any kind.

## Roadmap

See [PLAN.md](PLAN.md). Machine-specific notes live in [docs/GPU.md](docs/GPU.md).

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for the project layout, how to
run the tests and the commit conventions.

## License

[MIT](LICENSE). The speech models referenced above are downloaded at runtime and remain under
their own respective licenses.
