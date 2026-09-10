export type Address = {
  line1: string;
  line2?: string;
  city: string;
  postalCode: string;
  region?: string;
  country: string;
  formatted: string;
  // On autocomplete suggestions only, and `null` there for a hit the provider gave no
  // parseable point for — hence `| null` rather than just optional. Absent entirely in
  // the create form: the backend re-geocodes on submit and never trusts client coords.
  lat?: number | null;
  lng?: number | null;
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
