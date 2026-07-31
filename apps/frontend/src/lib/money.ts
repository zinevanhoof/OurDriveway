// Money is stored and transported as integer EUR cents, everywhere. Floats only
// ever appear in a form input, and are converted at the boundary below.
//
// EUR is the base currency. `currency` is a parameter rather than a constant so
// that adding real conversion later touches this file and nothing else — but
// note that converting for real needs an exchange-rate source and a rate valid
// *at the time of the booking*, so it is not just a formatting change.

export const CURRENCY = "EUR";

/** Format cents for display, in the viewer's locale. */
export function formatCents(cents: number, currency: string = CURRENCY): string {
  return new Intl.NumberFormat(navigator.language, {
    style: "currency",
    currency,
  }).format(cents / 100);
}

/** Form input (euros, possibly fractional) -> cents. Rounds to the nearest cent. */
export function eurosToCents(euros: number): number {
  return Math.round(euros * 100);
}

/** Cents -> euros, for pre-filling a form input. */
export function centsToEuros(cents: number): number {
  return cents / 100;
}
