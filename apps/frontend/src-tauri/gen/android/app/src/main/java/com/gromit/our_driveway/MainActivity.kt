package com.gromit.our_driveway

import android.graphics.Color
import android.os.Bundle
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
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
}
