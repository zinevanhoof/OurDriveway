import type { VariantProps } from "class-variance-authority"
import { cva } from "class-variance-authority"

export { default as IconBox } from "./IconBox.vue"

/**
 * The tinted square an icon sits in. Eleven of these across the app, reached two
 * incompatible ways: padding-based (`p-2` around a `:size="22"` glyph) and fixed
 * (`size-9`, `w-10 h-10`). Between them they produce six different boxes — 24, 34, 36,
 * 38, 40 and 48px — none of which was chosen, they are just what `p-2` came out as
 * around whatever icon the author passed.
 *
 * Fixed-size only here, and the glyph is sized from the box. That is the part that stops
 * the drift coming back: call sites drop their `:size` prop entirely, so the box can no
 * longer change size because someone picked a different icon. Converting the existing
 * ones moves each by at most 4px.
 */
export const iconBoxVariants = cva("flex shrink-0 items-center justify-center", {
  variants: {
    size: {
      sm: "size-8 [&>svg]:size-4",
      md: "size-9 [&>svg]:size-[18px]",
      lg: "size-10 [&>svg]:size-5",
      xl: "size-12 [&>svg]:size-6",
    },
    tone: {
      accent: "bg-accent text-accent-foreground",
      // Accent ground, primary glyph — WalletComponent's "money in" treatment, which is
      // a different thing from `accent` and was reached by hand in three places.
      brand: "bg-accent text-primary",
      primary: "bg-primary text-primary-foreground",
      muted: "bg-muted",
      card: "bg-card text-muted-foreground border border-border",
    },
    shape: {
      square: "rounded-md",
      circle: "rounded-full",
    },
  },
  defaultVariants: { size: "md", tone: "accent", shape: "square" },
})

export type IconBoxVariants = VariantProps<typeof iconBoxVariants>
