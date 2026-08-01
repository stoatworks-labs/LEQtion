//! LEQtion's measurement core.
//!
//! Pure DSP: no audio device, no threads, no clock. Everything here is a
//! function of the samples handed to it, which is what makes a sound level
//! meter testable — every claim this crate makes about a level is checked
//! against a synthetic signal whose answer is known in advance.
//!
//! The pieces, roughly in the order a sample meets them:
//!
//! | Module | What it does |
//! |---|---|
//! | [`window`] | Analysis windows and their normalisation constants |
//! | [`bands`] | Fractional-octave band table and bin integration |
//! | [`spectrum`] | Overlapped transforms → averaged band levels (the RTA) |
//! | [`weighting`] | A, C and Z, as both analytic curves and time-domain filters |
//! | [`spl`] | Fast/Slow/Impulse detectors, peaks, min/max |
//! | [`leq`] | Sliding and elapsed equivalent levels |
//! | [`calibration`] | Hardware calibrator workflow and the dBFS → dB SPL offset |
//! | [`engine`] | The one place that ties all of the above to a stream of samples |
//!
//! ## Conventions that hold everywhere in this crate
//!
//! - **Levels are dBFS until a calibration is applied**, and a full-scale sine
//!   reads 0 dBFS. Anything reporting a level says which it is; nothing
//!   silently presents a full-scale level as a sound pressure level.
//! - **Averaging happens in the energy domain.** Mean squares are averaged,
//!   never decibels. A mean of decibels is not a level.
//! - **Silence is [`spl::SILENCE_DBFS`], not `-inf`.** Infinities propagate
//!   through charts and averages and turn one muted input into a screen full of
//!   NaN.

pub mod bands;
pub mod calibration;
pub mod engine;
pub mod leq;
pub mod spectrum;
pub mod spl;
pub mod weighting;
pub mod window;

pub use bands::{Band, BandPlan, Fraction};
pub use calibration::{Calibration, CalibrationStatus, CalibrationTarget};
pub use engine::{ChannelSelect, Engine, EngineConfig, Frame, SplReading};
pub use leq::{LeqReading, LeqSpec, LeqWindow};
pub use spectrum::{Averaging, SpectrumAnalyser, SpectrumConfig, FFT_SIZES};
pub use spl::TimeWeighting;
pub use weighting::Weighting;
pub use window::WindowKind;
