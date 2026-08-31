import { fetchMe as fetchMeRaw } from "@/api/viewApi";
import { User } from "@/types/User";

/**
 * The signed-in user, for the auth store.
 *
 * A thin adaptor over `viewApi.fetchMe` rather than a second fetch: the store wants a
 * flat `User` and the endpoint answers `{ id, profile }`, where `profile` is null for
 * the moment between registering and the projection catching up. `id` always resolves,
 * because it comes from the claim itself rather than from a row.
 *
 * Why the server picks the row at all, rather than the client asking by id: the read
 * model's `app_user` is readable by everyone so spot-owner profiles resolve, so a
 * query by id would need a permission clause that is somehow both "only me" and
 * "public". Choosing the row from a signature-verified claim sidesteps that. The
 * `email` and `licensePlates` this endpoint alone returns are the fields that used to
 * need the field-level clause.
 */
export async function fetchMe(): Promise<User> {
  const me = await fetchMeRaw();

  return {
    id: me.id,
    firstName: me.profile?.firstName ?? "",
    lastName: me.profile?.lastName ?? "",
    profilePicture: me.profile?.profilePicture ?? null,
  };
}
