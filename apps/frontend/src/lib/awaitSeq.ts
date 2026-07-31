// Read-your-own-writes for a publish-only backend.
//
// A write doesn't return a row — it returns the position its event landed at in
// the log (`202 { id, seq: "SPOTS:4712" }`). The projections that answer reads
// are per-instance and eventually consistent, so an immediate follow-up read can
// legitimately hit an instance that hasn't applied that event yet: you create a
// spot and it isn't in the list.
//
// Echoing the position back on subsequent requests lets view-service block until
// its own projector has caught up to it (see await_seq.rs, capped at 2s).

const HEADER = "X-Await-Seq";

/** `"SPOTS:4712"` — the newest write this client has made. */
let latest: string | null = null;

/**
 * Records the log position from a write response.
 *
 * Only ever moves forward within a stream. Switching streams (a spot write, then
 * a booking write) replaces it: the header carries one position, and the most
 * recent write is the one worth waiting for.
 */
export function recordSeq(seq: string | null | undefined): void {
  if (!seq || !/^[A-Z_]+:\d+$/.test(seq)) return;

  const [stream, value] = seq.split(":");
  const [currentStream, currentValue] = latest?.split(":") ?? [];
  if (stream === currentStream && Number(value) <= Number(currentValue)) return;

  latest = seq;
}

/**
 * Header for the newest write, or nothing if this client hasn't written.
 *
 * Deliberately never cleared. Once a projector is past the position the check is
 * a single integer comparison that returns immediately, so a stale header costs
 * nothing — while clearing it after one use would leave concurrent requests, and
 * requests that land on a *different* instance later, unprotected.
 */
export function awaitSeqHeader(): Record<string, string> {
  return latest ? { [HEADER]: latest } : {};
}

/** Test seam. */
export function resetSeq(): void {
  latest = null;
}

// ponytail: runnable self-check for the ordering rules — call demo() from a
// scratch script (`npx tsx`) if you touch recordSeq().
export function demo() {
  const eq = (got: unknown, want: unknown, what: string) => {
    if (JSON.stringify(got) !== JSON.stringify(want))
      throw new Error(
        `${what}: expected ${JSON.stringify(want)}, got ${JSON.stringify(got)}`,
      );
  };

  resetSeq();
  eq(awaitSeqHeader(), {}, "no writes yet -> no header");

  recordSeq("SPOTS:10");
  eq(awaitSeqHeader(), { "X-Await-Seq": "SPOTS:10" }, "first write");

  recordSeq("SPOTS:4"); // an older ack arriving late must not rewind us
  eq(awaitSeqHeader(), { "X-Await-Seq": "SPOTS:10" }, "never moves backwards");

  recordSeq("SPOTS:11");
  eq(awaitSeqHeader(), { "X-Await-Seq": "SPOTS:11" }, "moves forward");

  recordSeq("BOOKINGS:2"); // different stream: the newest write is what matters
  eq(
    awaitSeqHeader(),
    { "X-Await-Seq": "BOOKINGS:2" },
    "switching stream replaces",
  );

  for (const junk of [
    null,
    undefined,
    "",
    "SPOTS",
    "SPOTS:",
    ":5",
    "spots:5",
    "SPOTS:x",
  ]) {
    recordSeq(junk as string);
    eq(
      awaitSeqHeader(),
      { "X-Await-Seq": "BOOKINGS:2" },
      `junk ignored: ${junk}`,
    );
  }

  resetSeq();
  return "awaitSeq: all checks passed";
}
