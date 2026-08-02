First release.

A desktop sound level meter and dual-channel analyser: fractional-octave RTA,
spectrograph, bargraph, time-weighted SPL, as many user-defined LEQs as you
want, level history charts, CSV logging, a signal generator, and transfer
function measurement with phase, coherence and delay finding — on a grid of
tiles you lay out yourself.

## What is in it

- **RTA** from 1/1 to 1/48 octave, with selectable transform size, window,
  overlap, averaging and peak hold.
- **Spectrograph** on the same log frequency axis, so it lines up under the RTA.
- **SPL** — Fast, Slow and Impulse, with max, min and peak.
- **LEQ** — as many as you like, each with its own window length and A, C or Z
  weighting. They integrate in the engine, so one keeps running whether or not a
  tile is showing it.
- **Level history** — any of those levels over time as a line. Each point covers
  a whole interval, and the band around the line is the min and max *inside* it,
  so a transient between ticks is on the chart rather than missed.
- **CSV logging** — one row per interval, every series at once, written from the
  chart's own points. Every row records whether it is calibrated and how many
  frames have been dropped.
- **Signal generator** — pink, white, sine and a repeating log sweep, with the
  peak it will actually reach shown beside the RMS level.
- **Transfer function** — magnitude, phase and coherence against the generator
  tapped internally or a hardware loopback, plus delay finding from the impulse
  response.
- **Signal generator as an input** — the same signals as a *backend*, so the
  analyser can be checked against a known level on a machine with nothing
  plugged in.

## Two measurement decisions worth knowing

**LEQ is filtered in the time domain, not weighted after an FFT.** Deriving it
by tilting a spectrum makes the answer depend on the transform size, the window
and the overlap, none of which have anything to do with the sound.

**The A-weighting filter is fitted, not assumed.** The textbook bilinear design
is 13.6 dB out at 19 kHz; this one is under 1 dB, because the pole-only sections
are matched-Z with a shaping zero solved per sample rate.

## Downloads

macOS only, on both architectures. That is a statement about the *builds*, not
the code: Tauri cannot cross-bundle, so a Windows installer needs a Windows host
and a Linux AppImage needs a Linux one. The only Windows machine here is ARM64,
which would produce an arm64-only installer — not what a Windows user with an
audio interface is running, and worse than shipping none.

The apps are **ad-hoc signed and not notarised**. macOS will refuse them on
first open; right-click → Open, or clear the quarantine attribute.

## Honestly

This is alpha. It has never been checked against a reference sound level meter,
no hardware calibrator has been connected to it, and no signal has been
round-tripped through a converter, a loopback cable or a loudspeaker — so the
transfer function and the delay finder have only ever seen the internal tap. The
ASIO path compiles behind a feature flag and has never carried audio.

The DSP is verified numerically against signals whose answers are known in
advance, and that is a different claim from being right about a room.
`docs/field-test.md` is the checklist for closing the gap.

**Not a certified sound level meter.** Until you calibrate against a hardware
calibrator every level is a full-scale level, and the app says so on every
readout.
