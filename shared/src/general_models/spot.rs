use std::collections::HashMap;

use serde::Deserialize;
use surrealdb::types::SurrealValue;

#[derive(Deserialize, SurrealValue)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub postal_code: String,
    pub region: Option<String>,
    pub country: String,
    pub formatted: String,
}

#[derive(Deserialize, SurrealValue)]
pub struct Availability {
    pub weekly: WeeklyAvailability,
    pub single: HashMap<String, Vec<TimeSlot>>,
}

#[derive(Deserialize, SurrealValue)]
pub struct WeeklyAvailability {
    pub monday: Vec<TimeSlot>,
    pub tuesday: Vec<TimeSlot>,
    pub wednesday: Vec<TimeSlot>,
    pub thursday: Vec<TimeSlot>,
    pub friday: Vec<TimeSlot>,
    pub saturday: Vec<TimeSlot>,
    pub sunday: Vec<TimeSlot>,
}

#[derive(Deserialize, SurrealValue)]
pub struct TimeSlot {
    pub start: String, // "08:00"
    pub end: String,   // "18:00"
}
