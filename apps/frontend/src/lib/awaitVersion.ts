// Read-your-own-writes, keyed by aggregate.
//
// A write answers with the version its aggregate reached, in the `X-Version`
// header — `spot:019f…@3`. Reads are served from projections that lag the write by
// however long the outbox relay and the projector take, so an immediate follow-up
// read can legitimately land before its own write is visible: you create a spot
// and it isn't in the list.
//
// Echoing the version back on subsequent requests lets the read side block until it
// has reached that version (see bus/src/await_version.rs, capped at 2s).
//
// This used to carry a log position — `SPOTS:4712`. Two reasons it no longer can:
// once the database is authoritative a write commits *before* its event reaches
// NATS, so no stream sequence exists when the route has to answer; and a stream
// position waited on every spot in the system rather than the one just written.

const HEADER = "X-Await-Version";

/** Newest version this client has written, per aggregate (`spot:019f…`). */
let latest: Record<string, number> = {};

/**
 * Records the version from a write response's `X-Version` header.
 *
 * One entry per aggregate, each only ever moving forward. Per *aggregate* and not
 * per stream: two spots are two independent rows, and waiting on one must not make
 * a reader wait on the other.
 */
export function recordVersion(version: string | null | undefined): void {
  if (!version) return;

  // `<table>:<uuid>@<n>`. Split from the right on `@` so the aggregate half
  // stays intact, mirroring `parse_version` on the server.
  const at = version.lastIndexOf("@");
  if (at < 0) return;

  const aggregate = version.slice(0, at);
  const digits = version.slice(at + 1);

  // `/^\d+$/` rather than `Number.isInteger(Number(digits))`: `Number("")` is 0,
  // and 0 is a perfectly good integer — so `spot:…@` would have recorded version
  // 0 instead of being rejected. The self-check below caught exactly that.
  if (!aggregate.includes(":") || !/^\d+$/.test(digits)) return;

  const reached = Number(digits);

  latest[aggregate] = Math.max(reached, latest[aggregate] ?? 0);
}

/**
 * Header for every aggregate this client has written, or nothing if it hasn't.
 *
 * `spot:019f…@3,user:01a0…@7` — the server waits for each in turn, under one
 * shared timeout, and skips aggregates it holds no table for.
 *
 * Deliberately never cleared. Once a projection is past a version the check is a
 * single comparison that returns immediately, so a stale entry costs nothing —
 * while clearing after one use would leave concurrent requests, and requests that
 * land on a different instance later, unprotected.
 */
export function awaitVersionHeader(): Record<string, string> {
  const value = Object.entries(latest)
    .map(([aggregate, version]) => `${aggregate}@${version}`)
    .join(",");

  return value ? { [HEADER]: value } : {};
}

/** Test seam. */
export function resetVersions(): void {
  latest = {};
}

// ponytail: runnable self-check for the ordering rules — call demo() from a
// scratch script (`npx tsx`) if you touch recordVersion().
export function demo() {
  const eq = (got: unknown, want: unknown, what: string) => {
    if (JSON.stringify(got) !== JSON.stringify(want))
      throw new Error(
        `${what}: expected ${JSON.stringify(want)}, got ${JSON.stringify(got)}`,
      );
  };

  const A = "spot:019f0000-0000-7000-8000-000000000001";
  const B = "booking:019f0000-0000-7000-8000-000000000002";

  resetVersions();
  eq(awaitVersionHeader(), {}, "no writes yet -> no header");

  recordVersion(`${A}@10`);
  eq(awaitVersionHeader(), { "X-Await-Version": `${A}@10` }, "first write");

  recordVersion(`${A}@4`); // an older ack arriving late must not rewind us
  eq(awaitVersionHeader(), { "X-Await-Version": `${A}@10` }, "never moves backwards");

  recordVersion(`${A}@11`);
  eq(awaitVersionHeader(), { "X-Await-Version": `${A}@11` }, "moves forward");

  // A second aggregate is kept alongside the first, not instead of it — the whole
  // point of the map. A booking must not cost a pending spot write its position.
  recordVersion(`${B}@2`);
  eq(
    awaitVersionHeader(),
    { "X-Await-Version": `${A}@11,${B}@2` },
    "aggregates are tracked side by side",
  );

  recordVersion(`${A}@12`); // and each still moves independently
  eq(
    awaitVersionHeader(),
    { "X-Await-Version": `${A}@12,${B}@2` },
    "each aggregate moves on its own",
  );

  // Junk must not land an entry. The old stream form is junk now, which is the
  // one that would otherwise slip through: it has no `@`.
  resetVersions();
  for (const bad of ["SPOTS:4712", "", "nope", `${A}@`, `${A}@x`, "@3"]) {
    recordVersion(bad);
  }
  eq(awaitVersionHeader(), {}, "malformed versions are ignored");

  return "ok";
}
