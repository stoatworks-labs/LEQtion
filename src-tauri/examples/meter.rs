//! A sound level meter on the command line.
//!
//! ```sh
//! cargo run --example meter                          # default input, 10 s
//! cargo run --example meter -- --device "Scarlett 2i2" --seconds 30 --offset 120
//! ```
//!
//! Every part of LEQtion except the window: it opens a real input through
//! `leqtion-audio`, drives the real `Engine`, and prints the levels it produces.
//!
//! It exists for two reasons. It is the end-to-end check that the driver, the
//! ring, the analysis thread and the DSP agree — the GUI tests cover the DSP
//! against synthetic signals, and this covers the join to real hardware. And it
//! is the thing to reach for when a measurement looks wrong on site, because it
//! prints the same numbers with none of the display in the way.
//!
//! `--offset` applies a calibration by hand, for when you know the figure
//! already: the number is the SPL that full scale corresponds to, which is what
//! LEQtion shows next to a calibration.
//!
//! The routing flags put each meter on its own input, as the Inputs panel
//! does, counted from 1 as the app counts them:
//!
//! ```sh
//! cargo run --example meter -- --device "Pro Tools Audio Bridge 16" \
//!     --levels 1 --compare 3 --stereo 3,4 --loudness 3,4
//! ```

use std::time::{Duration, Instant};

use leqtion_dsp::bands::Fraction;
use leqtion_dsp::calibration::{Calibration, CalibrationTarget};
use leqtion_dsp::engine::{ChannelSelect, Engine, EngineConfig};
use leqtion_dsp::stereo::StereoPair;
use leqtion_dsp::leq::{LeqSpec, LeqWindow};
use leqtion_dsp::weighting::Weighting;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }

    let seconds: f64 = flag(&args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    let offset: Option<f64> = flag(&args, "--offset").and_then(|s| s.parse().ok());

    let options = leqtion_audio::CaptureOptions {
        host: flag(&args, "--host"),
        device: flag(&args, "--device"),
        sample_rate: flag(&args, "--rate").and_then(|s| s.parse().ok()),
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
    let channels = info.channels as usize;

    let mut config = EngineConfig::default();
    config.spectrum.fraction = Fraction::Third;
    // Inputs on the command line count from 1, as they do in the app.
    let input = |name: &str| -> Option<ChannelSelect> {
        let v = flag(&args, name)?;
        if v == "mix" {
            return Some(ChannelSelect::Mix);
        }
        let n: usize = v.parse().ok()?;
        Some(ChannelSelect::Channel {
            index: n.saturating_sub(1),
        })
    };
    let inputs = |name: &str| -> Option<Vec<usize>> {
        let v = flag(&args, name)?;
        let list: Vec<usize> = v
            .split(',')
            .filter_map(|n| n.trim().parse::<usize>().ok())
            .map(|n| n.saturating_sub(1))
            .collect();
        (!list.is_empty()).then_some(list)
    };
    if let Some(levels) = input("--levels") {
        config.channel = levels;
    }
    config.routing.spectrum = input("--rta");
    config.routing.compare = input("--compare");
    if let Some(pair) = inputs("--stereo") {
        if let [left, right] = pair[..] {
            config.routing.stereo = StereoPair { left, right };
        }
    }
    if let Some(list) = inputs("--loudness") {
        config.routing.loudness = list;
    }
    config.leqs = vec![
        LeqSpec {
            id: "short".into(),
            label: String::new(),
            weighting: Weighting::A,
            window: LeqWindow::Sliding { seconds: 1.0 },
        },
        LeqSpec {
            id: "whole".into(),
            label: String::new(),
            weighting: Weighting::A,
            window: LeqWindow::Elapsed,
        },
    ];

    let mut engine = Engine::new(config, info.sample_rate as f64);
    if let Some(full_scale) = offset {
        // `Calibration::new` derives the offset from a measurement, so this
        // constructs the equivalent run: reading `-full_scale` dBFS when the
        // source is 94 dB puts full scale exactly at `full_scale` dB SPL.
        engine.set_calibration(Some(Calibration::new(
            CalibrationTarget {
                level_db: 94.0,
                frequency_hz: 1000.0,
            },
            94.0 - full_scale,
        )));
    }

    let unit = if offset.is_some() { "dB SPL" } else { "dBFS" };
    println!(
        "{} on {} — {} ch, {} Hz",
        info.device, info.host, channels, info.sample_rate
    );
    println!("levels in {unit}, loudness in LUFS\n");
    println!("   time   LAF     LCF     LZF    LAeq,1s   LAeq    peak   bands   corr    M      S      I    TP");

    let start = Instant::now();
    let mut last_print = Instant::now();
    let mut buf = vec![0.0f32; 8192];

    while start.elapsed().as_secs_f64() < seconds {
        let available = consumer.slots();
        if available == 0 {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        let want = available.min(buf.len()) / channels * channels;
        if want == 0 {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        if let Ok(chunk) = consumer.read_chunk(want) {
            let (a, b) = chunk.as_slices();
            buf[..a.len()].copy_from_slice(a);
            buf[a.len()..a.len() + b.len()].copy_from_slice(b);
            let n = a.len() + b.len();
            chunk.commit_all();
            engine.push_interleaved(&buf[..n], channels);
        }

        if last_print.elapsed() >= Duration::from_millis(500) {
            last_print = Instant::now();
            let f = engine.frame();
            let level = |w: Weighting| {
                f.spl
                    .iter()
                    .find(|s| s.weighting == w)
                    .map(|s| s.level)
                    .unwrap_or(f64::NAN)
            };
            let leq = |id: &str| {
                f.leqs
                    .iter()
                    .find(|l| l.id == id)
                    .map(|l| l.value)
                    .unwrap_or(f64::NAN)
            };
            // How many bands are above the floor — a quick way to see that the
            // spectrum is populated rather than a single spike or nothing.
            let live = f.bands_db.iter().filter(|&&v| v > -90.0).count();
            let lufs = |v: Option<f64>| v.map_or("   --".to_string(), |v| format!("{v:6.1}"));
            let corr = f
                .stereo
                .correlation
                .map_or("  -- ".to_string(), |r| format!("{r:+5.2}"));
            println!(
                "  {:5.1}s {:7.1} {:7.1} {:7.1} {:8.1} {:7.1} {:7.1}  {live:>4}/{}  {corr} {} {} {} {}",
                start.elapsed().as_secs_f64(),
                level(Weighting::A),
                level(Weighting::C),
                level(Weighting::Z),
                leq("short"),
                leq("whole"),
                f.spl
                    .iter()
                    .find(|s| s.weighting == Weighting::Z)
                    .map(|s| s.peak)
                    .unwrap_or(f64::NAN),
                f.bands_db.len(),
                lufs(f.loudness.momentary),
                lufs(f.loudness.short_term),
                lufs(f.loudness.integrated),
                lufs(f.loudness.true_peak),
            );
        }
    }

    let dropped = capture
        .stats
        .dropped_frames
        .load(std::sync::atomic::Ordering::Relaxed);
    capture.stop();

    let f = engine.frame();
    println!();
    println!("measured for {:.1} s, {dropped} frames dropped", f.elapsed_seconds);
    for l in &f.leqs {
        println!("  {:<12} {:7.1} {unit}", l.label, l.value);
    }

    let name = |c: ChannelSelect| match c {
        ChannelSelect::Channel { index } => format!("input {}", index + 1),
        ChannelSelect::Mix => "mix".to_string(),
    };
    let i = &f.inputs;
    println!();
    println!("levels on {}, RTA on {}", name(i.levels), name(i.spectrum));
    if let (Some(c), Some(trace)) = (i.compare, &f.compare) {
        // The comparison trace's own loudest band, so a routing mistake shows
        // as the wrong frequency rather than needing the window to spot.
        let (band, level) = trace
            .bands_db
            .iter()
            .enumerate()
            .fold((0, f32::MIN), |m, (n, &v)| if v > m.1 { (n, v) } else { m });
        println!(
            "comparison on {}: loudest band {} at {level:.1} {}",
            name(c),
            engine.plan().bands[band].label,
            if trace.calibrated { "dB SPL" } else { "dBFS" }
        );
    }
    let st = &f.stereo;
    println!(
        "vectorscope on inputs {} and {}: correlation {} (min {}), balance {}",
        st.left + 1,
        st.right + 1,
        st.correlation.map_or("--".into(), |r| format!("{r:+.3}")),
        st.correlation_min.map_or("--".into(), |r| format!("{r:+.3}")),
        st.balance_db.map_or("--".into(), |b| format!("{b:+.2} dB")),
    );
    let lo = &f.loudness;
    let opt = |v: Option<f64>| v.map_or("--".to_string(), |v| format!("{v:.1}"));
    println!(
        "loudness on inputs {:?}: I {} LUFS, LRA {} LU, max M {}, max S {}, TP {} dBTP",
        i.loudness.iter().map(|c| c + 1).collect::<Vec<_>>(),
        opt(lo.integrated),
        opt(lo.range),
        opt(lo.max_momentary),
        opt(lo.max_short_term),
        opt(lo.true_peak),
    );
    if dropped > 0 {
        println!();
        println!("Frames were dropped, so the LEQ above is short by an unknown amount.");
        std::process::exit(4);
    }
}

const USAGE: &str = "\
meter — LEQtion's measurement chain, without the window

  --host NAME        audio API (Core Audio, WASAPI, ALSA, JACK, ASIO)
  --device NAME      input device; default input if omitted
  --rate N           sample rate
  --seconds N        how long to measure (default 10)
  --offset N         SPL that full scale corresponds to, to read in dB SPL
  --levels N|mix     input the levels read (default 1)
  --rta N|mix        input the RTA reads (default: the levels input)
  --compare N|mix    input for the RTA's comparison trace
  --stereo L,R       vectorscope pair (default 1,2)
  --loudness A,B,..  loudness inputs (default 1,2)
";

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}
