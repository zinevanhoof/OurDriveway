import { apiFetch } from "./king";
import type { Address } from "@/types/domain/spot";

// Type-ahead suggestions proxied through spot-service (keeps the LocationIQ key
// server-side). Each item already carries every Address field, so a pick fills
// the form with no follow-up request.
export async function suggestAddress(q: string): Promise<Address[]> {
  const res = await apiFetch(
    `/api/spot/address/suggest?q=${encodeURIComponent(q)}`,
  );
  if (!res.ok) return [];
  return res.json();
}
