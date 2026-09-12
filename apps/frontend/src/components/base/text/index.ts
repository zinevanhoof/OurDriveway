import type { VariantProps } from "class-variance-authority";
import { cva } from "class-variance-authority";

export { default as Text } from "./Text.vue";
export { default as Title } from "./Title.vue";

/**
 * Shared by both, declared once. Not pulled into a third file — two consumers in the
 * same folder is not a module.
 *
 * `inverse` is for text sitting on `Surface variant="primary"`. There is no
 * `destructive-foreground` token in main.css, which is also why the app's destructive
 * button is a tint rather than a fill; on a filled primary surface the hierarchy is
 * carried by opacity (`class="opacity-80"`) rather than by a second token.
 */
const tone = {
  default: "text-foreground",
  muted: "text-muted-foreground",
  primary: "text-primary",
  destructive: "text-destructive",
  success: "text-success",
  inverse: "text-primary-foreground",
};

/**
 * Size and weight stay orthogonal.
 *
 * Coupling them would make the common cases one prop shorter — the roles do correlate,
 * a section header really is almost always `text-base font-bold`. But the app has four
 * weights across six sizes for what is arguably one role, and the way that happened is
 * that nobody could reach for a half-step without inventing a class string. Keeping the
 * axes separate is what makes `<Title size="sm" weight="bold">` a normal thing to write
 * instead of a reason to go back to raw Tailwind.
 *
 * The defaults carry the weight instead: `<Title>` with no props is the 12-site section
 * header, and `<Text>` with no props is the single most repeated string in the codebase.
 */
export const titleVariants = cva("", {
  variants: {
    size: {
      sm: "text-sm",
      md: "text-base",
      lg: "text-lg",
      xl: "text-xl",
      "2xl": "text-2xl",
    },
    weight: {
      medium: "font-medium",
      semibold: "font-semibold",
      bold: "font-bold",
      extrabold: "font-extrabold",
    },
    tone,
  },
  defaultVariants: { size: "md", weight: "bold", tone: "default" },
});

export const textVariants = cva("", {
  variants: {
    size: {
      xs: "text-xs",
      sm: "text-sm",
      md: "text-base",
      // Folds four spellings of the same overline into one — text-[11.5px]
      // font-semibold opacity-80, text-xs font-bold, text-[10px] font-bold, and
      // text-xs font-bold uppercase. Note the uppercase lives here: two call sites
      // currently .toUpperCase() the copy in JS, which a screen reader then spells out.
      eyebrow: "text-[11px] uppercase",
    },
    weight: {
      normal: "font-normal",
      medium: "font-medium",
      semibold: "font-semibold",
      bold: "font-bold",
    },
    tone,
  },
  defaultVariants: { size: "xs", weight: "medium", tone: "muted" },
});

export type TitleVariants = VariantProps<typeof titleVariants>;
export type TextVariants = VariantProps<typeof textVariants>;
