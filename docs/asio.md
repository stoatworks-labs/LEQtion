# Building LEQtion with ASIO

ASIO is Windows-only and is **off by default**, because the build needs
Steinberg's ASIO SDK and that SDK cannot be redistributed. Nothing in this
repository contains any part of it, and nothing here downloads it for you.

A build without ASIO still lists it in the backend picker, marked unavailable
with the reason — an interface that has vanished from the list is a support
question every single time, so the absence is stated rather than implied.

## What you need

- Windows, and the MSVC toolchain (`rustup target add x86_64-pc-windows-msvc`).
- LLVM/Clang on `PATH`. `bindgen` generates the ASIO bindings and will not run
  without `libclang`.
- The **ASIO SDK**, from Steinberg's developer site. You accept their licence to
  download it; read it, because it governs what you may do with the result.

## Setting it up

Unpack the SDK somewhere permanent and point `CPAL_ASIO_DIR` at the directory
that contains `common/`, `host/` and `driver/`:

```powershell
$env:CPAL_ASIO_DIR = "C:\sdk\asiosdk_2.3.3_2019-06-14"
```

Then build with the feature on:

```powershell
npm run tauri build -- --features asio
```

The feature chains through `leqtion` → `leqtion-audio` → `cpal/asio`, so that
one flag is all of it.

## Checking it worked

```powershell
cargo run -p leqtion-audio --example capture -- --list
```

ASIO should appear as an available host with your interface under it. If it is
still listed as unavailable, the feature did not compile in; if it is available
but has no devices, the driver is not installed or is being held by another
application — ASIO drivers are usually exclusive, so close anything else using
the interface.

## Things that will surprise you

**One application at a time.** Most ASIO drivers give exclusive access. If a DAW
has the interface, LEQtion cannot open it, and the error comes back from the
driver as a generic failure rather than as "in use".

**The driver owns the buffer size and sample rate.** Both are set in the
interface's own control panel, not here. LEQtion asks for a sample rate and
takes what it is given, which is why the device bar always shows the rate the
stream actually opened at.

**Channel counts are large.** A 32-channel interface presents 32 input channels
and LEQtion will list all of them. The channel selector in the device bar is how
you pick the one the measurement microphone is on; getting this wrong measures
an input with nothing plugged into it, which reads as digital silence.

**A calibration belongs to a channel.** LEQtion stores calibrations per device,
not per channel, so moving the microphone to a different input on the same
interface leaves the old offset in place. Recalibrate after moving it.

## Testing without an ASIO interface

The Windows 11 VM on the build Mac is **ARM64**, so it runs the native aarch64
Windows toolchain — and ASIO drivers for ARM64 Windows are close to nonexistent.
ASIO support is therefore compiled and enumerated there, not exercised against
real hardware. That is the honest state of it: the code path exists and builds,
and no measurement has been taken through it.
