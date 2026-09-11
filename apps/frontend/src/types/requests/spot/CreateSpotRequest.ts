import type { Address, Availability } from "@/types/domain/spot";

/** `POST /api/spot`. `shared::requests::spot::CreateSpotRequest`. */
export type CreateSpotRequest = {
  title: string;
  description?: string;
  /** EUR cents, integer. The form collects euros and converts on submit. */
  pricePerHourCents: number;
  address: Address;
  availability: Availability;
  /**
   * Media URLs, in display order — the whole thing, `https://…/spots/019f….jpeg`,
   * exactly as `POST /api/media/upload-url` returned it in its `url` field. Not a bucket
   * key: `shared::media::is_media_url` checks the origin and the path shape together, so
   * a bare `spots/019f….jpeg` is rejected.
   *
   * The photos are already in R2 by the time this request is sent.
   */
  images: string[];
};
