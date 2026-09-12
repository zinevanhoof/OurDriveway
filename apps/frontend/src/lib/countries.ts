/**
 * The countries a host can be paid in.
 *
 * Only the codes are listed. The **names come from `Intl.DisplayNames`**, so the select
 * reads in the viewer's own language and there is no translation table here to go stale
 * — the browser already ships one.
 *
 * The list is the SEPA area, which is where this app operates and where a EUR payout
 * lands without a currency conversion. It is deliberately not "every country Stripe
 * supports": that set changes on Stripe's schedule, and a copy of it here would be
 * wrong in the direction that matters — offering a country whose account then cannot be
 * created. Stripe still has the final say either way, and refuses the ones it does not
 * support.
 *
 * ponytail: a hand-kept list. If OurDriveway ever operates outside SEPA, the fix is to
 * fetch Stripe's supported-country list server-side and serve it, not to paste a longer
 * array here.
 */
const SEPA = [
  "AT", "BE", "BG", "CH", "CY", "CZ", "DE", "DK", "EE", "ES",
  "FI", "FR", "GB", "GR", "HR", "HU", "IE", "IS", "IT", "LI",
  "LT", "LU", "LV", "MT", "NL", "NO", "PL", "PT", "RO", "SE",
  "SI", "SK",
] as const;

/** The country's name in the viewer's language, or the bare code if it has none. */
export function countryName(code: string): string {
  // Constructed per call rather than once at module load: `navigator.language` is read
  // at construction, and this file is imported long before anything renders.
  const names = new Intl.DisplayNames([navigator.language], { type: "region" });
  return names.of(code) ?? code;
}

/** `[{ code, name }]`, sorted the way the viewer's locale sorts names. */
export function countryOptions(): { code: string; name: string }[] {
  return SEPA.map((code) => ({ code, name: countryName(code) })).sort((a, b) =>
    a.name.localeCompare(b.name, navigator.language),
  );
}
