import { Address, Availability } from "../domain/spot";

export type CreateSpotRequest = {
  title: string;
  description?: string;
  /** EUR cents, integer. The form collects euros and converts on submit. */
  pricePerHourCents: number;

  address: Address;
  availability: Availability;
  /**
   * Media keys, in display order — `spots/019f….jpeg`, never a URL. The photos
   * are already in R2 by the time this request is sent.
   */
  images: string[];
};
