package com.allansargeant.leqtion

import android.content.Context
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

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

    insetContentFromSystemBars()
  }

  /**
   * Keeps the page out from under the status and navigation bars.
   *
   * Android draws every app edge to edge from targetSdk 35 and, from 36,
   * ignores the opt-out attribute entirely, so the window genuinely spans the
   * whole screen and something has to account for it. The obvious web fix does
   * not work: `env(safe-area-inset-*)` reports the **display cutout** on
   * Android, not the system bars, so it reads zero on a phone with no notch
   * while the status bar still sits over the first line of the page. Adding
   * `viewport-fit=cover` to make it apply *regresses iOS*, where the webview is
   * already inset correctly.
   *
   * The padding goes on the activity's content view, not on the WebView.
   * Padding a WebView does not move anything: it lays its page out against its
   * full bounds and the padding is simply ignored, which looks exactly like the
   * listener never firing. Padding the container that holds it does move it.
   *
   * `systemBars() or displayCutout()` covers the bars on an ordinary phone and
   * the notch when a rotation moves the cutout to a side.
   */
  private fun insetContentFromSystemBars() {
    val content = findViewById<View>(android.R.id.content)

    // The bars sit over whatever is behind the content view, so the window
    // itself has to carry the app's colour or the inset shows as a pale band.
    window.setBackgroundDrawable(ColorDrawable(Color.parseColor(WINDOW_BACKGROUND)))

    ViewCompat.setOnApplyWindowInsetsListener(content) { view, windowInsets ->
      val insets = windowInsets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout(),
      )
      view.setPadding(insets.left, insets.top, insets.right, insets.bottom)
      // Returned unconsumed: nothing else in this window needs them, and
      // consuming would be a lie if that ever stops being true.
      windowInsets
    }

    // The view is already attached by this point, so the initial dispatch has
    // been and gone and the listener above would otherwise never fire.
    ViewCompat.requestApplyInsets(content)
  }

  private companion object {
    /** `--bg` from src/styles.css. Keep the two in step. */
    const val WINDOW_BACKGROUND = "#0b0d12"
  }
}
