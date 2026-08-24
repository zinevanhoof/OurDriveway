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

/** The newest position this client has written, per stream. */
let latest: Record<string, number> = {};

/**
 * Records the log position from a write response.
 *
 * One position per stream, each only ever moving forward. It used to be a single
 * slot that a write to another stream replaced — fine while only spot and booking
 * writes carried a seq, but user-service now answers login and refresh with a
 * SESSIONS position, and those are frequent enough to have displaced the USERS or
 * SPOTS position a following read still needed.
 */
export function recordSeq(seq: string | null | undefined): void {
  if (!seq || !/^[A-Z_]+:\d+$/.test(seq)) return;

  const [stream, value] = seq.split(":");
  latest[stream] = Math.max(Number(value), latest[stream] ?? 0);
}

/**
 * Header for every stream this client has written, or nothing if it hasn't.
 *
 * `SPOTS:4712,SESSIONS:19` — the server waits for each in turn, under one shared
 * timeout, and ignores streams it doesn't project.
 *
 * Deliberately never cleared. Once a projector is past a position the check is a
 * single integer comparison that returns immediately, so a stale entry costs
 * nothing — while clearing after one use would leave concurrent requests, and
 * requests that land on a *different* instance later, unprotected.
 */
export function awaitSeqHeader(): Record<string, string> {
  const value = Object.entries(latest)
    .map(([stream, seq]) => `${stream}:${seq}`)
    .join(",");

  return value ? { [HEADER]: value } : {};
}

/** Test seam. */
export function resetSeq(): void {
  latest = {};
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

  // A second stream is kept alongside the first, not instead of it — the whole
  // point of the map. A login must not cost a pending spot write its position.
  recordSeq("BOOKINGS:2");
  eq(
    awaitSeqHeader(),
    { "X-Await-Seq": "SPOTS:11,BOOKINGS:2" },
    "streams are tracked side by side",
  );

  recordSeq("SPOTS:12"); // and each still moves independently
  eq(
    awaitSeqHeader(),
    { "X-Await-Seq": "SPOTS:12,BOOKINGS:2" },
    "one stream advancing leaves the other alone",
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
      { "X-Await-Seq": "SPOTS:12,BOOKINGS:2" },
      `junk ignored: ${junk}`,
    );
  }

  resetSeq();
  return "awaitSeq: all checks passed";
}
