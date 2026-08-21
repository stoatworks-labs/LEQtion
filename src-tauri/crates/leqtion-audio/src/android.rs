//! Handing Android's JavaVM and Context to the audio backend.
//!
//! cpal's Android backend is AAudio, but the *device list* does not come from
//! AAudio — it comes from `android.media.AudioManager`, reached over JNI. To
//! make that call cpal needs two things it cannot obtain for itself: a pointer
//! to the process's JavaVM, and a `Context` object to call `getSystemService`
//! on. It asks `ndk-context` for both.
//!
//! In an ordinary Rust Android app `ndk-glue` fills `ndk-context` in before
//! `main`. There is no `main` here — Tauri owns the Android shell and does not
//! call `initialize_android_context`, and cpal is the only thing in the tree
//! that pulls `ndk-context` in at all. So nothing initialises it, and the
//! first attempt to list devices panics with **`android context was not
//! initialized`**, which surfaces as an immediate abort at launch rather than
//! as an audio error.
//!
//! This closes that gap from the one place that has both values: a JNI entry
//! point the activity calls with itself. See `MainActivity.kt` in
//! `gen/android`, which is why that directory is tracked rather than ignored.
//!
//! ## Why it has to be idempotent
//!
//! `initialize_android_context` asserts that it has not been called before, and
//! `onCreate` runs again whenever Android recreates the activity — a rotation
//! is enough. Left unguarded that assertion turns a rotation into a crash, so
//! the first call wins and later ones return without touching anything. The
//! flag is only set once the pointers are known good, so a failed attempt does
//! not lock out a later working one.

use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

static INITIALISED: AtomicBool = AtomicBool::new(false);

/// Whether the JavaVM and Context have been handed over.
///
/// Audio cannot be opened before this is true, and the failure if it is not is
/// an abort rather than an error, so it is worth being able to say so.
pub fn context_ready() -> bool {
    INITIALISED.load(Ordering::SeqCst)
}

/// `MainActivity.initAudioContext(Context)` — called from `onCreate`.
///
/// # Safety
///
/// Called by the JVM with a valid `JNIEnv` for the calling thread and a live
/// local reference to a `Context`. The reference is promoted to a global one
/// before being stored, because the local dies when this returns and cpal will
/// use it long afterwards. That global is deliberately never released: it lives
/// as long as the process, and `ndk-context` holds a raw pointer to it with no
/// way to learn that it has gone.
#[no_mangle]
pub unsafe extern "system" fn Java_com_allansargeant_leqtion_MainActivity_initAudioContext(
    env: *mut jni_sys::JNIEnv,
    _this: jni_sys::jobject,
    context: jni_sys::jobject,
) {
    if INITIALISED.load(Ordering::SeqCst) {
        return;
    }
    if env.is_null() || context.is_null() {
        return;
    }

    let functions = match (*env).as_ref() {
        Some(f) => f,
        None => return,
    };

    let mut vm: *mut jni_sys::JavaVM = std::ptr::null_mut();
    match functions.GetJavaVM {
        Some(get_java_vm) if get_java_vm(env, &mut vm) == jni_sys::JNI_OK && !vm.is_null() => {}
        _ => return,
    }

    let global = match functions.NewGlobalRef {
        Some(new_global_ref) => new_global_ref(env, context),
        None => return,
    };
    if global.is_null() {
        return;
    }

    ndk_context::initialize_android_context(vm.cast::<c_void>(), global.cast::<c_void>());
    INITIALISED.store(true, Ordering::SeqCst);
}
