import { apiFetch } from "@/api/king";
import { User } from "@/types/User";

// Why REST and not a GraphQL query: the view's `user` table must be readable by
// everyone so spot-owner profiles resolve, which means `users { id }` would
// return every user rather than you. Here the server picks the row from the
// signature-verified JWT claim, so no permission clause has to be both
// "only me" and "public" at once.
//
// `profile` is null for the moment between registering and the projection
// catching up; `id` always resolves because it comes from the claim itself.
//
// `id` is a plain hyphenated uuid. That is the form a `where` filter on a uuid
// field takes directly; a `user(id:)` lookup needs it wrapped by gqlRecordId().
export async function fetchMe(): Promise<User> {
  const response = await apiFetch("/api/view/me");
  if (!response.ok) throw new Error(`fetchMe failed: ${response.status}`);

  const me = (await response.json()) as {
    id: string;
    profile: { first_name: string; last_name: string; profile_picture?: string | null } | null;
  };

  return {
    id: me.id,
    firstName: me.profile?.first_name ?? "",
    lastName: me.profile?.last_name ?? "",
    profilePicture: me.profile?.profile_picture ?? null,
  };
}
