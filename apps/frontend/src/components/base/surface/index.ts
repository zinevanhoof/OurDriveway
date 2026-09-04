import type { VariantProps } from "class-variance-authority"
import { cva } from "class-variance-authority"

export { default as Surface } from "./Surface.vue"

/**
 * The box. Roughly forty divs across the app are one of these, written ~24 different
 * ways, and the drift is entirely in what order the author happened to type the classes.
 *
 * Two things are baked into the base rather than exposed as variants, because the
 * evidence says there is no decision behind them:
 *
 *   radius  `rounded-md` is used 48 times against `rounded-lg` 14, and the 14 follow no
 *           rule — the same kind of panel is `rounded-md` in ManageSpotComponent and
 *           `rounded-lg` in CheckoutComponent.
 *   gap     `gap-3` covers 8 of the 10 rows that have children side by side. It is a
 *           class, not a prop, so `class="gap-8"` overrides it through tailwind-merge
 *           without needing an API for every spacing anyone might want.
 */
export const surfaceVariants = cva("flex rounded-md gap-3", {
  variants: {
    variant: {
      // Layout only. For grouping things that should not read as a box of their own.
      none: "",
      card: "bg-card text-card-foreground border border-border",
      elevated: "bg-card text-card-foreground border border-border shadow-xs",
      primary: "bg-primary text-primary-foreground",
      accent: "bg-accent text-accent-foreground",
      muted: "bg-muted",
      dashed: "bg-card border border-dashed border-border",
      destructive: "bg-destructive/5 border border-destructive/40",
    },
    // Padding only, and four steps is all the app actually has. The arbitrary values it
    // also has — py-3.25, py-4.5, py-2.5, px-3.5, px-5 — each appear once or twice with
    // no role behind them, so they are not steps, they are typos with a build step.
    size: {
      none: "p-0", // media-flush cards; the padding moves to an inner Surface
      sm: "px-3 py-2", // dense list rows, small stat tiles, nested strips
      md: "p-3", // full-width rows, booking cards, notices
      lg: "p-4", // page-level panels, hero tiles
    },
    orientation: {
      horizontal: "flex-row items-center",
      vertical: "flex-col",
    },
    // A pure addition: nothing in the app has a hover, pressed or focus treatment today,
    // so there is no existing convention to stay compatible with. `text-left` is here
    // because the common case is `as="button"`, which would otherwise centre its text.
    interactive: {
      true: "cursor-pointer text-left transition-colors active:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50",
      false: "",
    },
  },
  defaultVariants: {
    variant: "card",
    size: "md",
    // Vertical, because a Surface with no orientation given should stack the way a plain
    // div does. A row is the case worth naming.
    orientation: "vertical",
    interactive: false,
  },
})

export type SurfaceVariants = VariantProps<typeof surfaceVariants>
