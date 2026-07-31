import type { ClassValue } from "clsx";
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

// SurrealDB record ids are `table:id`; most GraphQL args want just the id part.
// Passes through anything without a colon (already bare, or null).
export const recordId = (id: string | null | undefined) =>
  id?.split(":")[1] ?? id;
