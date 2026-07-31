export type Address = {
  line1: string;
  line2?: string;
  city: string;
  postalCode: string;
  region?: string;
  country: string;
  formatted: string;
  // Present on autocomplete suggestions; absent in the create form (backend
  // re-geocodes on submit and never trusts client coords).
  lat?: number;
  lng?: number;
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
