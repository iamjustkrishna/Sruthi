# Sruthi: roadmap

Goal: an offline Wispr Flow. English first, then more languages. It ships as a standalone desktop app
and is later folded into the Srutam app.

## Pipeline

| Stage | Offline choice |
|---|---|
| Trigger + overlay | Global hotkey (push-to-talk) + floating always-on-top pill |
| Capture + VAD | cpal → 16 kHz mono; Silero VAD (planned) |
| Speech-to-text | Parakeet TDT 0.6B v3 int8 (ONNX); Whisper turbo via whisper.cpp for more languages |
| Formatter | Rules pass + small local LLM (Qwen3 1.7B / Gemma 3 1B, llama.cpp) with a hallucination guard |
| Insert | Clipboard + simulated paste, clipboard restored |

## Phases

1. **Windows MVP** ✅ scaffolded: hotkey → record → Parakeet → paste, overlay pill, tray, model downloader, history
2. Formatter layer: filler removal, self-corrections, punctuation and lists; "raw" vs "smart" modes; guard that falls back to raw text
3. Personal dictionary and snippets, app-aware styles (Slack vs email vs code editor), configurable hotkey
4. Streaming transcription with VAD chunks while you speak (target under 700 ms after release)
5. macOS build (Metal/CoreML, Accessibility + Microphone permissions, notarization)
6. Android: custom keyboard (IME) with a mic key, sharing the Rust core
7. iOS (keyboard extension → app handoff for the mic)
8. Multilingual packs (Whisper turbo, IndicConformer, Hinglish code-switching) and integration into Srutam

## Dev machine notes

The dev machine is a Ryzen 5 5600H with 16 GB RAM and a Radeon RX 6500M (4 GB, AMD, so use Vulkan/DirectML rather than CUDA).
Parakeet runs well on CPU alone.
