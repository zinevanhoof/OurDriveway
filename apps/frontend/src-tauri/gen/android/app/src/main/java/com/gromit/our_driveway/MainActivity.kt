package com.gromit.our_driveway

import android.graphics.Color
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import android.widget.FrameLayout
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature

/**
 * How long the splash may outlive the app's own startup, in milliseconds.
 *
 * The condition below is held open by the web side, so anything that stops the web
 * side from reporting in — a throw inside `bootstrap()`, a dev server that is not
 * answering, an origin this build did not expect — would otherwise hold the splash
 * open forever. Better a few seconds of logo followed by whatever the webview did
 * manage to render than an app that never starts.
 */
private const val SPLASH_TIMEOUT_MS = 50000L

/**
 * How long the splash takes to fade once the app is ready, in milliseconds.
 *
 * Short on purpose. This plays *after* the startup wait, so every millisecond here is
 * added to it — long enough to read as a dissolve rather than a cut, not long enough
 * to feel like part of the loading.
 */
private const val SPLASH_FADE_MS = 250L

class MainActivity : TauriActivity() {
  /**
   * Whether the splash may go. Flipped by the web side once Vue has painted, or by
   * the timeout, or immediately when the message channel is unavailable.
   *
   * Plain `var`, no synchronisation: every writer runs on the main thread —
   * `onPostMessage` is delivered there, and so is the `Handler` below — which is
   * also the only thread that reads it, from the pre-draw listener.
   */
  private var ready = false

  override fun onCreate(savedInstanceState: Bundle?) {
    // Before `super.onCreate`, which is where the library requires it: it is what
    // swaps the manifest's `Theme.our_driveway.Starting` for `postSplashScreenTheme`.
    //
    // The condition is polled on every pre-draw, so the activity holds the splash up
    // rather than handing over at its own first frame — which lands long before the
    // webview has content, and longer still before `bootstrap()` in main.ts has
    // finished awaiting `refreshAccessToken()` and `fetchMe()`. That gap is the blank
    // screen this exists to remove.
    installSplashScreen().apply {
      setKeepOnScreenCondition { !ready }

      // Without this the splash is cut away the moment the condition clears: the
      // platform's own exit animation on API 31+ is skipped whenever a keep-condition
      // held the splash past its natural dismissal, and below 31 the compat library
      // never had one. Taking the listener means owning the exit entirely — including
      // `remove()`, which is what actually tears the splash down. Miss that call and
      // the splash stays up forever, which is the one way this can fail.
      //
      // Fading the whole view rather than the icon alone: the window behind it is
      // already painted and already the same colour, so what the eye sees is the logo
      // dissolving into the app rather than a layer sliding off it.
      setOnExitAnimationListener { splash ->
        splash.view
          .animate()
          .alpha(0f)
          .setDuration(SPLASH_FADE_MS)
          .withEndAction { splash.remove() }
          .start()
      }
    }
    Handler(Looper.getMainLooper()).postDelayed({ ready = true }, SPLASH_TIMEOUT_MS)

    // targetSdk 36 means the window is edge-to-edge whether we ask for it or not —
    // Android 16 dropped `windowOptOutEdgeToEdgeEnforcement`. So the insets are
    // consumed here instead: the content frame is padded to the safe area and the
    // webview is laid out inside it. The web side never sees the system bars, and
    // env(safe-area-inset-*) stays 0 everywhere.
    //
    // The app is light-only (nothing ever adds the `.dark` class), so the bars are
    // pinned to dark icons rather than following the system theme — `auto` would
    // paint white icons over our white strip on a phone in dark mode. The nav bar's
    // dark scrim only shows on API < 27, which can't do dark icons there.
    enableEdgeToEdge(
      statusBarStyle = SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT),
      navigationBarStyle = SystemBarStyle.light(Color.TRANSPARENT, Color.argb(0x80, 0x1b, 0x1b, 0x1b)),
    )
    super.onCreate(savedInstanceState)

    val content = findViewById<View>(android.R.id.content)
    content.setBackgroundColor(getColor(R.color.system_bars))

    // MobileNavbar is `bg-card`, a shade lighter than the shell the status bar sits on,
    // so the navigation bar gets its own strip rather than the window colour. It hangs
    // off the decor view, not the content frame: children of the content frame lay out
    // *inside* its padding, which is exactly the region this needs to cover.
    val navBar = View(this).apply { setBackgroundColor(getColor(R.color.navigation_bar)) }
    (window.decorView as ViewGroup).addView(
      navBar,
      FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, Gravity.BOTTOM),
    )

    ViewCompat.setOnApplyWindowInsetsListener(content) { v, insets ->
      // `ime()` is in the mask so the viewport shrinks when the keyboard opens,
      // which is the adjustResize behaviour an edge-to-edge window otherwise loses.
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars()
          or WindowInsetsCompat.Type.displayCutout()
          or WindowInsetsCompat.Type.ime()
      )
      v.setPadding(bars.left, bars.top, bars.right, bars.bottom)

      // Measured on its own, not off `bars`: that one folds in the keyboard, and the
      // strip should not grow to its height. Zero in landscape, where the bar is a side.
      navBar.layoutParams.height =
        insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom
      navBar.requestLayout()

      WindowInsetsCompat.CONSUMED
    }
  }

  /**
   * Opens the one channel the web side uses to say it has painted: `window.splash`.
   *
   * `addWebMessageListener`, not `addJavascriptInterface`. The old API exposes its
   * object to every page the webview loads, and this one loads third-party pages —
   * Stripe's, on a redirect payment. This API takes origin rules instead, so only our
   * own document can reach it. The feature needs WebView 83, and the `else` branch
   * covers anything older by giving up on the signal rather than on starting.
   *
   * `WryActivity` calls this from `setWebView`, before Rust asks the webview to load
   * anything — which is what the listener needs, since it is injected on the next
   * navigation rather than into a document already open.
   *
   * The release rule is the origin Tauri serves the bundle from. Debug builds allow
   * anything because `tauri android dev` serves from the host machine's LAN address,
   * which is not known here and cannot be written as a rule. A rule that fails to
   * match is not fatal: the message never arrives and the timeout takes over.
   */
  override fun onWebViewCreate(webView: WebView) {
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
      ready = true
      return
    }

    val origins = if (BuildConfig.DEBUG) setOf("*") else setOf("http://tauri.localhost")

    WebViewCompat.addWebMessageListener(webView, "splash", origins) {
      _, _, _, isMainFrame, _ ->
      if (isMainFrame) ready = true
    }
  }
}
