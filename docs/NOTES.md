# Notes

Working notes for this repo: status, decisions, and the traps that have actually bitten.
Migrated out of Claude Code's memory on 2026-08-24, so they are written in the first
person and dated by when each thing was learned — that date is usually the useful part.

Cross-cutting notes that are not specific to this repo live in
[fleet-notes](https://github.com/stoatworks-labs/fleet-notes).

## leqtion

*LEQtion — desktop sound level meter (Tauri v2 + Rust), RTA/spectrograph/SPL/user-defined LEQs on configurable tiles. PUBLIC MIT, v0.1.0 alpha*

**LEQtion** — desktop sound level meter and real-time analyser. Started 2026-08-01.
`~/Projects/LEQtion`, **PUBLIC MIT** at https://github.com/stoatworks-labs/LEQtion.

Tauri v2 shell + React tile dashboard over a Rust measurement core. Grew out of
[simplerta](https://github.com/stoatworks-labs/simpleRTA/blob/main/docs/NOTES.md) (`simpleRTA`): same fractional-octave RTA idea, but native, with SPL, LEQ and
calibration on top. simpleRTA stays the browser one — **do not merge them**, the user
chose to keep both.

**Layout:** `src-tauri/crates/leqtion-dsp` (pure DSP, no I/O, 117 tests — the important
crate), `crates/leqtion-audio` (cpal in + out, lock-free ring), `crates/diag` (vendored),
`src/` React frontend that renders and never calculates. 134 Rust + 23 frontend tests.

**Tiles:** RTA, spectrograph, bargraph, SPL, LEQ, **generator**, **transfer function**.
Drag to move/resize, layout persisted. LEQs are user-defined: any window length + A/C/Z
weighting, and they live in the engine so they keep integrating whether or not a tile
shows them.

**Analyser half (added 2026-08-01):** signal generator (pink/white/sine/sweep, dBFS RMS
with expected-peak warning, ramped so it can't click) and a **multi-time-window transfer
function** — H1 from complex-averaged cross-spectra, magnitude + phase + coherence, delay
found from the impulse response of H. Reference is the generator tapped internally or a
hardware loopback. See [transfer function traps](https://github.com/stoatworks-labs/fleet-notes/blob/main/notes/reference_transfer_function_traps.md).

**`leqtion-audio::synthetic` offers the generator as an INPUT** — a "Signal generator"
backend with one device per signal, feeding the ring directly, so the whole chain runs
with no interface, no microphone and no microphone permission. **Calibration is refused
while it is open, and must stay refused** (AGENTS.md §4.4a): a synthetic 1 kHz sine
passes all four gates and would write a dB SPL offset invented from nothing into
settings.json. The engine cannot catch that — from inside the analysis, a sine on a wire
and a calibrator on a capsule are identical. Only the source knows.

**Backends:** Core Audio / WASAPI / ALSA / JACK work; **ASIO compiles but has never
carried audio** (feature-gated, needs the non-redistributable Steinberg SDK, and the only
Windows box here is ARM64 where ASIO drivers barely exist). See `docs/asio.md`.

**Verified:** DSP against synthetic signals; a **real audio interface** over Core Audio end-to-end via
`cargo run --example meter`. **Not verified:** against a reference meter, and no hardware
calibrator has ever been connected. Not a certified sound level meter — the README says so.

**Two CLI diagnostics worth remembering** (still the fastest answer when the question is
about numbers): `cargo run -p leqtion-audio --example capture -- --list` and
`cargo run --example meter -- --seconds 30`.

**The GUI *can* be automated here** — an earlier note in this file and in AGENTS.md said
otherwise and was wrong. Coordinate clicking fails with AppleScript −25204, but the
webview publishes its controls by name to the accessibility tree, so `click button
"Start" of group 1 of UI element 1 of scroll area 1 of …` works. See
**screenshot capture** (working-practice note, kept in Claude memory).

**On the website since 2026-08-01**, with a hero screenshot of it measuring its own
generator (pink noise at −20 dBFS, RTA flat, `docs/screenshots/measuring-pink-noise.png`),
and a **video: `Qe8D6juRCU4`**, 45.8s, filmed the same day on the synthetic input with no
hardware in the chain. Instagram reel queued for 16 Aug. See [project videos](https://github.com/stoatworks-labs/fleet-notes/blob/main/notes/project_project_videos.md).

**v0.1.0 released 2026-08-01** — macOS aarch64 + x86_64 only, ad-hoc signed, not
notarised. `scripts/release-local.sh` is the **first Tauri-only caller in the fleet**:
`release-rust.sh` assumes a Rust server binary with a launcher beside it, and LEQtion
has no server, so this one drives `tauri build` directly and uses `release-lib.sh` only
for staging and `rl_adhoc_sign`. No Windows or Linux build: Tauri cannot cross-bundle,
the Parallels VM is ARM64 (an arm64-only Windows installer is worse than none), and
there is no Linux host for webkit2gtk. Missing *builds*, not missing support.

**Level history + CSV logging landed in v0.1.0** (`leqtion-dsp/history.rs`,
`src-tauri/src/logger.rs`). A history point is an *interval* — min/mean/max, energy
mean — not a sample; downsampling happens in `History::view` and keeps the extremes.
The log writes on the history's interval, never its own clock, and every row carries
`calibrated` and cumulative `dropped_frames`.

**A second thread of work started 2026-08-11 — see **leqtion tuning** (below)**: projects
and shows, a trace library, target curves, an EQ optimiser, a processor database and delay
alignment, ending in exported/deployed DSP settings. Designed in `docs/tuning.md`; step 1
(projects/shows) is built. Read that doc before touching any of it.

**The video (`Qe8D6juRCU4`) predates both features** — it shows neither the chart nor
logging, and its end card carries no version. See **release workflow** (working-practice note, kept in Claude memory).

## leqtion next prerelease line

*LEQtion's beta line — branch `next` = 'LEQtion NEXT', published as GitHub pre-releases; the version string alone drives tag, prerelease flag and the in-app banner*

**leqtion tuning** (below) ships from a branch called **`next`**, badged **LEQtion NEXT**.
`main` stays on the released line — as of 2026-08-11, main is v0.1.1 and `next` is
v0.2.0-beta.1, and the beta commits are **not** ancestors of main.

**The version string is the only switch.** Anything carrying a semver pre-release
identifier (`0.2.0-beta.1`) causes `scripts/release-local.sh` to tag it, title it
"LEQtion NEXT vX (pre-release)" and pass `--prerelease`; the app reads the same string
back out of the binary at startup (`src/lib/version.ts` → `isPrerelease`) and shows a
permanent, non-dismissible banner. **Do not add a beta flag** — a flag is something to
forget, and a beta warning missing from a beta is the failure that matters.

`--prerelease` is what actually protects users: it keeps the build out of "Latest
release" and out of `/releases/latest`, which is what stops an accidental download.
Release notes are read *after* the download, not before.

**Pre-releases stay off the website and out of the README download block.** Those
advertise the current release; the generated block on `next` still points at the stable
version and that is correct — a branch banner above it explains.

**Installing both lines side by side:** stable goes to `/Applications/LEQtion.app`, the
beta to `/Applications/LEQtion NEXT.app` (both DMGs contain a bundle literally named
`LEQtion.app`, so the beta must be renamed on install). They share the bundle id
`com.allansargeant.leqtion`, so LaunchServices may resolve either — the in-app NEXT banner
is the only reliable way to tell which one is running. Established 2026-08-13, after three
`LEQtion*.app` copies in /Applications turned out to be ad-hoc local builds all stamped
`0.2.0-beta.1` with three *different* executable hashes, none matching the released beta;
see [installed app version audit](https://github.com/stoatworks-labs/fleet-notes/blob/main/notes/reference_installed_app_version_audit.md).

Traps confirmed 2026-08-11:

- **Tauri and Apple both accept `0.2.0-beta.1`.** It goes into
  `CFBundleShortVersionString` verbatim, bundles, Developer ID-signs, notarises and
  staples; `spctl` reports "Notarized Developer ID". The worry that Apple requires three
  numeric components does not apply here.
- `gh release create` on a tag that already exists reports `targetCommitish: main`
  regardless — **that is cosmetic**. Check `git rev-list -n1 <tag>` to see where the tag
  really points.
- `gh release view --json` has **no `isLatest` field**. Use
  `gh api repos/OWNER/REPO/releases/latest --jq .tag_name` instead.
- `release-local.sh --upload` re-runs the *entire* build before uploading, so it is not
  a cheap way to publish artefacts that were already built.

## Tauri 2 NSIS DOES accept a semver pre-release version (2026-08-22)

The open question when cutting [livepremier plus](https://github.com/stoatworks-labs/livepremier-plus/blob/main/docs/NOTES.md) (`livepremier-plus`) v0.4.0-preview.1:
LEQtion is the fleet's only Tauri app carrying a pre-release version
(`0.2.0-beta.1`) and it ships **macOS only**, so nothing proved the Windows
path. Tauri's docs are strict about Windows needing a numeric version, and MSI
genuinely is — but **NSIS is not**.

Confirmed on Tauri **2.11.5** with `bundle.targets` of
`app, dmg, nsis, deb, rpm`: `"version": "0.4.0-preview.1"` built
`LivePremier.Plus_0.4.0-preview.1_x64-setup.exe` with no complaint, alongside
both macOS arches. So the house convention — the semver identifier is the only
switch — carries to repos that ship Windows too, as long as the target is NSIS
rather than MSI/WiX.

**Bump every manifest, not just the root**: `package.json`,
`launcher/package.json`, `launcher/src-tauri/Cargo.toml` and
`launcher/src-tauri/tauri.conf.json` — plus `cargo metadata` to refresh
`Cargo.lock`. See [fleet mass release traps](https://github.com/stoatworks-labs/fleet-notes/blob/main/notes/reference_fleet_mass_release_traps.md).

## leqtion tuning

*LEQtion's system-tuning thread — 11-step arc from projects/shows to DSP deployment and delay alignment. Designed in docs/tuning.md; step 1 built 2026-08-11.*

A second thread of work in **leqtion** (below), started **2026-08-11**: turning the
analyser into something that produces **DSP settings for amps/processors/crossovers**.
Ordered by the user: projects containing shows → trace library → CSV/XML interchange →
trace as target curve → smoothing → draw a target → optimiser (EQ to target) →
amp/DSP database (d&b, Linea Research, Lake, L-Acoustics) → fit to the *real* processor →
deploy to the system → delay alignment (subs/fills/delays/absolute zero, multi-position
best fit, Haas offset, timing *and* phase).

**The design is written down: `docs/tuning.md`.** Read it before touching any step — it
carries the decisions later steps depend on and a numbered invariant list (§13). The
load-bearing one is §2, the trace format: a stored transfer trace keeps **magnitude,
phase, coherence and its delay compensation** or it is not stored, because steps 7 and 11
cannot work from magnitude alone and retrofitting the library later is the expensive
mistake. `TransferReading` + `TransferPlan::frequencies` already supply all of it —
**step 2 is serialisation, not new DSP.**

Other decisions worth not re-deriving:

- **Resample traces in the complex domain with the bulk delay removed first.** Never
  interpolate dB, never interpolate wrapped phase — at 1/12 octave near 10 kHz a 20 ms
  residual wraps faster than the point spacing and interpolation aliases.
- **EQ decisions power-average across positions; alignment averages the *score*, never
  the data.** Vector-averaging positions invents comb nulls that exist at no seat.
- **Fit within the processor's representable set — never fit ideally and round.** And
  every database entry is sourced and dated; unverified fields are marked, and the fitter
  refuses to constrain against them rather than guessing a filter count.
- **Step 10 (deploy) is built last, staged-diff-then-confirm, previous state captured
  first.** It is the only item that can damage hardware or hearing. `db-remote`'s
  AES70/OCA work is the natural first transport; manufacturers without a documented
  protocol get file export instead.
- An alignment result is reported **with the spread across positions**, never a bare mean.

**Step 1 is built** (`src-tauri/src/project.rs`, `src/ui/ProjectBar.tsx`): project = a
directory grouping shows, show = a *complete independent config* (engine, transfer,
generator, device, rate, LEQs, layout). The user chose that split deliberately — project
is "just a folder". Two invariants came out of it and are in AGENTS.md §4.10–4.12: a show
**records** a calibration and can never apply one (`ShowRestore` has no field for it),
loading a show never starts a generator, and **nothing requires a project**.

**Ships from the `next` branch as GitHub pre-releases** — see
**leqtion next prerelease line** (below). v0.2.0-beta.1 published 2026-08-11, notarised,
with main still on v0.1.1 as Latest.

Not verified: the Tauri IPC round trip has only been checked statically (command names
and args cross-checked both ways) plus a screenshot of the bar rendering. No project has
been created through the GUI.
