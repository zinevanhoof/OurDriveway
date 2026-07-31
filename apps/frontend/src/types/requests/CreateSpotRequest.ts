import { Address, Availability } from "../domain/spot";

export type CreateSpotRequest = {
  title: string;
  description?: string;
  /** EUR cents, integer. The form collects euros and converts on submit. */
  pricePerHourCents: number;

  address: Address;
  availability: Availability;
};
