//! Check that an input actually delivers audio.
//!
//! ```sh
//! cargo run -p leqtion-audio --example capture              # default input
//! cargo run -p leqtion-audio --example capture -- --list
//! cargo run -p leqtion-audio --example capture -- --device "Scarlett 2i2" --seconds 5
//! ```
//!
//! This exists because "the meter is not moving" has several very different
//! causes — no permission, the wrong device, a device that opens but never
//! calls back, an input with nothing plugged into it — and they are hard to tell
//! apart from inside a GUI. Here they are distinguishable: the tool reports
//! whether the stream opened, whether samples arrived, and what their level was.
//!
//! A run that reports frames arriving at exactly digital silence is the
//! signature of macOS denying microphone access: the stream opens and the
//! callback fires, but every sample is zero.

use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", USAGE);
        return;
    }

    if args.iter().any(|a| a == "--list") {
        list();
        return;
    }

    let device = flag(&args, "--device");
    let host = flag(&args, "--host");
    let seconds: f64 = flag(&args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3.0);

    let options = leqtion_audio::CaptureOptions {
        host: host.clone(),
        device: device.clone(),
        ..Default::default()
    };

    let (capture, mut consumer) = match leqtion_audio::open(options) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("could not open the input: {e}");
            std::process::exit(1);
        }
    };

    let info = capture.info.clone();
    println!(
        "opened {} on {} — {} ch, {} Hz, {}",
        info.device, info.host, info.channels, info.sample_rate, info.sample_format
    );
    println!("listening for {seconds:.1} s…");

    let start = Instant::now();
    let mut frames: u64 = 0;
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;

    while start.elapsed().as_secs_f64() < seconds {
        let available = consumer.slots();
        if available == 0 {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        if let Ok(chunk) = consumer.read_chunk(available) {
            let (a, b) = chunk.as_slices();
            for &x in a.iter().chain(b.iter()) {
                peak = peak.max(x.abs());
                sum_sq += (x as f64) * (x as f64);
            }
            frames += (a.len() + b.len()) as u64;
            chunk.commit_all();
        }
    }

    let dropped = capture
        .stats
        .dropped_frames
        .load(std::sync::atomic::Ordering::Relaxed);
    let errors = capture
        .stats
        .stream_errors
        .load(std::sync::atomic::Ordering::Relaxed);
    capture.stop();

    let expected = (seconds * info.sample_rate as f64 * info.channels as f64) as u64;
    println!();
    println!("samples   {frames} (expected about {expected})");
    println!("dropped   {dropped}");
    println!("errors    {errors}");

    if frames == 0 {
        println!();
        println!("NOTHING ARRIVED. The stream opened but the driver never called back.");
        std::process::exit(2);
    }

    let rms = (sum_sq / frames as f64).sqrt();
    let peak_db = if peak > 0.0 {
        20.0 * (peak as f64).log10()
    } else {
        f64::NEG_INFINITY
    };
    let rms_db = if rms > 0.0 {
        20.0 * rms.log10()
    } else {
        f64::NEG_INFINITY
    };

    println!("peak      {peak_db:.1} dBFS");
    println!("rms       {rms_db:.1} dBFS");
    println!();

    if peak == 0.0 {
        println!("Every sample was exactly zero.");
        println!("On macOS that usually means microphone access was denied — check");
        println!("System Settings → Privacy & Security → Microphone. It can also mean");
        println!("the selected channel has nothing connected to it.");
        std::process::exit(3);
    }

    // A tenth of the expected frames is a stream that is running but starved.
    if frames * 10 < expected {
        println!("Far fewer samples than expected — the device is not keeping up.");
        std::process::exit(4);
    }

    println!("Audio is arriving.");
}

const USAGE: &str = "\
capture — check that an audio input delivers samples

  --list             list hosts and input devices
  --host NAME        audio API to use (Core Audio, WASAPI, ALSA, JACK, ASIO)
  --device NAME      input device; default input if omitted
  --seconds N        how long to listen (default 3)
";

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn list() {
    for host in leqtion_audio::hosts() {
        let mark = if host.is_default { " (default)" } else { "" };
        if !host.available {
            println!(
                "{}{mark} — unavailable: {}",
                host.name,
                host.note.as_deref().unwrap_or("no reason given")
            );
            continue;
        }
        println!("{}{mark}", host.name);
        match leqtion_audio::devices(Some(&host.id)) {
            Ok(devices) => {
                for d in devices {
                    println!(
                        "    {}{}  — {} ch, {:?} Hz",
                        d.name,
                        if d.is_default { " (default)" } else { "" },
                        d.max_channels,
                        d.sample_rates
                    );
                }
            }
            Err(e) => println!("    {e}"),
        }
    }
}
