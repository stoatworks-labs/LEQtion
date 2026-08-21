package com.allansargeant.leqtion

import android.content.Context
import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  /**
   * Hands the JavaVM and an Android Context to the Rust audio backend.
   *
   * Implemented in `crates/leqtion-audio/src/android.rs`. cpal reaches
   * AudioManager over JNI to enumerate devices and asks ndk-context for both
   * values; nothing under Tauri fills ndk-context in, so without this the
   * first attempt to list an input aborts the process at launch with
   * "android context was not initialized". The Rust side ignores repeat calls.
   */
  private external fun initAudioContext(context: Context)

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    // After super.onCreate, never before: the native library is loaded by the
    // `Rust` object's initialiser, and the first thing to touch it is
    // WryActivity's own onCreate. Calling earlier would throw
    // UnsatisfiedLinkError.
    //
    // applicationContext rather than `this`: cpal holds the reference in a
    // global for the life of the process, and holding an Activity that long
    // leaks it every time Android recreates one.
    initAudioContext(applicationContext)
  }
}
