**Sruthi 0.8.0: dictation in native Indian scripts.**

- **Indic Conformer.** A new public, MIT-licensed speech model for Hindi and seven other Indic languages. It runs fully offline on the CPU and writes Devanagari directly. Its CTC decoder is built in — no cloud, no gated checkpoint.
- **It follows the heat rules.** Indic Conformer is marked as a heavy model, so it shows the heat warning and a wait estimate before first use, and the heat guard can stop a long run. It never uses the graphics card.
- **Works with the script toggle.** Leave the script on Native to paste Devanagari, or switch to Latin to have the same output romanized at the end of the pipeline.
- **Experimental accuracy.** The ONNX graph and token set are verified, but the mel settings still need a real Hindi recording on your machine to be certain. If the words come out wrong, tell us — the decoder is isolated and easy to tune.

Everything from 0.7.0 is included: paste-a-link models, the runtime model registry, and safe link removal. From 0.6.0: the Latin/Native script toggle. From 0.5.0: the heat guard and measured wait estimates.

Windows is the primary platform. The macOS build is Apple Silicon only and unsigned (right-click the app and choose Open on first launch), and the app-aware features need Windows. The Windows installer is not code-signed, so Windows may show "Windows protected your PC"; choose More info, then Run anyway.
