//! Input paths whose sensitivity is *specified* rather than measured.
//!
//! A hardware calibrator answers the only question that matters — how many dB
//! SPL does full scale correspond to, on this capsule, on this input, at this
//! gain — and nothing here is a substitute for one. But a phone has no
//! calibrator socket and no removable capsule, and on one platform the answer
//! is written down in a specification that every compliant device must meet.
//! Where that is true we can start a measurement already calibrated. Where it
//! is not, we show dBFS and say so.
//!
//! ## Android: the offset is a constant, and it is 130 dB
//!
//! The Compatibility Definition Document requires that a device declaring
//! support for `AudioSource.UNPROCESSED` meets, as MUST-level requirements:
//!
//! - **C-1-5** — a 1000 Hz sinusoid at 94 dB SPL yields RMS 520 for 16-bit
//!   samples, *or −36 dB full scale* for float samples.
//! - **C-1-2** — ±10 dB, 100 Hz to 7 kHz.
//! - **C-1-6** — SNR ≥ 60 dB. **C-1-7** — THD < 1% at 1 kHz, 90 dB SPL.
//! - **C-1-8/9** — signal processing disabled, and no level multiplier that
//!   introduces delay.
//!
//! C-1-5 *is* the calibration. Rearranged into the form this codebase uses:
//!
//! ```text
//! offset_db = reference_spl_db − measured_dbfs
//!           = 94.0 − (−36.0)
//!           = 130.0
//! ```
//!
//! (520 / 32768 = 0.015869, and 20·log₁₀(0.015869) = −35.9889 dB, so the two
//! halves of C-1-5 agree with each other to 0.011 dB — the spec's own
//! rounding, and four hundred times smaller than the ±5 dB an uncalibrated
//! guess would be out by. `android_offset_matches_the_published_figures`
//! pins it.)
//!
//! So this is not a per-model lookup table on Android. It is one number that
//! holds for every device reporting
//! `AudioManager.PROPERTY_SUPPORT_AUDIO_SOURCE_UNPROCESSED`, and a device
//! reporting it while missing the figure is out of compliance, not merely
//! unusual.
//!
//! What the CDD does *not* promise is a tolerance on C-1-5. The flatness,
//! noise and distortion figures are bounded; the sensitivity is stated as an
//! equality with no stated spread. Treat 130 dB as a good starting offset that
//! removes the need to guess, not as a claim of class 2 accuracy — which is
//! why this arrives as [`CalibrationSource::PlatformSpec`] and not as
//! something indistinguishable from a calibrator run.
//!
//! ## iOS: fixed, but unpublished
//!
//! The iOS input path can be made deterministic — `AVAudioSessionModeMeasurement`
//! removes system-supplied signal processing including AGC, and
//! `isInputGainSettable` is false for the built-in microphone, so there is no
//! gain to drift. That gets us a *stable* number. It does not tell us what the
//! number is: Apple publishes no sensitivity figure for the built-in capsule,
//! and there is no public per-model dataset. Every app that ships
//! "pre-calibrated" iOS profiles measured them itself, one model at a time.
//!
//! [`IOS_PROFILES`] is therefore empty on purpose, with the procedure for
//! filling it recorded on the constant. An empty table is the honest state; a
//! table of plausible-looking numbers would be the single worst thing this
//! module could contain, for exactly the reason recorded in the
//! `leqtion-dsp::calibration` module docs — a wrong offset is invisible
//! afterwards, because everything downstream is self-consistent and uniformly
//! wrong.

use leqtion_dsp::calibration::{Calibration, CalibrationSource, CalibrationTarget};

/// The reference the CDD states its sensitivity requirement against.
const CDD_REFERENCE_SPL_DB: f64 = 94.0;

/// The response the CDD requires at that reference, in dB full scale.
const CDD_REFERENCE_DBFS: f64 = -36.0;

/// dBFS → dB SPL offset for a compliant Android `UNPROCESSED` input.
///
/// See the module docs for the derivation. Written as the subtraction rather
/// than as `130.0` so that the two published figures it comes from stay
/// visible at the point of use.
pub const ANDROID_UNPROCESSED_OFFSET_DB: f64 = CDD_REFERENCE_SPL_DB - CDD_REFERENCE_DBFS;

/// Which capture path the samples are actually arriving through.
///
/// This exists as an explicit value rather than being inferred from
/// `target_os` because on Android the platform guarantee attaches to *one
/// audio source*, not to the operating system. A stream opened on any other
/// source on the same handset carries no guarantee at all, and applying
/// [`ANDROID_UNPROCESSED_OFFSET_DB`] to it would produce a confident, wrong
/// SPL. Nothing may assume the unprocessed path; something has to have asked
/// for it and had the request honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPath {
    /// Android `AudioSource.UNPROCESSED` / `AAUDIO_INPUT_PRESET_UNPROCESSED`,
    /// on a device reporting `PROPERTY_SUPPORT_AUDIO_SOURCE_UNPROCESSED`.
    ///
    /// **Nothing constructs this yet, and cpal cannot.** cpal's AAudio backend
    /// sets only device id, performance mode and sample rate — it never calls
    /// `AAudioStreamBuilder_setInputPreset`, and AAudio's default input preset
    /// is `VOICE_RECOGNITION`, a processed path. Reaching this variant needs
    /// either a patched cpal or a small AAudio shim alongside it, plus a JNI
    /// read of the `AudioManager` property to confirm the device claims
    /// support. Until then Android capture falls through to [`Self::Unknown`]
    /// and the app shows dBFS, which is the correct behaviour rather than a
    /// missing feature.
    AndroidUnprocessed,

    /// Everything else: every desktop input, every mobile input opened on a
    /// source with no published sensitivity. dBFS only, unless the user runs a
    /// calibrator.
    Unknown,
}

/// A sensitivity that is known without measuring it.
#[derive(Debug, Clone, PartialEq)]
pub struct InputProfile {
    /// Add this to a dBFS level to get dB SPL.
    pub offset_db: f64,
    /// How the offset is known.
    pub source: CalibrationSource,
    /// What is actually guaranteed, in one line, for display next to the
    /// reading. The user is entitled to know that their meter is trusting a
    /// specification rather than a calibrator.
    pub note: &'static str,
}

impl InputProfile {
    /// Turn the profile into a [`Calibration`] the engine can apply.
    ///
    /// `measured_dbfs` is filled in with the reference the offset was derived
    /// against, so the stored record reads the same way as one produced by a
    /// calibrator run and `full_scale_spl_db` keeps working unchanged.
    pub fn calibration(&self, device: &str) -> Calibration {
        let target = CalibrationTarget {
            level_db: CDD_REFERENCE_SPL_DB,
            frequency_hz: 1000.0,
        };
        let mut cal = Calibration::new(target, target.level_db - self.offset_db);
        cal.device = device.to_string();
        cal.source = self.source;
        cal
    }
}

/// Per-model offsets measured in-house on iOS hardware.
///
/// Empty, and an empty table is the correct state until someone has stood a
/// calibrator on a specific model. To add an entry: open the input with the
/// session mode set to measurement so AGC is out of the path, fit a class 1
/// calibrator, run the existing calibration workflow in the app, and record
/// the accepted `offset_db` here against the model identifier — not the
/// marketing name, which is not unique across regions.
///
/// Two things to record with any entry, because they change the number: iOS
/// exposes several capsules on the same handset and the offset is per capsule,
/// and a case over the microphone port is worth several decibels on its own.
pub const IOS_PROFILES: &[(&str, f64)] = &[];

/// The profile for a capture path, if its sensitivity is known.
///
/// `None` means dBFS: the honest answer whenever nothing can vouch for the
/// gain of the chain.
pub fn profile_for(path: InputPath) -> Option<InputProfile> {
    match path {
        InputPath::AndroidUnprocessed => Some(InputProfile {
            offset_db: ANDROID_UNPROCESSED_OFFSET_DB,
            source: CalibrationSource::PlatformSpec,
            note: "Android unprocessed input: sensitivity fixed by the platform \
                   specification at 94 dB SPL = −36 dBFS. Not a substitute for a \
                   calibrator.",
        }),
        InputPath::Unknown => None,
    }
}

/// The capture path currently in use.
///
/// Always [`InputPath::Unknown`] today. This is the single function the
/// Android port changes once the unprocessed source is actually reachable —
/// see [`InputPath::AndroidUnprocessed`] for what that takes. It is a function
/// rather than a constant so that the decision can become a runtime check of
/// the `AudioManager` property without disturbing any caller.
pub fn current_input_path() -> InputPath {
    InputPath::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_offset_matches_the_published_figures() {
        // Both halves of CDD C-1-5, checked against each other: RMS 520 of a
        // full-scale 16-bit sample is the stated −36 dBFS.
        let from_rms = 20.0 * (520.0f64 / 32768.0).log10();
        assert!(
            (from_rms - CDD_REFERENCE_DBFS).abs() < 0.02,
            "RMS 520 is {from_rms} dBFS, not {CDD_REFERENCE_DBFS}"
        );
        assert_eq!(ANDROID_UNPROCESSED_OFFSET_DB, 130.0);
    }

    #[test]
    fn a_profile_round_trips_to_the_reference_level() {
        // The whole point: feed the profile the dBFS the spec says a 94 dB
        // source produces, and get 94 dB back out.
        let profile = profile_for(InputPath::AndroidUnprocessed).expect("android profile");
        let cal = profile.calibration("test device");
        assert!((cal.spl_from_dbfs(CDD_REFERENCE_DBFS) - CDD_REFERENCE_SPL_DB).abs() < 1e-9);
        assert_eq!(cal.full_scale_spl_db(), 130.0);
    }

    #[test]
    fn a_profile_is_not_a_calibrator_run() {
        let profile = profile_for(InputPath::AndroidUnprocessed).expect("android profile");
        let cal = profile.calibration("test device");
        assert_eq!(cal.source, CalibrationSource::PlatformSpec);
        assert!(
            !cal.source.is_unit_specific(),
            "a platform guarantee describes a class of devices, not this one"
        );
    }

    #[test]
    fn an_unknown_path_yields_dbfs() {
        assert_eq!(profile_for(InputPath::Unknown), None);
    }

    #[test]
    fn nothing_claims_a_known_gain_until_the_port_lands() {
        // Guards the honest default. When the Android capture path can really
        // request the unprocessed source, change `current_input_path` and this
        // test together — deliberately, not by accident.
        assert_eq!(current_input_path(), InputPath::Unknown);
        assert_eq!(profile_for(current_input_path()), None);
    }

    #[test]
    fn the_ios_table_is_empty_rather_than_guessed() {
        // If this ever fails, an entry was added: it must have come from a
        // calibrator on that model, not from a plausible-looking number.
        assert!(IOS_PROFILES.is_empty());
    }
}
