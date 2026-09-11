## LEQtion v0.1.2

- **The app now asks for the microphone.** v0.1.1 never did: it was signed with the
  hardened runtime but without the audio-input entitlement, and macOS's answer to that is
  silent refusal — no prompt, no error, and no LEQtion row under Privacy & Security →
  Microphone to switch on by hand. Every input, from the built-in microphone to Dante
  Virtual Soundcard, metered exactly digital silence. Update, launch, click Allow.
- **The RTA no longer grows a comb below its resolution limit.** At fine resolutions with
  a small transform — 1/48 octave at 2048 points, say — bands that happened to sit on an
  FFT bin centre read 7–14 dB above their neighbours, at every multiple of the bin spacing.
  Bands now integrate the fraction of each bin they actually cover, so a flat spectrum
  reads flat at every width and the shaded "interpolated" region joins the measured one
  without a step.
- **A live peak-frequency readout across the top of the RTA.** It names the tallest band
  and its level, in the same unit as the SPL tile, and refines the frequency to a fraction
  of a bin when a tone is what tops it — "997 Hz" at 1/3 octave rather than "1k". It reads
  the same average the bars are drawn from, so it settles as they do.
- Fixed the field casing between the engine and the window that emptied the whole
  window when the Analysis panel was opened, broke the Sweep signal, and would have
  taken the app down at the moment a calibration was about to succeed. A display fault
  now says so in one tile instead of blanking everything.
- A single clip no longer pins every later calibration attempt at "Clipping".

macOS only, arm64 and x86_64, Developer ID-signed and notarised. Windows and Linux
compile but have no builds — see the README.
