## LEQtion v0.1.2

- **The app now asks for the microphone.** v0.1.1 never did: it was signed with the
  hardened runtime but without the audio-input entitlement, and macOS's answer to that is
  silent refusal — no prompt, no error, and no LEQtion row under Privacy & Security →
  Microphone to switch on by hand. Every input, from the built-in microphone to Dante
  Virtual Soundcard, metered exactly digital silence. Update, launch, click Allow.
- Fixed the field casing between the engine and the window that emptied the whole
  window when the Analysis panel was opened, broke the Sweep signal, and would have
  taken the app down at the moment a calibration was about to succeed. A display fault
  now says so in one tile instead of blanking everything.
- A single clip no longer pins every later calibration attempt at "Clipping".

macOS only, arm64 and x86_64, Developer ID-signed and notarised. Windows and Linux
compile but have no builds — see the README.
