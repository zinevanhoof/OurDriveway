// Backend (MyError) bodies:
//   422 -> { errors: { field: string[] } }   (garde validation, per field)
//   4xx -> { detail: string[] }               (context_* errors, form-level)
// The 422 keys are garde's paths, which are the *Rust* field names — snake_case,
// with array elements as `license_plates[0]`. vee-validate fields are camelCase
// with the same bracket syntax, so the only translation needed is the case.
type ValidationBody = { errors?: Record<string, string[]> };
type ErrorBody = { detail?: string[] };

/** `license_plates[0]` -> `licensePlates[0]`. No-op on names already camelCase. */
const toCamel = (key: string) =>
  key.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());

/**
 * Applies a backend 422 validation response to vee-validate field errors.
 * Returns true if it handled the response (was a 422 with field errors).
 */
export async function applyValidationErrors(
  response: Response,
  setErrors: (fields: Record<string, string | string[]>) => void,
): Promise<boolean> {
  if (response.status !== 422) return false;

  const body: ValidationBody = await response.json().catch(() => ({}));
  if (!body.errors) return false;

  setErrors(
    Object.fromEntries(
      Object.entries(body.errors).map(([path, messages]) => [
        toCamel(path),
        messages,
      ]),
    ),
  );
  return true;
}

/**
 * Reads the backend's `detail` messages for a non-validation error response,
 * for display as form-level errors. Falls back if the body can't be parsed.
 */
export async function readErrorDetail(response: Response): Promise<string[]> {
  const body: ErrorBody = await response.json().catch(() => ({}));
  return body.detail ?? ["Something went wrong. Please try again."];
}
