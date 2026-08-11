# System tuning — design

LEQtion measures. This document designs the arc that turns those measurements into
**settings someone can put into a system processor**: projects and shows, a trace library,
target curves, an optimiser, a database of real processors, deployment, and delay
alignment.

It is written *before* the code because most of these steps are cheap in isolation and
expensive in the wrong order. The trace format in §2 is the load-bearing decision — steps
4 through 8 all read it, and step 8 (alignment) needs things a magnitude-only trace cannot
give back. Getting §2 wrong means rebuilding the library later with a migration nobody
wants to write.

Nothing here is built yet. Sections are marked with their status as work lands.

---

## 0. The order of work

| # | Step | Status |
|---|------|--------|
| 1 | Projects, containing shows | **built** — `src-tauri/src/project.rs`, `src/ui/ProjectBar.tsx` |
| 2 | Save/recall traces to a library | not started |
| 3 | Import/export traces as CSV/XML | not started |
| 4 | Load a trace as a target curve | not started |
| 5 | Smooth a trace into a simplified target | not started |
| 6 | Draw a target curve from scratch | not started |
| 7 | Optimise: suggest EQ to fit live → target | not started |
| 8 | Amplifier/processor database | not started |
| 9 | Constrain recommendations to the real processor | not started |
| 10 | Deploy settings to the system | not started |
| 11 | Delay alignment (subs/fills/delays/absolute) | not started |

Steps 1–6 are bookkeeping and interpolation, and are the safe part. Step 7 onward makes
claims about what a PA *should* do, and step 10 acts on them. The further down this table
a change is, the more it needs to be defensible.

---

## 1. Projects and shows

A **project** is a folder that groups shows. It owns almost nothing itself: a name, some
notes, and a trace library shared by everything inside it.

A **show** is a complete, independent configuration — engine config, transfer config,
generator settings, input device and sample rate, LEQ definitions, and tile layout.
Loading a show puts the application back exactly as it was left. Two shows in one project
may use different interfaces, different band resolutions and different tiles; nothing is
inherited from the project except access to its traces.

This is deliberately the opposite of the usual "project holds the config, sessions hold the
data" split. The reason is how the app is actually used: a project is a venue or a tour, and
within it "FOH system", "monitor world" and "broadcast feed" are different rigs measured
different ways, not variations on one setup.

### 1.1 What a show does *not* own: calibration

A show stores which device it used, and it stores a **snapshot** of the calibration that was
in force — but the live calibration used for measurement always comes from the existing
device-keyed table in `settings.json`.

This is a deliberate deviation from "a show owns everything", and it is a safety decision.
A calibration belongs to a microphone and a preamp, not to an application state — that is
why `Settings::calibrations` is keyed by device name today. If a show could carry its own
offset, opening last year's show with this year's microphone would present dB SPL computed
from an offset measured on hardware that is no longer in the room, and nothing downstream
could detect it. That is the same failure mode AGENTS.md §4.4a refuses for the synthetic
input, arriving by a different door.

The snapshot is still recorded, because a stored trace has to state the calibration it was
captured under to be worth anything later. It is used to *interpret stored traces*, never to
scale live audio.

**Invariant: a show never applies a calibration to live audio. It records the one it saw.**

### 1.2 On disk

A project is a directory, not a single file:

```
<projects root>/<Project Name>/
  project.json          manifest: id, name, created, notes, show index
  shows/<show-id>.json  one complete show configuration each
  traces/<trace-id>.json project-scoped trace library
```

with a global library alongside the existing settings:

```
<config dir>/
  settings.json         unchanged: device calibrations, last project, app preferences
  traces/<trace-id>.json global library — traces available to every project
```

A directory rather than a bundle because every write stays small and atomic. The existing
`Settings::save` writes the whole file on every change, which is fine for a few kilobytes of
layout; it is not fine once a project carries fifty transfer functions. One file per show and
one per trace means saving a tile drag rewrites a show, not a library.

Packaging a project for someone else is a zip, added when it is actually needed. It is not a
storage format.

### 1.3 `settings.json` keeps its job

The existing settings file is not replaced. It keeps device calibrations (§1.1), the app's
own preferences, and gains a pointer to the last open project and show so the app returns
where it was left. A first run with no project still works exactly as it does now — the
current settings become an implicit unsaved show, and "Save as project…" is what promotes
it. **The app must never require a project to measure.** Someone opening a meter to check a
level should not have to name a project first.

---

## 2. Traces — the load-bearing format

A trace is a stored measurement. It is *not* a screenshot of a tile, and this is the whole
design.

```rust
pub struct Trace {
    pub id: String,
    pub name: String,
    pub created: OffsetDateTime,
    pub notes: String,
    pub tags: Vec<String>,
    /// The axis, always stored explicitly — never implied by a config.
    pub frequencies: Vec<f32>,
    pub data: TraceData,
    pub capture: Option<CaptureMeta>,
}

pub enum TraceData {
    /// A dual-channel measurement. Magnitude AND phase AND coherence, always
    /// together — see §2.1.
    Transfer {
        magnitude_db: Vec<f32>,
        phase_deg: Vec<f32>,
        coherence: Vec<f32>,
        /// The delay that was compensated out of the reference when this was
        /// captured. Without it this trace has no common time base. §2.2.
        delay_ms: f64,
        delay_samples: u32,
        /// Averages behind the estimate. Coherence before four is meaningless.
        frames: u64,
    },
    /// A single-channel band measurement (RTA). Carries the band plan it came
    /// from, because band levels are only meaningful against the plan that
    /// produced them (AGENTS.md §4.9).
    Bands {
        levels_db: Vec<f32>,
        bands_per_octave: u32,
        calibrated: bool,
    },
    /// A target: magnitude, and optionally phase for minimum-phase work.
    /// No coherence — a target is not a measurement and must never be drawn as
    /// though it were.
    Curve {
        magnitude_db: Vec<f32>,
        phase_deg: Option<Vec<f32>>,
    },
}

pub struct CaptureMeta {
    /// Where the microphone was. Free text is fine; §8 needs to tell positions
    /// apart, not to know where they are.
    pub position: String,
    pub device: String,
    pub sample_rate: f64,
    pub calibrated: bool,
    pub calibration_offset_db: Option<f64>,
    pub transfer: Option<TransferConfig>,
    pub show_id: Option<String>,
    pub app_version: String,
}
```

Everything here already exists in the engine: `TransferReading` supplies magnitude, phase,
coherence, delay and frame count; `TransferPlan::frequencies` supplies the axis; `Frame`
supplies band levels and calibration state. **Step 2 is serialisation, not new DSP.** That is
the point of designing it now — the format is free today and expensive in six steps' time.

### 2.1 Why phase and coherence are not optional

A trace that stores only magnitude is enough to draw a pretty overlay and nothing else.

- Step 7 (EQ fitting) needs coherence, because fitting EQ to data below the coherence floor
  is fitting EQ to noise, and the result is confident nonsense.
- Step 11 (alignment) needs phase, because aligning two sources is a question about their
  complex sum. Magnitude alone cannot answer it at all.

So: **a `Transfer` trace stores magnitude, phase and coherence or it is not stored.** There
is no "magnitude-only export" of a measurement inside the library. CSV export (§3) may emit
fewer columns, because that is an interchange format for other tools; the library may not.

### 2.2 The common time reference trap

Two transfer functions can only be compared *in time* if both are expressed against the same
zero. AGENTS.md §4.7 says the reference is delay-compensated, never the measurement — which
means the phase in a stored trace already has some delay taken out of it, and the amount
differs from capture to capture.

Consequences, all of which must hold:

- Every `Transfer` trace stores `delay_ms` and `delay_samples`. A trace without them cannot
  participate in §11 and must be refused there rather than quietly mis-aligned.
- Alignment adds the stored compensation back before doing anything. Absolute arrival time
  is `delay_ms` plus whatever the phase slope says; neither half alone is the answer.
- Two traces captured at *different mic positions* have different propagation delays and that
  is real data, not error. Two traces captured at the *same* position in different app runs
  may differ by whatever the delay finder settled on, and that is error. The library cannot
  tell these apart on its own, which is why `CaptureMeta::position` exists.

### 2.3 Resampling between axes

Traces from different `points_per_octave`, or imported from another tool, land on different
frequency axes. Comparing or averaging them requires resampling, and there are two rules:

**Never interpolate decibels, and never interpolate wrapped phase.** Convert to a complex
value, interpolate the real and imaginary parts, convert back. Interpolating dB is
interpolating a logarithm and understates peaks; interpolating phase across a ±180° wrap
produces a sweep through every angle in between.

**Remove the bulk delay before interpolating, and put it back after.** A trace with 20 ms of
residual delay wraps phase every 50 Hz. At 1/12-octave spacing the points near 10 kHz are
hundreds of degrees apart and complex interpolation between them is meaningless — it aliases,
exactly like sampling a sine below Nyquist. Flattening the phase slope first makes the
residual smooth and the interpolation honest.

This is one pure function in `leqtion-dsp` and every later step calls it. It is worth its
tests.

### 2.4 Averaging across positions

Step 7 and step 11 both average across measurement positions, and they must average
*differently*.

- **For EQ decisions (§7): average power, not complex.** Vector-averaging several positions
  produces comb filtering from the arrival-time differences between them — nulls that exist
  in the average and at no actual seat. EQ applied to those nulls makes every seat worse.
  Power averaging discards the phase relationship deliberately, which is the right loss.
- **For alignment (§11): never average the inputs at all.** Alignment optimises a cost
  computed per position and summed. The thing being averaged is the *score*, not the data.

Both obey AGENTS.md §4.1 — the arithmetic happens in the energy domain. A mean of decibels
is still not a level.

---

## 3. Interchange — CSV and XML

Export exists so traces can leave, and import so they can arrive from Smaart, Open Sound
Meter, REW, EASE and the rest.

CSV is the pragmatic one: `frequency, magnitude_db, phase_deg, coherence`, one header row,
units in the header, `.` as the decimal separator regardless of locale. Import accepts a
subset — frequency plus magnitude is enough for a target curve, and a file without phase is
**imported as a `Curve`, never as a `Transfer`**, because promoting incomplete data to a
measurement is how §11 gets fed something that cannot support it.

XML is for the formats that have one, and each gets its own reader. There is no generic XML
trace format worth inventing; the value is in reading what other tools actually emit, so each
reader is written against real files and named after its source.

Round-tripping our own CSV must be lossless for the columns it carries, and there is a test
for that.

---

## 4–6. Target curves

A target is a `TraceData::Curve`. Three ways to get one:

**From a trace (§4).** Take a measurement's magnitude, drop coherence and phase, keep the
axis. The provenance stays in `notes` — a target derived from a measurement should be able to
say which one.

**By smoothing (§5).** Fractional-octave smoothing, in the power domain, width selectable
(1/1, 1/3, 1/6 octave). Smoothing a house curve to 1/3 octave is what makes it a *target*
rather than a copy of one position's comb filtering. Two properties matter: the smoother is
symmetric in log frequency, not in bins, and it does not shift energy — a flat input stays
flat and the band edges do not droop. Both are testable and both are commonly got wrong.

**By drawing (§6).** Control points with interpolation between them, plus tilt and shelf
primitives, because most house curves are described as "flat to 1 kHz then −1 dB/octave" and
typing that should not require placing forty points.

A target has no coherence and must never be drawn with the confidence styling a measurement
gets. On screen it is visually distinct from a measured trace, always.

---

## 7. The optimiser

Input: a live or stored measurement, a target, a frequency range, and a set of constraints.
Output: **suggested** filters, plus an honest statement of what they achieve.

The shape of it:

- Error is `measured − target` in dB, after resampling both to a common axis (§2.3) and
  smoothing the measurement to the resolution the target is expressed at. Fitting 1/48-octave
  detail with EQ is chasing seat-to-seat comb filtering.
- Every point is **weighted by coherence**, and points below the coherence floor are weighted
  to zero. This falls out of AGENTS.md §4.6 and it is the difference between an optimiser and
  a random filter generator.
- The fit runs over a user-set frequency range, defaulting to something well inside where the
  system has output. Nothing outside that range is corrected, ever.
- Gain is bounded and boost is bounded harder than cut, separately and by default. An
  optimiser that finds 12 dB of boost at 40 Hz has found the limit of the loudspeaker, not a
  filter setting.

**The optimiser proposes and the user disposes.** Its output is a filter set to review, with
the residual error shown before and after, never a change applied on its own. Between here
and §10 there is a real amplifier.

---

## 8. The processor database

A catalogue of what real system processors can actually do, because §9 fits *within* those
limits rather than fitting freely and rounding afterwards.

Starting scope: **d&b audiotechnik, Linea Research, Lake, L-Acoustics.**

An entry describes a processor's capability, per output channel: how many parametric bands,
which filter types, the frequency/Q/gain ranges and — importantly — the **step sizes**;
delay range and step; gain range and step; polarity; crossover options; FIR support and tap
count where it exists.

Two rules for this database:

**Every entry is sourced and dated, and unverified fields are marked as such.** A filter count
guessed from memory produces a recommendation that will not load into the device, discovered
by someone standing in front of a rig. The database carries a citation per model, and the
fitter refuses to constrain against a field marked unverified rather than assuming a value.

**Not every filter is a biquad.** Lake's Mesa and Raised Cosine filters, d&b's array
processing and CPL, and L-Acoustics' own filter set are not textbook parametrics, and
modelling them as such produces settings whose predicted response is wrong. Where a filter
type cannot be modelled honestly, the database says so and the fitter does not use it. A
smaller set of filters that behave as predicted beats a larger set that does not.

---

## 9. Fitting to the real processor

Fit under constraints; do not fit ideally and then round.

Quantising an ideal filter set to the device's grid afterwards is not the same answer — a
0.1 dB gain step and a 1/12-octave frequency grid interact with Q in ways that move the
result, and the error can exceed the correction on narrow filters. The fit searches the
representable set.

Output is the filter set plus the predicted response, and the predicted response is computed
from the *quantised* values. What is shown is what will be loaded.

---

## 10. Deployment

This is the one item on the list that can damage hardware or hearing. It is built last, and
it is built defensively:

- **Stage, review, then send.** No optimiser output reaches a device without an explicit
  confirmation step showing exactly what will change, per channel, old value → new value.
- **Read the device's current state first and keep it**, so there is something to go back to.
  A deployment that cannot be undone is not finished.
- **Never write while a show is running** without a deliberate, separate opt-in.
- Limits, protection settings and anything not being tuned are never written.

Protocol work partly exists in the fleet already: `db-remote` speaks AES70/OCA to d&b, and
that is the natural first target. Each manufacturer gets its own transport behind one trait,
and a manufacturer with no documented control protocol gets **file export** instead — a
preset the operator loads by hand. File export is the honest fallback and covers most of the
value.

---

## 11. Delay alignment

Four phases, all the same underlying problem:

1. **Subs to mains** — the hard one. Two sources with overlapping bandwidth in a crossover
   region, where the answer is the delay that makes them sum.
2. **Fills to mains**
3. **Delays to mains**
4. **Everything to an arbitrary zero** — e.g. the kick drum, so the whole system is referred
   to a point on stage rather than to the mains.

### 11.1 The maths

For each measurement position *p* you need both elements measured **at that position**:
`H_a,p` (the element being aligned) and `H_b,p` (the reference element). Applying delay τ and
gain *g* to element *a* gives a summed response

```
S_p(f, τ, g, σ) = H_b,p(f) + σ · g · H_a,p(f) · e^(−j2πfτ)
```

with σ = ±1 for polarity. The best fit maximises summed energy through the overlap region,
weighted by coherence and summed over positions:

```
score(τ, g, σ) = Σ_p Σ_f  w(f) · min(γ²_a, γ²_b) · |S_p(f, τ, g, σ)|²
```

`w(f)` restricts the fit to the crossover region — outside it, one element dominates and τ
barely moves the score, so including that range dilutes the answer with data that has no
opinion.

Both polarities are always evaluated and both reported. A best fit that is 0.5 dB better
with the polarity flipped is a result the user needs to see, not a decision to make silently.

### 11.2 Timing *and* phase

These are different questions and the tool answers both:

- **Arrival time** comes from the impulse response — where the energy lands. It is robust and
  it is what a delay finder gives you.
- **Phase alignment** is whether the two responses are in step *through the crossover*, which
  is the thing that determines whether they sum. Two sources can share an arrival time and
  still cancel, because the crossover filters and the drivers themselves have phase slopes.

The optimiser above solves for phase alignment; the impulse arrival is the starting point
that keeps the search near the right answer instead of a neighbouring wrap. Both are
reported, and where they disagree, that disagreement is shown — it is diagnostic, usually of
a crossover doing something unexpected.

### 11.3 Best fit across positions

One position gives one answer that is right at that seat. The score above is summed over
positions, so the result is the delay that does best across the ones supplied — with per-position
weighting, since the seat in the middle of the audience is worth more than the one at the
edge of coverage.

The result reports **how much the positions disagreed**. A tight cluster means a real answer;
a wide spread means the two elements do not have a single alignment that works everywhere,
which is a physical fact about the rig, not a failure of the fit. Presenting a mean without
its spread would be the tool's worst possible behaviour.

### 11.4 Haas offset

A user-set offset in milliseconds, applied *after* the fit, to bias imaging toward one source.
It is recorded separately from the solved delay and shown as such: the deployed number is
"solved 12.4 ms + Haas 10.0 ms = 22.4 ms". Folding an intentional offset into the solved
value loses the ability to re-solve without losing the artistic decision.

---

## 12. Where the code lives

The existing rule holds: **`leqtion-dsp` stays free of I/O, threads and clocks**, and the
frontend renders without calculating. That places everything as follows.

```
leqtion-dsp/
  trace.rs        Trace types, resampling, complex conversion (§2.3)
  smoothing.rs    fractional-octave smoothing (§5)
  target.rs       target curve construction and evaluation (§4–6)
  optimise.rs     the EQ fit (§7, §9) — pure, constraint-aware
  align.rs        the delay/polarity solver (§11) — pure
  filters.rs      biquad/filter models and their predicted responses

leqtion-devices/  NEW crate: the processor catalogue (§8) and, later, control
                  transports (§10). Capability *types* that optimise.rs must see
                  live in leqtion-dsp; the catalogue and the network live here.

src-tauri/src/
  project.rs      projects, shows, load/save (§1)
  library.rs      the trace library: storage, scopes, import/export (§2, §3)
```

Splitting the capability *types* from the *catalogue* is what keeps the DSP crate pure while
still letting the fitter respect a real device's limits. `optimise.rs` takes a
`ProcessorCapability` as an argument; it never looks one up.

---

## 13. Invariants this thread adds

Recorded here as they are decided, and mirrored into AGENTS.md as each lands.

1. **A show never applies a calibration to live audio.** It records the one it saw. Live
   calibration always comes from the device-keyed table. (§1.1)
2. **A stored transfer trace carries magnitude, phase and coherence, or it is not stored.**
   (§2.1)
3. **A trace without a stored delay compensation cannot be aligned**, and alignment refuses
   it rather than guessing. (§2.2)
4. **Resample in the complex domain, with the bulk delay removed first.** Never interpolate
   decibels or wrapped phase. (§2.3)
5. **EQ decisions average power across positions; alignment averages scores, never data.**
   (§2.4)
6. **Every fit is weighted by coherence**, and data below the floor is weighted to zero.
   (§7)
7. **A target curve is never drawn with the styling of a measurement.** (§4–6)
8. **Fit within the device's representable set; never fit ideally and round.** (§9)
9. **Nothing is written to a device without a staged diff and an explicit confirmation**,
   and the previous state is captured first. (§10)
10. **An alignment result is reported with the spread across positions**, never as a bare
    mean. (§11.3)
