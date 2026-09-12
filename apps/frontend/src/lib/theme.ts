/**
 * The app's own design tokens, in a form Stripe's SDKs accept.
 *
 * Both of Stripe's embedded surfaces — the Payment Element in `CheckoutComponent` and
 * Connect's components in the withdraw screen — are themed from the same CSS variables
 * the rest of the app uses, so they don't read as third-party panels dropped into the
 * page. Neither accepts the format those variables are actually in.
 */

/**
 * A CSS color string as `#rrggbb`, or `undefined` if it cannot be resolved.
 *
 * Needed because **Tailwind 4 emits `oklch(...)` and Stripe accepts only HEX, `rgb()`
 * or `hsl()`** — passing the raw token through gives a silently unthemed iframe.
 *
 * The canvas is the conversion: assigning any CSS color to `fillStyle` and reading it
 * back yields the browser's normalised form, which is hex for opaque colors. The
 * sentinel distinguishes "resolved to that value" from "refused it and kept the old
 * one", since an invalid assignment is a no-op rather than an error.
 */
export function cssColorToHex(value: string): string | undefined {
  if (!value) return undefined;

  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return undefined;

  const sentinel = "#010203";
  ctx.fillStyle = sentinel;
  ctx.fillStyle = value;

  const resolved = ctx.fillStyle;
  if (typeof resolved !== "string" || resolved === sentinel) return undefined;
  return resolved.startsWith("#") ? resolved : undefined;
}

/** One design token off `:root`, trimmed. `""` when it is not defined. */
export function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/**
 * A CSS length as pixels — `"0.75rem"` becomes `"12px"`.
 *
 * Needed because **Connect's appearance API accepts pixel values only** and rejects
 * everything else with a console error, while our tokens are in `rem`: `--radius` is
 * `0.75rem`. (The Payment Element is more forgiving and takes the token as it stands,
 * which is why `CheckoutComponent` passes it through unconverted.)
 *
 * `max` clamps, because Connect also caps some of these — `borderRadius` at 24px — and
 * a token that grows past the cap would be refused rather than reduced.
 *
 * `undefined` for anything it cannot read, so the caller omits the variable rather than
 * handing Stripe another value it will reject.
 */
export function cssLengthToPx(value: string, max?: number): string | undefined {
  const size = Number.parseFloat(value);
  if (!Number.isFinite(size)) return undefined;

  // Root-relative, because these are read off `:root` — an `em` there is an `rem`.
  const rootPx = Number.parseFloat(token("font-size") || "") || 16;
  const px = value.endsWith("rem") || value.endsWith("em") ? size * rootPx : size;

  if (!value.endsWith("px") && !value.endsWith("rem") && !value.endsWith("em")) {
    return undefined;
  }
  return `${Math.round(max === undefined ? px : Math.min(px, max))}px`;
}
