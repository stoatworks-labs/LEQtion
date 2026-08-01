# Field test checklist

Everything in LEQtion is verified against synthetic signals with known answers. This is the
list of things that **only real hardware can settle**, written down so it survives between
sessions. Nothing here has been done yet.

Work top to bottom: each step assumes the ones above it passed. Record what you actually
saw, not what you expected — a result that disagrees with this document means this document
is wrong.

---

## 0. Before anything

```bash
cd src-tauri && cargo run -p leqtion-audio --example capture -- --list
```

Check the interface appears under **both** `inputs:` and `outputs:` with the *same name*. If
it does, input and output share a clock and the internal reference will hold alignment
indefinitely. If it does not, expect drift and re-find the delay every few minutes — the
generator tile says so when it happens.

---

## 1. Calibration — the one that matters most

Needs a class 1 or class 2 acoustic calibrator and the measurement microphone.

1. Fit the calibrator, switch it on, open **Calibrate…**.
2. Confirm it settles and reports **Ready** within a few seconds.
3. Note the offset and the resulting full-scale SPL.
4. Accept, then read the SPL tile: **it must show the calibrator's own figure** — 94.0 dB
   for the 94 dB setting.
5. Switch the calibrator to 114 dB without touching preamp gain. The SPL tile must read
   114.0 dB, and re-calibrating at that setting must give **the same offset** to within a
   few hundredths.

Then try to make it refuse, and check each refusal explains itself usefully:

- lift the calibrator half off the capsule → **unstable**, with a spread figure
- switch to the 250 Hz setting while the target says 1 kHz → **wrong frequency**
- wind the preamp up until it clips → **clipping**
- switch the calibrator off → **too quiet**

**What this cannot check:** that the calibrator is itself in calibration. If a reference
meter is available, compare a steady source side by side and record the difference.

## 2. SPL and LEQ against a reference meter

With the same source and both microphones in the same place:

- LAF and LCF within a decibel or so of the reference
- LAeq over a fixed period — one minute is plenty — compared against the reference's LAeq
- Lmax and Lpeak after a hand clap

Record the numbers. Any consistent offset is worth chasing; a difference that changes with
level or spectrum is worth chasing harder.

## 3. RTA

Feed pink noise from the generator into a system and check the RTA is roughly flat, then
check the two ends: the 20 Hz and 20 kHz bands must exist at 1/3 octave. Sweep the
resolution 1/1 → 1/48 and confirm band centres stay put rather than sliding.

## 4. Generator

- Pink at −20 dBFS out of a chosen channel, into a metered input: confirm the level and
  that **only that channel** carries signal.
- Move the output channel while it is running. It must not click or drop out.
- Drag the level slider through its full range. No clicks, no steps.
- Set pink to −6 dBFS RMS and confirm the tile warns about clipping *before* the input
  peak meter shows it.

## 5. Transfer function — electrical loopback first

This is the step that proves the whole chain, and it needs one cable.

1. Patch an output back to a spare input.
2. Reference → **Loopback on input N**. Generator → pink, −20 dBFS.
3. **Find delay** → apply. Expect a small figure — a few samples of converter latency.
4. The magnitude trace must be **flat within a fraction of a decibel**, phase **flat at
   zero**, coherence **at 1.0 across the whole range**.

If that is not what you see, stop. Nothing further is trustworthy until it is.

Then put something known in the loop — a graphic EQ with one band pulled, or a processor
with a crossover — and confirm the measured response matches what the device says it is
doing, in both magnitude *and* phase.

## 6. Transfer function — acoustic

1. Loudspeaker fed from the generator, measurement microphone at a known distance.
2. **Find delay**. The **metres** figure must match the tape measure — that is the single
   best check that the delay finder is honest. Confidence should be high; a low confidence
   figure usually means it locked onto a reflection rather than the direct sound.
3. Coherence should be high through the middle and fall away at both extremes. Cover the
   microphone and confirm coherence collapses.
4. Move the microphone a metre further away and re-find: the delay must increase by
   about 2.9 ms.

## 7. Internal reference versus loopback

Measure the same system both ways. The two should agree except for whatever the output
converter and any output processing contribute — which is exactly the difference the
internal tap cannot see. Note how large that difference actually is; it is the honest answer
to "does the internal reference matter?"

## 8. Long run

Leave a measurement going for an hour with an elapsed LEQ.

- `dropped frames` must stay at zero. Any other value invalidates the LEQ.
- `reference underruns` must stay at zero on a shared clock.
- On a split clock, note how far the delay has drifted after an hour.

## 9. ASIO, if a Windows machine with an interface is to hand

See [asio.md](asio.md). Nothing about the ASIO path has ever carried audio.

---

## Recording the results

Put the numbers in this file, under a dated heading, and commit them. A checklist with no
results is just a plan; the value is in the record of what the thing actually did.
