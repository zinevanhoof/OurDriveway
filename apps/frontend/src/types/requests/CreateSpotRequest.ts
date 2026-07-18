export type CreateSpotRequest = {
  title: string;
  description?: string;
  pricePerHour: number;

  address: Address;
  availability: Availability;
};

export type Address = {
  line1: string;
  line2?: string;
  city: string;
  postalCode: string;
  region?: string;
  country: string;
  formatted: string;
};

export type Availability = {
  weekly: WeeklyAvailability;
  single: SingleAvailability;
};

export type WeeklyAvailability = {
  monday: TimeSlot[];
  tuesday: TimeSlot[];
  wednesday: TimeSlot[];
  thursday: TimeSlot[];
  friday: TimeSlot[];
  saturday: TimeSlot[];
  sunday: TimeSlot[];
};

export type SingleAvailability = Record<string, TimeSlot[]>;

export type TimeSlot = {
  start: string;
  end: string;
};
