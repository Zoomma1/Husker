use axum::extract::{FromRequest, Request};
use axum::Json;
use validator::{Validate, ValidationError};
use crate::errors::AppError;

/// Extractor qui désérialise le body JSON en `T` puis applique `Validate`.
///
/// - JSON malformé / illisible      -> `AppError::BadRequest` (400)
/// - JSON valide mais champ refusé   -> `AppError::Validation` (422)
///
/// Les handlers n'ont plus aucun `if ...is_empty()` : les contraintes sont
/// déclarées via `#[derive(Validate)]` sur les DTOs.
pub struct ValidatedJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: serde::de::DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state)
            .await
            .map_err(|rej| AppError::BadRequest(rej.body_text()))?;
        value
            .validate()
            .map_err(|e| AppError::Validation(e.to_string()))?;
        Ok(ValidatedJson(value))
    }
}

/// Refuse une chaîne vide ou composée uniquement d'espaces.
///
/// Reproduit le comportement historique `name.trim().is_empty()` sous forme
/// déclarative, réutilisable par n'importe quel DTO.
pub fn non_blank(value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::new("blank"));
    }
    Ok(())
}
