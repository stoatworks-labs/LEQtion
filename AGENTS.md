# AGENTS.md — bringing an LLM up to speed on LEQtion

Orientation for an AI assistant (or a new human) picking this up cold. `README.md` is the
user-facing story; this file is the map, the invariants, and how to tell finished work from
scaffolding.

---

## 1. What this is

A **desktop sound level meter and dual-channel analyser**. Tauri v2 shell, React + TypeScript
frontend, Rust doing every part of the measurement. Public repo, MIT.

The product is the *numbers*. The tiles are how they get looked at, but a bug in the UI is
cosmetic and a bug in the DSP is a wrong measurement someone might quote in a licensing
dispute. Weight your effort accordingly.

## 2. Layout

```
src/                       React frontend. Presentation only.
  types.ts                   Mirrors the Rust wire types by hand. Keep in step.
  lib/ipc.ts                 The ONLY file that calls invoke/listen. Frame bus lives here.
  lib/plot.ts                Axis maths + colour ramp. Pure, tested.
  lib/format.ts              Level/duration formatting. Mirrors Rust label logic, tested.
  lib/useFrame.ts            Frame hooks: ref for canvases, throttled state for readouts.
  state/store.ts             zustand: config, devices, layout, calibration.
  tiles/                     One file per tile kind + registry.tsx
  ui/                        App shell, grid, toolbar, device bar, calibration dialog.

src-tauri/
  src/lib.rs                 Tauri commands. Thin — no analysis, ever.
  src/session.rs             Audio → analysis thread → frame events.
  src/settings.rs            One JSON file, written atomically.
  src/logger.rs              CSV log.
  src/project.rs             Projects and shows. See docs/tuning.md §1.
  crates/leqtion-dsp/        THE IMPORTANT CRATE. Pure DSP, no I/O, 139 tests.
                               generator.rs  signal sources
                               transfer.rs   multi-time-window TF, coherence, delay
                               history.rs    levels over time, per-interval buckets
  crates/leqtion-audio/      cpal capture, host/device enumeration, lock-free ring.
  crates/diag/               Vendored fleet logging/crash crate. Don't edit here.
  examples/meter.rs          Whole chain, no window. The end-to-end check.
```

**`leqtion-dsp` must stay free of I/O, threads and clocks.** Everything in it is a function
of the samples handed to it. That is what makes a sound level meter testable, and it is why
every claim the README makes about accuracy is a test rather than an assertion.

**`src/` must not do arithmetic that belongs in Rust.** The frontend never computes a band
table, a level, or a weighting. It renders what the engine sends. If a component starts
calculating a frequency or a decibel, the design has gone wrong — the axis labels and the
data plotted against them can only be guaranteed to agree if there is one implementation.

## 3. Build, run, test

```bash
npm install
npm run app            # tauri dev
npm run app:build      # bundle
npm test               # vitest, 40 tests
npm run typecheck      # tsc -b across app/node/test projects
npm run lint           # oxlint
```

```bash
cd src-tauri && cargo test --workspace     # 181 tests
```

```bash
cd src-tauri && cargo clippy --workspace --all-targets
```

Verify against real hardware — this is the step that catches what the unit tests cannot:

```bash
cd src-tauri && cargo run --example meter -- --seconds 30
```

## 4. The invariants that matter

### 4.1 Averaging happens in the energy domain, always

Mean squares are averaged, never decibels. A mean of decibels is not a level. This holds in
`leq.rs`, `spl.rs`, `spectrum.rs` and `calibration.rs`, and
`leq::tests::averaging_decibels_would_give_a_different_answer` exists specifically to fail
if someone "simplifies" an accumulator into a mean of levels.

### 4.2 LEQ is filtered in the time domain

Not weighted band-by-band after an FFT. See the module docs in `weighting.rs` for why. The
engine runs A, C and Z filters in parallel and hands block mean squares to the accumulators.

### 4.3 The weighting filter design is measured, not assumed

`weighting.rs` does not use a plain bilinear transform, and the reason is a 13.6 dB error at
19 kHz. Read the module docs before changing anything there. Two things have already been
tried and rejected with numbers, both recorded in comments so they are not tried again:

- pre-warping the bilinear sections (broadband error 0.23 → 0.65 dB), and
- an energy-weighted fit objective (worst-case grew to 2.5 dB for a 0.03 dB gain).

`filter_accuracy_is_pinned_per_sample_rate` holds the published figures. If you improve the
design, lower those numbers **and** the table in the README and the module docs — they are
meant to agree.

### 4.4 Uncalibrated levels are never presented as SPL

`Frame::calibrated` is false until a calibration is loaded, and every readout that shows a
level also shows whether it is dB SPL or dBFS. Do not add a display path that omits it.

An offset can now arrive without a calibrator in the room, so `Calibration::source`
records which of three things produced it — see `CalibrationSource`:

- `Calibrator` — a hardware calibrator, on this capsule, on this input, at this gain. The
  only one that measured the chain actually in use.
- `PlatformSpec` — the platform specifies the sensitivity of the capture path. Android's
  unprocessed audio source: CDD **C-1-5** requires 94 dB SPL at 1 kHz to read −36 dBFS,
  which is an offset of exactly **130 dB** on any device declaring
  `PROPERTY_SUPPORT_AUDIO_SOURCE_UNPROCESSED`. Derived, sourced and tested in
  `leqtion-audio::profiles`.
- `DeviceProfile` — measured on one unit of a model and assumed to hold for the rest.

**Only `Calibrator` may be described as "calibrated" without qualification.** The other two
are good enough to display an SPL and not good enough to imply a calibrator was used, so
the source travels with the number to the readout. This extends the rule above rather than
loosening it: the failure mode is the one recorded in the `calibration.rs` module docs — an
offset that is wrong is invisible afterwards, because everything downstream of it is
self-consistent and uniformly wrong.

A user calibration always beats a profile. Resolution is in `src/lib.rs` where the stored
calibration is applied, and the fallback is deliberately `or_else`, not a merge.

### 4.4a The synthetic input can never be calibrated

`leqtion-audio::synthetic` offers the generator as a *backend*, so the analyser can be run
against a known level with no device in the chain. `begin_calibration` and
`accept_calibration` both refuse while it is open, and that refusal must stay.

The reason is not tidiness. A generated 1 kHz sine passes every gate the calibration
workflow has — perfectly steady, exactly on frequency, unclipped, far above the noise
floor — so without the guard it would sail through and produce a full-scale-to-SPL offset
invented out of nothing, which every reading afterwards would inherit and which
`settings.json` would keep. The engine cannot catch this: from inside the analysis there is
no difference between a calibrator on a capsule and a sine on a wire. Only the source
knows, which is why the check lives in `src/lib.rs` and not in `leqtion-dsp`.

### 4.4b The history and the log are the same points

`leqtion-dsp::history` records every level over time, and the CSV log writes a row when
an interval completes — from `Engine::history_latest`, not from a timer of its own. Two
clocks sampling one measurement would disagree about the same second, and the log is the
copy someone keeps.

Three things in there are load-bearing:

- **A point is an interval, not a sample.** `min`, `mean`, `max` cover the whole
  interval; sampling instantaneously would alias, and a transient would exist only if it
  landed on a tick. `mean` is an energy mean.
- **Downsampling happens in `History::view`, never in the UI.** Thinning a level trace by
  dropping points loses exactly the peaks it was drawn to show, so a view keeps the min of
  the mins and the max of the maxes. Zooming out may lose time resolution; it must never
  lose level.
- **Every log row carries `calibrated` and `dropped_frames`.** A calibration can change
  mid-log, and a gap in the audio means the period is short by an unknown amount. Both
  have to be in the file, not just on screen.

### 4.4c A profile is never inferred from `target_os`

On Android the guarantee attaches to **one audio source**, not to the operating system. A
stream opened on any other source on the same handset carries no guarantee, and applying
the 130 dB offset to it would produce a confident, wrong SPL — the §4.4a failure again,
reached by a different route. So `profiles::current_input_path()` must return
`InputPath::AndroidUnprocessed` only when something has actually requested the unprocessed
source *and* had the request honoured.

It returns `Unknown` today, and `nothing_claims_a_known_gain_until_the_port_lands` pins
that. **cpal cannot currently request the unprocessed path** — its AAudio backend sets only
device id, performance mode and sample rate, never `AAudioStreamBuilder_setInputPreset`,
and AAudio's default input preset is `VOICE_RECOGNITION`, which is processed. Reaching the
guarantee needs a patched cpal or a small AAudio shim, plus a JNI read of the `AudioManager`
property. Until then Android would show dBFS, which is correct rather than missing.

On iOS the session mode must be `AVAudioSessionModeMeasurement` before the input opens, or
AGC is in the path and no offset means anything. `IOS_PROFILES` is empty because Apple
publishes no sensitivity figure and there is no public per-model dataset; entries may only
come from a calibrator run on that model.

### 4.5 The audio callback does not allocate, lock or block

It copies into a lock-free ring and nothing else. If the ring overflows, frames are counted
as dropped and the UI says the measurement is invalid. Never "fix" a dropout by making the
callback wait.

### 4.6 Coherence is the transfer function's honesty feature

Never draw a transfer function without it. The magnitude trace breaks where coherence falls
below the floor, and points are faded in proportion. Do not add a display path that hides
it, and do not report coherence before `MIN_FRAMES_FOR_COHERENCE` — a single frame always
reads exactly 1.0 and that is not a measurement, it is an artefact of the arithmetic.

### 4.7 The reference is delay-compensated, never the measurement

The measurement always arrives later; delaying it instead would mean predicting the future.
Changing the delay throws away every average, because the spectra accumulated so far were
measured against a differently aligned reference.

### 4.8 The generator never starts by itself

`Signal::Off` is the default and is not restored from settings on launch. Level and
band-limiting persist; the signal does not. Opening a measurement app must never put pink
noise into a rig before anyone has touched anything.

### 4.9 Band levels come from the plan the engine returned

`plan_revision` on a frame changes when the band table changes. The UI refetches rather than
building its own. `bands_db.length === plan.bands.length` is checked before drawing.

### 4.10 A show records a calibration; it never applies one

A **show** (`project.rs`) is a complete saved configuration — engine, transfer, generator,
input, tile layout. It stores a *snapshot* of the calibration that was in force, and that
snapshot never reaches the engine: `Show::restore` returns a `ShowRestore`, which has no
calibration field at all. Live calibration always comes from the device-keyed table in
`settings.json`.

A calibration belongs to a microphone and a preamp, not to an application state. If a show
carried its own offset, opening last year's show with this year's microphone would present
dB SPL derived from hardware that is no longer in the room, and nothing downstream could
detect it — the same failure §4.4a refuses for the synthetic input, arriving by a different
door. The snapshot is kept only so a stored trace can say what it was measured against.

### 4.11 Loading a show never starts a generator

§4.8 applies to shows as well as to launch, and more strongly: a show can be loaded with an
output stream already open and a system live. `Show::restore` forces `Signal::Off` while
keeping the level and band-limiting, so loading a show that was saved mid-measurement stops
the noise rather than starting it.

### 4.12 Nothing requires a project

The app meters, logs and calibrates with no project open, exactly as it did before projects
existed. `settings.json` holds a *pointer* to the last open project, and a project that has
been moved or deleted since resolves to nothing rather than to an error. Someone opening a
meter to check a level must never have to name a project first.

## 5. Traps

**Spectrum normalisation is `2/(N·S2)`.** The `1/N` is easy to omit and costs 10·log10(N) —
42 dB at a 16384-point transform. `spectrum::tests::full_scale_sine_reads_zero_db_in_its_own_band`
catches it. (The sibling browser project `simpleRTA` had the same bug independently.)

**The IEC 61672 table is tabulated at exact midband frequencies, not nominal ones.**
Evaluating A-weighting at 31.5 Hz gives −39.53 dB against a published −39.4; the table means
10^(15/10) = 31.623 Hz. `midband_hz()` in the tests exists for this.

**`F_MIN`/`F_MAX` in `bands.rs` are 19 and 20500, not 20 and 20000.** The band everyone calls
"20 Hz" has an exact base-2 centre of 19.686 Hz. Tightening these to round numbers silently
drops both end bands from a 1/3-octave display.

**Input and output can be different devices.** On a laptop the input is "MacBook Pro
Microphone" and the output is "MacBook Pro Speakers", so asking for an output with the
input's name fails. The session picks the output device up front — same name if one exists,
default otherwise — and sets `clock_shared` accordingly. Two devices means two clocks, so
the internal reference drifts; the UI says so. On any real interface both sides share a
name and the fast path applies. `capture --list` prints both lists.

**The pink normalisation constant is measured, not derived.** `PINK_NORMALISATION` is the
reciprocal of the pinking filter's RMS gain, measured over four million samples. It does not
depend on the sample rate. If pink noise comes out at the wrong level, this is the number.

**Deleting a project or a show is a move, not an unlink.** Both go to a `.deleted/` folder
and the command returns the path, which the UI shows. A button labelled "Delete" that
actually means "move" has to say so, and an unreadable show file is left exactly where it is
rather than replaced with a default — unlike a tile layout, a show is work someone did.

**Project names become directory names, so they are sanitised for Windows.** Windows forbids
`\ / : * ? " < > |`, silently strips trailing dots and spaces, and reserves `CON`, `PRN`,
`AUX`, `NUL`, `COM1`–`COM9` and `LPT1`–`LPT9` even with an extension. A name that saves on
macOS and cannot be opened on Windows is the failure this prevents; leading dots go too,
because a hidden directory would save and then never appear in the list.

**cpal 0.18 renamed things.** `device.description()?.name()`, not `device.name()`.
`SampleRate` is a plain `u32` alias. One `cpal::Error`, no `StreamError`.
`build_input_stream` takes the config **by value**.

**macOS needs `NSMicrophoneUsageDescription`** (in `src-tauri/Info.plist`) or the process is
killed when it opens an input, and the crash points at the audio driver rather than the
missing key. A `tauri dev` build is not bundled, so permission is attributed to the terminal.

**Every sample exactly zero** means macOS denied microphone access — the stream opens and
the callback fires regardless. `examples/capture.rs` detects and explains this, and the app
does too: `Engine` counts unbroken zero seconds into `Frame::input_silent_seconds` and
`SilentInputBanner` says so on screen. The engine only reports the fact; naming the cause is
the caller's job, since a generator switched off looks identical from inside the analysis.

**A denied microphone can also present as a *hang* rather than as silence.** Which one you
get depends on the state of the TCC entry for the bundle id, and the entry is keyed to a
code signature — so a Developer ID build and an ad-hoc `cargo build` of the same app do not
share it. `Failed to match existing code requirement for subject com.allansargeant.leqtion
and service kTCCServiceMicrophone` in `log show --predicate 'subsystem == "com.apple.TCC"'`
is the tell, and `tccutil reset Microphone com.allansargeant.leqtion` is the fix. Neither
failure reaches the app as an error.

## 6. What has and has not been verified

Verified:

- The DSP, extensively, against synthetic signals with known answers (117 tests).
- The audio path against real Core Audio hardware — enumeration across Dante, NDI, L-ISA and
  Pro Tools bridge devices, and capture from the built-in microphone with zero drops.
- The whole chain end to end via `examples/meter.rs`: A < C < Z on room noise, sliding LEQ
  tracking, elapsed LEQ settling, peak held.
- The app builds, bundles, launches, and lays out its tiles.
- **The running GUI, on the synthetic input.** The app was driven from the `Signal
  generator` backend on pink noise at −20 dBFS at 48 kHz: the RTA reads flat across the
  band, which is what pink noise must look like on a constant-percentage-bandwidth
  display, and LAF and LAeq agree with each other to 0.1 dB. That exercises the whole
  chain — capture source, ring, analysis thread, engine, frame events, every tile — with
  no hardware at all.

**Not** verified:

- Against a reference sound level meter. No absolute accuracy claim is supported.
- With a hardware calibrator. No calibrator has been connected, so `Ready` and
  `accept_calibration` have never run against real hardware. The *rejection* paths have:
  the dialog was driven against a live microphone and reported `Clipping` and
  `WrongFrequency` with real values. That exercise is what found both calibration bugs
  fixed since — the wire-format one that took the whole UI down, and the held-peak one that
  pinned a fresh run at `Clipping`. Assume the accept path still has its own.
- ASIO. It compiles behind a feature flag and has never carried audio — the only Windows
  machine here is ARM64, where ASIO drivers barely exist. See `docs/asio.md`.
- Anything downstream of a converter. No signal has been round-tripped through an
  interface, a loopback cable or a loudspeaker, so the transfer function and the delay
  finder have only ever seen the internal tap.

Note on driving the GUI: coordinate clicking (`click at`) fails on this machine with
AppleScript error −25204, which is what "assistive access is denied" looks like and is why
an earlier version of this section said the UI could not be automated. It can. The
webview publishes its controls to the accessibility tree, so
`click button "Start" of group 1 of UI element 1 of scroll area 1 of …` works where a
coordinate click does not. Prefer that over synthetic clicks, and prefer `examples/meter.rs`
over both when the question is about numbers rather than about the UI.

Keep this section honest. It is the part someone will rely on.

## Notes

`docs/NOTES.md` carries this repo's working notes — current status, decisions
already made, and the traps that have actually bitten. Read it before changing
anything non-obvious. Cross-cutting fleet knowledge lives in
[fleet-notes](https://github.com/stoatworks-labs/fleet-notes).
