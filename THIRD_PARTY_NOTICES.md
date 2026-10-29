# Third-party notices

Sruthi is free software under the [GNU GPL v3 or later](LICENSE). It is built on work by other people, listed here with the terms it comes under. This is a good-faith summary, not legal advice; the authoritative text is each project's own license file.

Nothing below changes the license of Sruthi's own code. Everything listed is either permissive or has a permissive option that is compatible with the GPL.

## What ships inside the app

### Rust libraries

Sruthi links about 600 crates (listing them all here would go stale on every update). `src-tauri/Cargo.lock` is the exact list, and `cargo metadata` prints each crate's declared license. At the time of writing (Sruthi 0.4.1):

| License | Crates | Notes |
| --- | --- | --- |
| MIT, Apache-2.0, or "MIT OR Apache-2.0" | the large majority | Permissive. Apache-2.0 is compatible with GPL v3. |
| Zlib, ISC, BSD-2/3-Clause, 0BSD, Unlicense, CC0, Unicode-3.0, BSL-1.0, CDLA-Permissive-2.0 | the rest, bar those below | Permissive. Their notices are kept in each crate's source. |
| MPL-2.0 | 5 (`cssparser`, `cssparser-macros`, `dtoa-short`, `option-ext`, `selectors`) | File-level copyleft. MPL-2.0 section 3.3 explicitly allows combining with GPL code. The MPL files stay available from crates.io, unmodified. |
| "... OR LGPL-2.1-or-later" | 2 (`r-efi`) | A third option on a dual license; we use the MIT or Apache option. |

No crate is GPL-only, AGPL, or under a non-commercial license.

### Vendored code

`src-tauri/vendor/transcribe-rs` is a patched copy of [transcribe-rs](https://github.com/cjpais/transcribe-rs) (MIT, copyright Ilya Stupakov and contributors). Its own `LICENSE` is kept in that folder and the changes are described in `SRUTHI_PATCH.md`. The files there keep their original license and carry no Sruthi header.

### Native runtimes

- **[ONNX Runtime](https://github.com/microsoft/onnxruntime)** (MIT), fetched by the `ort` crate at build time. It runs Parakeet and Moonshine.
- **[Tauri](https://tauri.app)** (Apache-2.0 or MIT) and the system webview (WebView2 on Windows, WebKit on macOS).

### Front-end libraries

- `@tauri-apps/api` and `@tauri-apps/plugin-opener` (Apache-2.0 or MIT).
- Build tools (`vite`, `typescript`) are used to build the app and are not shipped in it.

### Artwork

- App logos on Home are from [Simple Icons](https://simpleicons.org) (CC0). The logos remain trademarks of their owners and are shown only to say which app a dictation went into. See `src/assets/apps/NOTICE.txt`.
- The Sruthi logo and icons are Sruthi's own.

## What the app downloads on your request

These are not part of the installer. The app fetches them when you pick them, and they stay under their authors' terms.

| What | Where from | License |
| --- | --- | --- |
| whisper.cpp engine (CPU and Vulkan builds) | [ggml-org/whisper.cpp](https://github.com/ggml-org/whisper.cpp) releases | MIT |
| llama.cpp engine (`llama-server`) | [ggml-org/llama.cpp](https://github.com/ggml-org/llama.cpp) releases | MIT |
| Speech models (Parakeet, Whisper, Moonshine, Hinglish fine-tunes) | Hugging Face | each model's own license; check its download page |
| Clean-up models (Qwen3 and Qwen3.5 GGUF builds) | Hugging Face (`unsloth`) | each model's own license; check its download page |

Models are separate works from Sruthi. Some carry attribution requirements (for example Creative Commons "BY") or limits on commercial use. If you redistribute a model, follow its license, not Sruthi's.

## Cloud clean-up

If you add your own API key, Sruthi sends the text of a dictation (never audio) to Gemini, Groq or Claude. Those services have their own terms; the key is yours and stays in your operating system's credential store.
