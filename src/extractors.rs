use crate::errors::AppError;
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use axum::Json;
use validator::{Validate, ValidationError};

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

/// Extractor qui désérialise la query string en `T`, en normalisant le rejet vers
/// `AppError::BadRequest` (JSON `{"error": ...}`) au lieu du texte brut renvoyé par
/// `axum::extract::Query` — cohérent avec `ValidatedJson` pour le body (HUSKER-25).
pub struct ValidatedQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidatedQuery<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(|rej| AppError::BadRequest(rej.body_text()))?;
        Ok(ValidatedQuery(value))
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
