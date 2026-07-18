use axum::extract::{FromRequest, Request};
use axum::Json;
use garde::Validate;
use serde::de::DeserializeOwned;

use crate::error::myerror::MyError;

/// Extracts `Json<T>` then runs garde validation, returning a `MyError`
/// (per-field JSON on validation failure) instead of axum-valid's fixed body.
pub struct Valid<T>(pub T);

impl<S, T> FromRequest<S> for Valid<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate,
    T::Context: Default,
{
    type Rejection = MyError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await?;
        value.validate()?;
        Ok(Valid(value))
    }
}
