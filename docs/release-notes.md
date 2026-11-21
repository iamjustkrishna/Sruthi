**Sruthi 0.6.0: choose the script you type in.**

- **Latin or Native output.** A new switch in Settings decides the script of pasted dictations. Latin is the default and works everywhere. Native turns romanized Hindi into Devanagari with the local AI model, and converts Devanagari speech output back to Latin when you pick Latin.
- **The script pass runs last.** It happens after corrections and saved phrases, so nothing rewrites the converted text afterwards. English words, numbers and punctuation are left alone.
- **Your saved words follow the switch.** Changing it converts your saved phrases and recent corrected references too. A converted copy of the last dictation goes to the clipboard with a short toast — you paste it over the old text yourself. Sruthi never selects all in another app, so a long document cannot be destroyed by a setting change.
- **Offline and local.** Both directions run on this PC. No text is sent anywhere, and no telemetry is collected.

Everything from 0.5.0 is included: the heat guard, heavy-model warnings and measured wait estimates. From 0.4.3: first-run setup with a PC check and model recommendation, models limited to what your system can run, and the My month and All time share cards.

Windows is the primary platform. The macOS build is Apple Silicon only and unsigned (right-click the app and choose Open on first launch), and the app-aware features need Windows. The Windows installer is not code-signed, so Windows may show "Windows protected your PC"; choose More info, then Run anyway.
