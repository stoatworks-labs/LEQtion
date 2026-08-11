## LEQtion NEXT v0.2.0-beta.1 — pre-release

> ### ⚠️ This is not the current release
>
> **The current release is [v0.1.1](https://github.com/stoatworks-labs/LEQtion/releases/latest).
> Use that one unless you specifically want to try the new work.**
>
> This build comes from the `next` branch and carries the first step of a longer
> thread of work that is not finished. It is published so the new features can be
> tried and argued with, not because they are done.
>
> - **Anything you have to defend, measure on the stable release.** The measurement
>   core is unchanged from v0.1.1 and every DSP test still passes — but a pre-release
>   is the wrong thing to have been running when someone asks where a number came
>   from. The app shows a permanent banner saying so while it is open.
> - **Projects and shows saved here may not open in later builds.** The on-disk format
>   is new and will change as the trace library lands. Do not put anything in it you
>   cannot recreate.
> - Everything below step 1 of the plan is designed but **not built**.

---

### What is new

**Projects and shows.** A *project* is a folder that groups shows; a *show* is a
complete, independent configuration — engine settings, transfer function settings,
generator, input device and sample rate, LEQ definitions and tile layout — saved under
a name. Loading a show puts the whole application back as it was.

Projects live in `Documents/LEQtion/`, one directory per project, one file per show.
They are meant to be findable, backed up and handed to a colleague.

Three things it deliberately will not do:

- **A show records a calibration but never applies one.** Live calibration keeps coming
  from the per-device table, because a calibration belongs to a microphone and a preamp
  rather than to an application state. Opening last year's show with this year's
  microphone must not present dB SPL derived from hardware that is no longer in the
  room.
- **Loading a show never starts the generator.** The level and band-limiting come back;
  the signal does not. A show can be loaded with an output stream already open and a
  system live, which makes this matter more here than it does at launch.
- **Nothing requires a project.** The app meters, logs and calibrates with none open,
  exactly as it always has.

Deleting a project or a show **moves it to a `.deleted/` folder** and tells you where it
went. Nothing is erased.

### What this is a step towards

`docs/tuning.md` designs the whole arc: a trace library, CSV/XML interchange, target
curves, smoothing, an EQ optimiser, a database of real amplifiers and system processors
(d&b, Linea Research, Lake, L-Acoustics), fitting within a given processor's actual
limits, deployment, and delay alignment for subs, fills and delays with multi-position
best fit and a Haas offset.

Only the container is built. The document is worth reading before forming an opinion
about the rest — in particular the trace format, which is the decision every later step
depends on.

### Also in this build

- A permanent pre-release banner, driven by the version string itself rather than by a
  flag someone has to remember to turn off.
- `scripts/release-local.sh` publishes any version carrying a semver pre-release
  identifier as a GitHub pre-release automatically.

### Unchanged

The measurement core. No DSP, weighting, LEQ, spectrum or transfer-function code was
touched — 133 DSP tests, 168 Rust tests and 40 frontend tests all pass.

### Builds

macOS only, arm64 and x86_64, Developer ID-signed and notarised as usual. Tauri cannot
cross-bundle, so Windows and Linux binaries still need hosts that are not available
here — an absence of builds, not of support.

### Known gaps

- The projects and shows UI has been exercised by unit tests on both sides of the IPC
  boundary and confirmed to render in the real app, but **no project has been created
  by clicking through a running build**. That is the first thing to try, and the first
  thing to report.
- LEQtion remains **not a certified sound level meter**, has never been checked against
  a reference meter, and no hardware calibrator has been connected to it.
