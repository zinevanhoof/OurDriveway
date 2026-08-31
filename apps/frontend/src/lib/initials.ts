/**
 * One letter per word: `{ firstName: "Zine", lastName: "Van Hoof" }` -> `"ZVH"`.
 *
 * **Empty in, empty out.** That is the case this exists to get right: every caller
 * passes `?? ''` for a person who has not loaded or has not been projected yet, so
 * `("", "")` is the ordinary first render of any avatar, not an edge case.
 *
 * The trap is that `"".split(/\s+/)` is `[""]`, not `[]` — one empty string, which a
 * bare `.map(w => w[0].toUpperCase())` then reads `[0]` of and throws
 * `Cannot read properties of undefined`. `filter(Boolean)` is what makes the empty
 * case an empty string instead of a TypeError, and it also absorbs a name that is
 * only whitespace.
 */
export function initialsOf(name?: { firstName: string; lastName: string }): string {
  if (!name) return "";

  return `${name.firstName} ${name.lastName}`
    .split(/\s+/)
    .filter(Boolean)
    .map((word) => word[0].toUpperCase())
    .join("");
}

// ponytail: runnable self-check — call demo() from a scratch script (`npx tsx`)
// if you touch initialsOf().
export function demo() {
  const eq = (got: unknown, want: unknown, what: string) => {
    if (got !== want)
      throw new Error(`${what}: expected ${JSON.stringify(want)}, got ${JSON.stringify(got)}`);
  };

  eq(initialsOf({ firstName: "Zine", lastName: "Van Hoof" }), "ZVH", "one letter per word");
  eq(initialsOf({ firstName: "Ada", lastName: "Lovelace" }), "AL", "the ordinary case");

  // The four call sites all pass `?? ''` for an absent person, so this is the render
  // that used to throw: the drawer opening before its query lands, an unprojected
  // renter, the header before auth resolves.
  eq(initialsOf({ firstName: "", lastName: "" }), "", "no name is no initials, not a crash");
  eq(initialsOf({ firstName: "Ada", lastName: "" }), "A", "half a name still initials");
  eq(initialsOf({ firstName: "", lastName: "Lovelace" }), "L", "…from either half");
  eq(initialsOf({ firstName: "  ", lastName: "\t" }), "", "whitespace is not a word");
  eq(initialsOf(undefined), "", "no name at all — the slot is being used instead");

  console.log("initials: ok");
}
