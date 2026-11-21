**Sruthi 0.7.0: add a model by link.**

- **Paste a link on the Models page.** Give it a name and an HTTPS link to an ONNX model, a model archive, or a Hugging Face repo. It appears as a normal row with the same heat class, size and wait estimate as the built-in models.
- **Safe by construction.** Only Hugging Face, GitHub and the model hosts Sruthi already uses are accepted. Plain HTTP, local files and unknown hosts are rejected before anything is fetched. A corrupt saved registry never stops the built-in models from loading.
- **Removing a link keeps the files.** The Remove action deletes only the row here; downloaded models are never deleted automatically.
- **A single registry.** Built-in and added models now share one list, so the heat guard, the 90-second stop, memory checks and the benchmark all see them the same way.

Everything from 0.6.0 is included: the Latin/Native script toggle, converted saved phrases, and the clipboard copy of the last dictation. From 0.5.0: the heat guard, heavy-model warnings and measured wait estimates.

Windows is the primary platform. The macOS build is Apple Silicon only and unsigned (right-click the app and choose Open on first launch), and the app-aware features need Windows. The Windows installer is not code-signed, so Windows may show "Windows protected your PC"; choose More info, then Run anyway.
