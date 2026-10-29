**Sruthi 0.4.3: a proper first run.**

- **First-run setup.** A new install now opens with a short splash, then asks (optionally) what to call you and which languages you dictate in. It looks at your PC (processor, memory, graphics card and its memory, free disk) and recommends one speech model with a few alternatives, each marked as a good fit or a slow one for your hardware. The download runs while you pick your shortcut and microphone, and the last step is a real dictation to try it. Skip it at any step and Sruthi installs the recommended model for you.
- **Run setup again.** Settings has a button for it, and Home now greets you by name.
- **Only models your system can run.** The Whisper models and the graphics-card path are Windows-only, so on a Mac the Models page hides them, setup recommends Parakeet or Moonshine, and a Whisper download is refused with a clear message instead of failing.
- **Graphics cards.** The README now says plainly which cards work: any NVIDIA, AMD or Intel card with Vulkan, on Windows.
- **Housekeeping.** Intel Macs are not supported (the speech engine has no Intel Mac build), and the planning notes are no longer part of the repository.

Everything from 0.4.2 is included: My month and All time share cards, About Sruthi in Settings, and a SHA256SUMS.txt with every release.

Windows is the primary platform. The macOS build is Apple Silicon only and unsigned (right-click the app and choose Open on first launch), and the app-aware features need Windows. The Windows installer is not code-signed, so Windows may show "Windows protected your PC"; choose More info, then Run anyway.
