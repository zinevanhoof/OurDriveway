pub mod booking;
pub mod payment;
pub mod spot;
pub mod user;

/// The 422 body's keys are a contract with the frontend: `serverErrors.ts` maps
/// them onto vee-validate field names and only case-converts on the way.
///
/// Worth pinning because two things here quietly decide them. `#[garde(transparent)]`
/// on a newtype is what keeps a bad email reported as `email` and not `email[0]`,
/// and the rules that moved onto `general_models::spot` report through `dive`, which
/// is what keeps the grid's paths nested under the field that dove into it.
#[cfg(test)]
mod error_paths {
    use garde::Validate;

    fn paths<T: Validate<Context = ()>>(value: T) -> Vec<String> {
        let mut paths: Vec<String> = value
            .validate()
            .unwrap_err()
            .iter()
            .map(|(path, _)| path.to_string())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    #[test]
    fn a_newtype_reports_as_its_parent_field() {
        let bad = crate::requests::user::SignupRequest {
            first_name: "test".into(),
            last_name: "test".into(),
            email: serde_json::from_value("nope".into()).unwrap(),
            password: serde_json::from_value("weak".into()).unwrap(),
        };
        assert_eq!(paths(bad), vec!["email", "password"]);
    }

    #[test]
    fn the_grids_own_rules_report_under_the_field_that_dove_into_them() {
        use super::spot::fixtures::{create, slot};

        // A weekly slot off the 30-minute grid: the rule lives on
        // `WeeklyAvailability` now, and has to surface as a path the edit form can
        // find rather than at the root.
        let mut off_grid = create(500, vec![slot("08:15", "10:00")]);
        assert_eq!(paths(off_grid), vec!["availability.weekly.monday"]);

        // A one-off date in the past is a *submission* rule, applied on the field.
        off_grid = create(500, vec![slot("08:00", "10:00")]);
        off_grid
            .availability
            .single
            .insert("2000-01-01".into(), vec![slot("08:00", "10:00")]);
        assert_eq!(paths(off_grid), vec!["availability"]);
    }
}
