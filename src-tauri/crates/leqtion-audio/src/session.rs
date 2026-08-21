//! Putting iOS into a state where a measurement is possible.
//!
//! On every other platform opening an input device is the whole story. On iOS
//! it is not: audio is arbitrated by a process-wide `AVAudioSession`, and
//! until one has been configured with a category that permits recording and
//! then made active, opening an input unit fails outright. cpal does not do
//! this for you — it watches the session for route changes and interruptions
//! but never sets the category — so without this the app builds, launches,
//! enumerates the device, and then refuses to start with
//! `Invalid property value`, which names neither the session nor the category.
//!
//! ## Why these settings and not others
//!
//! **`playAndRecord`** rather than `record`, because the generator has to be
//! able to play out while an input is open — running a measurement against a
//! known signal is the whole point of it. `record` alone would silence it.
//!
//! **`measurement` mode** is the one that matters for the numbers. It asks the
//! system to remove its input processing — automatic gain control above all —
//! and that is what makes a reading mean anything: AGC moving the gain under
//! the meter turns any calibration into a number that was true once. It also
//! makes the input path deterministic enough for a fixed offset to be a
//! sensible idea at all, which is the premise the whole `profiles` module
//! rests on. See its docs for what iOS does and does not publish.
//!
//! This is a request, not a guarantee. The system may decline, and nothing in
//! the API reports what it actually applied — so a measurement on iOS is
//! calibrated against the chain as it behaves, or it is dBFS.

/// Prepare the platform's audio system for capture.
///
/// Called before a device is opened. Everywhere except iOS there is nothing to
/// do and this succeeds immediately.
#[cfg(not(target_os = "ios"))]
pub fn prepare_for_measurement() -> Result<(), String> {
    Ok(())
}

/// Configure and activate the shared `AVAudioSession` for measurement.
///
/// Idempotent: setting the same category and mode on an already-active session
/// is not an error, so this can be called before every open rather than once
/// at startup — which is deliberate, because the session can be taken away by
/// an interruption and has to be re-established afterwards.
#[cfg(target_os = "ios")]
pub fn prepare_for_measurement() -> Result<(), String> {
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionCategoryPlayAndRecord,
        AVAudioSessionModeMeasurement,
    };

    // SAFETY: every call here is an ordinary Objective-C message to the shared
    // session object, which AVFoundation guarantees exists. The two globals are
    // framework string constants; they are `Option` because the linker cannot
    // prove they resolved, and a `None` means the framework is not present,
    // which is a broken build rather than a runtime condition to handle.
    unsafe {
        let session = AVAudioSession::sharedInstance();

        let category = AVAudioSessionCategoryPlayAndRecord
            .ok_or("AVAudioSessionCategoryPlayAndRecord is unavailable")?;
        let mode =
            AVAudioSessionModeMeasurement.ok_or("AVAudioSessionModeMeasurement is unavailable")?;

        session
            .setCategory_mode_options_error(category, mode, AVAudioSessionCategoryOptions(0))
            .map_err(|e| {
                format!("could not put the audio session into measurement mode: {e:?}")
            })?;

        session
            .setActive_error(true)
            .map_err(|e| format!("could not activate the audio session: {e:?}"))?;
    }

    Ok(())
}
