use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

// ── Auth ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Claims {
    iss: String,
    scope: String,
    aud: String,
    exp: i64,
    iat: i64,
}

/// Generate a signed JWT for Firebase service account authentication.
/// Pass the RSA private key PEM string and the service account client email.
pub async fn generate_jwt(
    private_key: &str,
    client_email: &str,
) -> Result<String, RtdbError> {
    let now = Utc::now();
    let exp = now
        .checked_add_signed(Duration::seconds(3600))
        .expect("valid timestamp")
        .timestamp();

    let claims = Claims {
        iss: client_email.to_string(),
        scope: "https://www.googleapis.com/auth/firebase.database \
                https://www.googleapis.com/auth/userinfo.email"
            .to_string(),
        aud: "https://oauth2.googleapis.com/token".to_string(),
        exp,
        iat: now.timestamp(),
    };

    let key = EncodingKey::from_rsa_pem(private_key.as_bytes())
        .map_err(|e| RtdbError::Auth(e.to_string()))?;
    let header = Header::new(jsonwebtoken::Algorithm::RS256);
    let token = encode(&header, &claims, &key)
        .map_err(|e| RtdbError::Auth(e.to_string()))?;

    Ok(token)
}

#[derive(Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Exchange a signed JWT for a Firebase OAuth2 access token.
pub async fn exchange_jwt_for_access_token(jwt: &str) -> Result<String, RtdbError> {
    let client = Client::new();
    let params = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
        ("assertion", jwt),
    ];

    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(RtdbError::Request)?
        .json::<AccessTokenResponse>()
        .await
        .map_err(RtdbError::Request)?;

    if let Some(token) = response.access_token {
        Ok(token)
    } else if let Some(error) = response.error {
        Err(RtdbError::Auth(format!(
            "{}: {}",
            error,
            response.error_description.unwrap_or_default()
        )))
    } else {
        Err(RtdbError::Auth(
            "No access token or error in response".to_string(),
        ))
    }
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// A document returned by the Firebase RTDB REST API.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RtdbDocument {
    pub name: String,
    pub fields: HashMap<String, RtdbFieldValue>,
}

/// A single field value — mirrors the RTDB REST API wire format.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RtdbFieldValue {
    pub string_value: Option<String>,
    pub integer_value: Option<i64>,
    pub boolean_value: Option<bool>,
    pub array_value: Option<RtdbArrayValue>,
    pub map_value: Option<HashMap<String, RtdbFieldValue>>,
}

impl RtdbFieldValue {
    pub fn string(value: impl Into<String>) -> Self {
        Self {
            string_value: Some(value.into()),
            integer_value: None,
            boolean_value: None,
            array_value: None,
            map_value: None,
        }
    }

    pub fn integer(value: i64) -> Self {
        Self {
            string_value: None,
            integer_value: Some(value),
            boolean_value: None,
            array_value: None,
            map_value: None,
        }
    }

    pub fn boolean(value: bool) -> Self {
        Self {
            string_value: None,
            integer_value: None,
            boolean_value: Some(value),
            array_value: None,
            map_value: None,
        }
    }
}

/// An array value in the RTDB REST wire format.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RtdbArrayValue {
    pub values: Vec<RtdbFieldValue>,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RtdbError {
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Parse error: {0}")]
    Parse(String),
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Read a value from Firebase RTDB at `path`.
///
/// # Arguments
/// * `base_url` - Your project URL, e.g. `https://my-project.firebaseio.com`
/// * `path`     - The node path, e.g. `users/alice`
/// * `token`    - OAuth2 access token from `exchange_jwt_for_access_token`
pub async fn get(
    base_url: &str,
    path: &str,
    token: &str,
) -> Result<Value, RtdbError> {
    let url = format!("{}/{}.json?auth={}", base_url.trim_end_matches('/'), path, token);
    let response = Client::new()
        .get(&url)
        .send()
        .await
        .map_err(RtdbError::Request)?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(RtdbError::NotFound(path.to_string()));
    }

    response
        .json::<Value>()
        .await
        .map_err(RtdbError::Request)
}

/// Write (overwrite) a value at `path` using HTTP PUT.
pub async fn put(
    base_url: &str,
    path: &str,
    token: &str,
    body: &Value,
) -> Result<Value, RtdbError> {
    let url = format!("{}/{}.json?auth={}", base_url.trim_end_matches('/'), path, token);
    Client::new()
        .put(&url)
        .json(body)
        .send()
        .await
        .map_err(RtdbError::Request)?
        .json::<Value>()
        .await
        .map_err(RtdbError::Request)
}

/// Update specific fields at `path` using HTTP PATCH (does not overwrite siblings).
pub async fn patch(
    base_url: &str,
    path: &str,
    token: &str,
    body: &Value,
) -> Result<Value, RtdbError> {
    let url = format!("{}/{}.json?auth={}", base_url.trim_end_matches('/'), path, token);
    Client::new()
        .patch(&url)
        .json(body)
        .send()
        .await
        .map_err(RtdbError::Request)?
        .json::<Value>()
        .await
        .map_err(RtdbError::Request)
}

/// Delete the value at `path`.
pub async fn delete(
    base_url: &str,
    path: &str,
    token: &str,
) -> Result<(), RtdbError> {
    let url = format!("{}/{}.json?auth={}", base_url.trim_end_matches('/'), path, token);
    Client::new()
        .delete(&url)
        .send()
        .await
        .map_err(RtdbError::Request)?;
    Ok(())
}




// Testing


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_field_value() {
        let v = RtdbFieldValue::string("hello");
        assert_eq!(v.string_value, Some("hello".to_string()));
        assert!(v.integer_value.is_none());
        assert!(v.boolean_value.is_none());
    }

    #[test]
    fn integer_field_value() {
        let v = RtdbFieldValue::integer(42);
        assert_eq!(v.integer_value, Some(42));
        assert!(v.string_value.is_none());
    }

    #[test]
    fn boolean_field_value() {
        let v = RtdbFieldValue::boolean(true);
        assert_eq!(v.boolean_value, Some(true));
        assert!(v.string_value.is_none());
    }
}