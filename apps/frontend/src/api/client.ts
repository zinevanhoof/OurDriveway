import { useAuthStore } from "@/stores/auth";
import { refreshAccessToken } from "./refresh";
import { awaitVersionHeader, recordVersion } from "@/lib/awaitVersion";

// The one place a request is made, an error is read, or a write's version is
// recorded. It replaced `king.ts`, which did only the first of those and handed
// back a raw `Response` for every status — so each of the ten endpoint modules
// grew its own idea of what an error was, and four of them disagreed.
//
// ## Errors are thrown, always, as ApiError
//
// Nothing below returns a `Response`. A caller gets its parsed body or an
// `ApiError`, which is also what makes every one of these usable as a vue-query
// `queryFn`/`mutationFn` without a wrapper: TanStack decides success by whether
// the promise rejects.
//
// ## The body is read exactly once
//
// This is the rule the old client broke, and it cost every non-JWT 401 its
// message. `king.ts` peeked at a 401's body to decide whether to refresh —
// without cloning — and handed the drained response back, so the form's own
// `res.json()` threw "body stream already read", the `.catch` swallowed it, and
// a wrong password rendered as "Something went wrong. Please try again."
//
// The fix is not a `.clone()`. It is that there is only one reader now: the
// error is parsed here, into a value, and the refresh decision is made from that
// value rather than from the stream.

/** `{ status, title, errors }` — garde. Keys are already camelCase; see `MyError`. */
type ValidationBody = {
  status?: number;
  title?: string;
  errors?: Record<string, string[]>;
};

/** `{ status, title, detail }` — everything else. */
type DetailBody = { status?: number; title?: string; detail?: string[] };

/**
 * A failed request, with the backend's own words kept intact.
 *
 * The three shapes `MyError` can answer with, flattened into one type the UI can
 * branch on without knowing which arm produced it:
 *
 *   422 -> `fields`, per input, from garde
 *   4xx -> `detail`, form-level
 *   any -> `title`, the short human name of the failure
 *
 * `status === 422` and "`fields` is populated" are now the same statement — the
 * backend reserves 422 for garde alone. This still reads whichever key is
 * present rather than trusting the status, so it cannot be wrong about a body it
 * actually received.
 */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly title: string,
    /** Form-level messages, ready to render. Never empty — falls back to `title`. */
    readonly detail: string[],
    /** Per-field messages, camelCase keys matching the form's field names. */
    readonly fields: Record<string, string[]>,
  ) {
    super(detail[0] ?? title);
    this.name = "ApiError";
  }

  get isValidation() {
    return this.status === 422;
  }
  get isUnauthorized() {
    return this.status === 401;
  }
  get isForbidden() {
    return this.status === 403;
  }
  get isNotFound() {
    return this.status === 404;
  }
  get isConflict() {
    return this.status === 409;
  }
  /** No response at all — offline, DNS, a refused socket. Never a server answer. */
  get isOffline() {
    return this.status === 0;
  }

  /**
   * Hands per-field messages to vee-validate, and says whether there were any.
   *
   * `false` means there is nothing to show under an input and the caller should
   * fall through to {@link detail}. Both halves matter: a form that only calls
   * this drops every form-level error on the floor.
   */
  applyTo(setErrors: (fields: Record<string, string[]>) => void): boolean {
    const entries = Object.entries(this.fields);
    if (entries.length === 0) return false;

    setErrors(this.fields);
    return true;
  }

  /**
   * Reads a failed response. **The only place an error body is parsed.**
   *
   * Total: a body that is empty, HTML from a proxy, or truncated mid-flight
   * still produces a usable error rather than a `SyntaxError` from `.json()`
   * thrown out of the middle of the transport.
   */
  static async from(res: Response): Promise<ApiError> {
    const body: ValidationBody & DetailBody = await res
      .json()
      .catch(() => ({}) as ValidationBody & DetailBody);

    const title = body.title ?? res.statusText ?? "Request failed";
    const detail = body.detail?.length ? body.detail : [title];

    return new ApiError(res.status, title, detail, body.errors ?? {});
  }

  /** `fetch` itself rejected — there is no response to read a status off. */
  static offline(cause: unknown): ApiError {
    const message =
      cause instanceof Error ? cause.message : "Could not reach the server.";

    return new ApiError(0, "Network error", [message], {});
  }
}

/**
 * Whether a 401 is worth spending a refresh on.
 *
 * `AuthedJwt` answers a token it could not decode with `detail: ["JWT"]`
 * (`shared/src/extractors/authed_jwt.rs`), and that is the only 401 a new access
 * token can fix. A wrong password is also a 401 and must fall straight through —
 * refreshing on it would burn the session and hide the real message, which is
 * exactly what the old client did by accident.
 */
const isExpiredJwt = (err: ApiError) =>
  err.status === 401 && err.detail.some((d) => d.includes("JWT"));

/**
 * Paths stay relative: the app is served from the same origin as the API (Caddy
 * in prod, the vite proxy in dev), which is what lets the httponly refresh
 * cookie ride along on the default `credentials: "same-origin"`. On native,
 * `installNativeFetch` in `http.ts` has already rewritten `window.fetch` to
 * resolve these against Caddy's origin.
 */
async function send(
  method: string,
  path: string,
  body: unknown,
  init: RequestInit,
): Promise<Response> {
  const auth = useAuthStore();

  try {
    return await fetch(path, {
      ...init,
      method,
      // Only when there is one. Eight bodyless POSTs used to declare a JSON body
      // they did not have.
      body: body === undefined ? init.body : JSON.stringify(body),
      headers: {
        // No GraphQL left to content-negotiate for; every route answers JSON.
        Accept: "application/json",
        ...(body !== undefined && { "Content-Type": "application/json" }),
        ...(auth.accessToken && {
          Authorization: `Bearer ${auth.accessToken}`,
        }),
        // Hold the read until this client's own writes have been projected.
        ...awaitVersionHeader(),
        ...init.headers,
      },
    });
  } catch (cause) {
    throw ApiError.offline(cause);
  }
}

/**
 * Reads a successful response, recording the version it reached on the way past.
 *
 * `X-Version` replaced a `seq` field in the body, which every write had to carry
 * and every caller had to dig out — twice via `response.clone()`, because the
 * function was also handing the body to someone else. One header, read in one
 * place, and no endpoint module mentions versions at all any more.
 *
 * A write that touches no stream simply sends no header, so the two deliberate
 * opt-outs (`createSession`, `createAccountSession`) are enforced by the server
 * rather than by remembering not to call `recordVersion`.
 */
async function readBody<T>(res: Response): Promise<T> {
  recordVersion(res.headers.get("X-Version"));

  // Read as text and parse, rather than `.json()`: most writes answer 202 with
  // no body at all, `/email/resend` answers 204, and logout answers 200 with an
  // empty one — `.json()` throws on every one of those. Testing `content-length`
  // instead would work today and quietly stop working behind a proxy that
  // chunks, which is not a failure anyone would connect back to here.
  const text = await res.text();

  return (text ? JSON.parse(text) : undefined) as T;
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  init: RequestInit = {},
): Promise<T> {
  let res = await send(method, path, body, init);
  if (res.ok) return readBody<T>(res);

  const error = await ApiError.from(res);
  if (!isExpiredJwt(error)) throw error;

  // One retry, with a fresh token. A refresh that fails has already logged the
  // user out and `main.ts` routes them to login off `isAuthenticated`, so the
  // original 401 is the honest thing to report.
  if (!(await refreshAccessToken())) throw error;

  res = await send(method, path, body, init);
  if (!res.ok) throw await ApiError.from(res);

  return readBody<T>(res);
}

export const get = <T>(path: string) => request<T>("GET", path);
export const post = <T>(path: string, body?: unknown) =>
  request<T>("POST", path, body);
export const patch = <T>(path: string, body?: unknown) =>
  request<T>("PATCH", path, body);
/**
 * `init` is for exactly one caller: `release()` passes `keepalive` so the hold is
 * given back even when the renter closes the tab mid-checkout.
 */
export const del = <T>(path: string, init?: RequestInit) =>
  request<T>("DELETE", path, undefined, init);

/**
 * `?lng=4.9&lat=52.4` — skipping anything undefined, encoding everything else.
 *
 * Every query string in the app was a template literal, and `fetchWallet`
 * interpolated a caller-supplied `month` into one without encoding it.
 */
export function query(
  params: Record<string, string | number | undefined>,
): string {
  const search = new URLSearchParams();

  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) search.set(key, String(value));
  }

  const qs = search.toString();
  return qs ? `?${qs}` : "";
}
