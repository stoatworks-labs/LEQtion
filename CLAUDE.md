# CLAUDE.md — LEQtion

Command reference. For the model, the invariants and the traps, read
[AGENTS.md](AGENTS.md) first.

## Commands

```bash
npm install
npm run app          # tauri dev
npm run app:build    # bundle (.dmg / .msi / .AppImage)
npm test             # vitest — 36 tests
npm run typecheck    # tsc -b
npm run lint         # oxlint
```

```bash
cd src-tauri && cargo test --workspace
```

```bash
cd src-tauri && cargo clippy --workspace --all-targets
```

## Verifying against real hardware

```bash
cd src-tauri && cargo run -p leqtion-audio --example capture -- --list
```

```bash
cd src-tauri && cargo run --example meter -- --seconds 30
```

`meter` is the whole measurement chain without the window. Use it in preference to
clicking through the GUI — it prints the same numbers and it is the only end-to-end
check that runs unattended.

## Windows with ASIO

```bash
npm run tauri build -- --features asio
```

Needs `CPAL_ASIO_DIR` and LLVM. See [docs/asio.md](docs/asio.md).

## Ground rules

- **Average mean squares, never decibels.** A mean of decibels is not a level.
- **LEQ is filtered in the time domain**, not weighted after an FFT.
- **Read the `weighting.rs` module docs before touching the filter design.** Plain
  bilinear is 13.6 dB out at 19 kHz. Pre-warping and an energy-weighted fit have both
  been tried, measured and rejected; the numbers are in the comments.
- Spectrum normalisation is `2/(N·S2)` and a full-scale sine reads **0 dBFS**.
- **Never present an uncalibrated level as dB SPL.** Every readout states its unit.
- The frontend renders; it does not calculate. No band tables or decibels in `src/`.
- **Never draw a transfer function without coherence.** The magnitude trace breaks below
  the floor; points fade in proportion. Coherence before four averages is meaningless.
- **The generator defaults to Off and is never restored on launch.** Level persists, the
  signal does not.
- **A history point is an interval, not a sample**, and downsampling keeps the extremes.
  A trace that gets flatter as you zoom out is a bug, not a rendering choice.
- **The log writes on the history's interval, never its own clock**, and every row states
  whether it is calibrated and how many frames have been dropped.
- **A show records a calibration but never applies one**, and loading a show never starts a
  generator. **Nothing requires a project** — the app still meters with none open.
- **Deleting a project or show moves it to `.deleted/`**, and the UI says where it went.
- Public repo, MIT. "Commit" = commit **and** push.

## The tuning thread

`docs/tuning.md` is the design for the arc from projects → trace library → target curves →
optimiser → processor database → deployment → delay alignment. **Read it before starting any
of those steps**; §2 (the trace format) and §13 (the invariants) are the parts later work
depends on. Step 1, projects and shows, is built; everything after it is not.
