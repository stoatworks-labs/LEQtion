# Attributions

LEQtion is built on other people's work. This file lists what that work is, who did
it, and what it is doing here.

It is generated — the master lists live in the `stoatworks-backend` repo and are
pushed out by `scripts/sync-attributions.py`. Edit it there, not here.

## Code we derived from other people's work

Someone else solved this first, and this project would not exist in its current form without their work.

### libebur128 — Jan Kokemüller

<https://github.com/jiixyj/libebur128>  
Licence: MIT

BS.1770 tabulates the K-weighting filter at 48 kHz only. The loudness meter in src-tauri/crates/leqtion-dsp/src/loudness.rs builds it at the actual sample rate instead, from the analogue parameters of each stage — a shelf at 1681.97 Hz, +3.9998 dB, Q 0.7072, and an RLB high-pass at 38.135 Hz, Q 0.5003 — bilinear-transformed. Those parameters, to full precision, and that derivation are libebur128's. No code was copied, and LEQtion adds one correction libebur128 does not make: the high-pass numerator is scaled so its gain above the corner matches the 48 kHz table at every rate.

## Third-party code this project uses

Libraries, SDKs and frameworks the project is built on or bundles.

### Tauri

<https://tauri.app>  
Licence: MIT or Apache-2.0  
Copyright: The Tauri Programme within The Commons Conservancy

A Cargo and npm dependency — of the app itself under src-tauri/, or of the desktop launcher under launcher/src-tauri/.

Wraps a web front end in a native desktop app using the platform's own webview rather than a bundled browser, so the binary stays small.

### React

<https://react.dev>  
Licence: MIT  
Copyright: Meta Platforms, Inc. and affiliates

An npm dependency.

The UI layer for the browser tools and the Electron and Tauri front ends.

### The Rust crate ecosystem

<https://crates.io>  
Licence: predominantly MIT or Apache-2.0  
Copyright: the individual crate authors

Cargo dependencies, resolved and pinned in Cargo.lock.

Async runtimes, protocol codecs, serialisation and GUI toolkits. The exact set and versions for any build are in that repo's Cargo.lock, which is the authoritative list.

### The npm ecosystem

<https://www.npmjs.com>  
Licence: predominantly MIT  
Copyright: the individual package authors

npm dependencies, resolved and pinned in the lockfile.

Build tooling, test runners and the libraries the front ends are assembled from. The exact set and versions for any build are in that repo's lockfile, which is the authoritative list.

The full transitive dependency set for any build is pinned in this repo's lockfile,
which is the authoritative list. What is named above is the layers a reader would
want to know about, not every package that has ever been resolved.

## Standards and published specifications

What the implementation is measured against.

- **IEC 61672 sound level meter specification** — Weightings, time constants and the LEQ integration the meter implements.
- **ITU-R BS.1770-5** — Programme loudness: the K-weighting, the gating behind integrated loudness, and the true-peak meter, whose Annex 2 interpolating filter is used as published.
- **EBU R 128 and EBU Tech 3341** — How the loudness meter behaves: momentary and short-term windows and their maxima. Its tests rebuild the conformance cases from their descriptions.
- **EBU Tech 3342** — Loudness range, and the four cases it is tested against.

## Getting this wrong

If your work is here and the description is inaccurate, the licence is wrong, or you would rather not be listed — open an issue and it will be fixed.
