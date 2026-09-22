/**
 * Which way the last navigation went, and what a page does about it.
 *
 * **Vue Router never says whether a navigation was a push or a pop**, so this reads the
 * `position` counter the history keeps in `history.state` and compares it with the last
 * one it saw. Reading the history rather than tagging the call sites is what makes the
 * hardware back button and the Android back gesture behave: half the pops in this app
 * are not call sites at all.
 */
import { ref } from "vue";
import type { RouteLocationNormalized } from "vue-router";

/** `tab` is a sideways move between two navbar roots — neither a push nor a pop. */
export type NavDirection = "forward" | "back" | "tab";

export const navDirection = ref<NavDirection>("forward");

const position = () =>
  (window.history.state as { position?: number } | null)?.position ?? 0;
let last = position();

export function trackNavDirection(to: RouteLocationNormalized) {
  const moved = position() - last;
  last = position();

  navDirection.value =
    // A navbar root is never slid to, whatever it is arrived from: the tabs are the
    // app's floor, and a detail screen dismissing back to one is the same sideways
    // move as switching tabs, not a pop that should drag a screen off to the right.
    // Only `to` is asked — `from` being a tab is what a push *away* from one looks
    // like, and that one does slide.
    to.meta.tab
      ? "tab"
      : // A `replace` leaves the counter where it was, and every replace in this app is a
        // screen getting out of the way — withdraw back to the wallet, a deleted listing
        // back to the list — so "not forward" is the right reading of zero.
        moved > 0
        ? "forward"
        : "back";
}

// Not the app's usual `stiffness: 500, damping: 35`: that one is tuned to overshoot a
// little, which is right for a button arriving and wrong for a whole screen, where the
// bounce reads as the page having been thrown. `damping: 38` against `stiffness: 420` is
// just short of critical, so it settles without coming back.
//
// `zIndex: { duration: 0 }` makes the layer a switch rather than a number to interpolate;
// a z-index caught halfway between two pages is a frame of the wrong one on top.
const slide = {
  type: "spring",
  stiffness: 420,
  damping: 38,
  zIndex: { duration: 0 },
} as const;

/**
 * How long the page underneath a push has to stay mounted: long enough for the arriving
 * one to have covered it. Slightly past where the spring above has visually settled,
 * because unmounting early is what would show through.
 *
 * In seconds, motion's unit. `MobileLayout` reads it too, to know when the page that
 * covers the map has arrived — retune the spring and this moves with it.
 */
export const COVERED = 0.45;

/**
 * One page, animated by where it sits in the navigation rather than by which screen it is.
 *
 * **Exactly one of the two pages ever moves.** On a push it is the arriving one, sliding
 * in over a page that sits perfectly still; on a pop it is the departing one, sliding off
 * a page that was already in place. Nothing parallaxes and nothing is seen travelling
 * behind anything else — at every frame you are looking at one screen fully covering
 * another.
 *
 * That only works with `zIndex`, not DOM order: two pages are mounted at once and
 * `AnimatePresence` appends the arriving one, so left alone the newcomer always paints on
 * top. Right for a push, wrong for a pop, where the screen being dismissed has to stay in
 * front the whole way out.
 *
 * The `leave` variant is a function for a reason. An exiting child keeps the props it was
 * rendered with, so its own `custom` is one navigation stale by the time it matters —
 * `AnimatePresence`'s `custom` prop is what feeds this one the direction it is leaving in.
 * It arrives typed as `unknown`, which is why each of the three narrows it on the way in.
 */
export const pageVariants = {
  enter: (custom: unknown) => {
    const d = custom as NavDirection;
    if (d === "tab") return { opacity: 0, zIndex: 1 };
    // A pop arrives underneath and already in place. It is not sliding in — it is
    // being uncovered, so it must not be seen moving at all.
    if (d === "back") return { x: 0, zIndex: 0 };
    return { x: "100%", zIndex: 1 };
  },
  center: (custom: unknown) => {
    const d = custom as NavDirection;
    return {
      x: 0,
      opacity: 1,
      zIndex: d === "back" ? 0 : 1,
      transition: d === "tab" ? { duration: 0.15 } : slide,
    };
  },
  leave: (custom: unknown) => {
    const d = custom as NavDirection;
    // A tab switch does not render the outgoing page at all: `duration: 0`, so it is
    // gone on the frame the arriving one starts fading up from the background.
    if (d === "tab")
      return { opacity: 0, zIndex: 0, transition: { duration: 0 } };
    // A pop: this is the page that moves, and it stays in front the whole way out, so
    // the page being uncovered is never glimpsed sliding along behind it.
    if (d === "back") return { x: "100%", zIndex: 2, transition: slide };
    // A push: nothing about this page moves. It sits still underneath the arriving one
    // and unmounts only once covered — hence a fade to nothing on a delay rather than
    // an immediate exit, which would leave the background showing on the left for as
    // long as the slide takes. Motion needs *some* value to animate to hold a child
    // this long, and an opacity that only drops when it is already hidden is free.
    return {
      opacity: 0,
      zIndex: 0,
      transition: { duration: 0.01, delay: COVERED, zIndex: { duration: 0 } },
    };
  },
};
