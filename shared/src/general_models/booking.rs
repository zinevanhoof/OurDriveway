use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use diesel::deserialize::FromSqlRow;
use diesel::expression::AsExpression;
use serde::{Deserialize, Serialize};

use crate::general_models::spot::TimeSlot;

/// The slots a booking occupies: `"YYYY-MM-DD"` -> slots.
///
/// Bare wall-clock strings in the *spot's* timezone, with no zone attached and no
/// hold expiry — `ends_at` is the folded instant, and `hold_until` lives on the
/// booking row.
///
/// Deliberately the same shape as a spot's `availability.single`, so every reader —
/// including the frontend — subtracts one from the other with no reshaping.
///
/// ## A newtype, not `type Booked = HashMap<..>`
///
/// The alias meant a foreign type, so `jsonb_column!` could not put the `FromSql`/`ToSql`
/// impls on it — the orphan rule forbids it. That cost two wrappers in `diesel_ext`: one
/// applied with `deserialize_as` at ten field sites, and a second one for the single
/// `Option<Booked>` on the wallet, because `Option<BookedJson> -> Option<Booked>` ran into
/// the same rule from the other side. As a local type it carries the impls itself, exactly
/// as [`Address`](crate::general_models::spot::Address) and
/// [`Availability`](crate::general_models::spot::Availability) do, and `Option<Booked>`
/// then decodes through diesel's own blanket nullable impl with nothing hand-written.
///
/// **`#[serde(transparent)]` is load-bearing.** This is the payload of a *stored* event
/// (`events::booking::BookingCreated`) and what the frontend reads off every booking
/// screen. Without it serde would emit `{"0": {...}}` instead of the bare map, and every
/// event already in the log would stop decoding. See the round-trip test below.
///
/// `Deref`/`DerefMut` to the map because every reader treats this as a `HashMap` and
/// nothing is gained by making them say `.0` — the newtype is here for the trait impls,
/// not to hide the collection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, AsExpression, FromSqlRow)]
#[diesel(sql_type = diesel::sql_types::Jsonb)]
#[serde(transparent)]
pub struct Booked(pub HashMap<String, Vec<TimeSlot>>);

crate::jsonb_column!(Booked);

impl Booked {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deref for Booked {
    type Target = HashMap<String, Vec<TimeSlot>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Booked {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<HashMap<String, Vec<TimeSlot>>> for Booked {
    fn from(map: HashMap<String, Vec<TimeSlot>>) -> Self {
        Self(map)
    }
}

impl From<Booked> for HashMap<String, Vec<TimeSlot>> {
    fn from(Booked(map): Booked) -> Self {
        map
    }
}

impl IntoIterator for Booked {
    type Item = (String, Vec<TimeSlot>);
    type IntoIter = std::collections::hash_map::IntoIter<String, Vec<TimeSlot>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl FromIterator<(String, Vec<TimeSlot>)> for Booked {
    fn from_iter<T: IntoIterator<Item = (String, Vec<TimeSlot>)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(start: &str, end: &str) -> TimeSlot {
        TimeSlot {
            start: start.to_string(),
            end: end.to_string(),
        }
    }

    /// The wire format is the bare map, NOT `{"0": …}`.
    ///
    /// This is the whole reason for `#[serde(transparent)]`. `Booked` is the payload of a
    /// stored event and what every booking screen reads, so a wrapped shape would break
    /// the log and the frontend at once — silently, since serde would happily write the
    /// new shape and only fail on the old rows.
    #[test]
    fn the_json_shape_is_the_bare_map() {
        let booked = Booked::from(HashMap::from([(
            "2026-08-14".to_string(),
            vec![slot("09:00", "11:00")],
        )]));

        let json = serde_json::to_string(&booked).unwrap();
        assert_eq!(json, r#"{"2026-08-14":[{"start":"09:00","end":"11:00"}]}"#);

        let back: Booked = serde_json::from_str(&json).unwrap();
        assert_eq!(back, booked, "the map must round-trip unchanged");
    }

    /// An event written before the newtype has to keep decoding.
    #[test]
    fn a_bare_map_still_decodes() {
        let stored = r#"{"2026-08-14":[{"start":"09:00","end":"11:00"}]}"#;
        let booked: Booked = serde_json::from_str(stored).unwrap();
        assert_eq!(booked.get("2026-08-14").map(Vec::len), Some(1));
    }
}
