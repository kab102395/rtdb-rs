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

    #[error("Invalid query: {0}")]
    InvalidQuery(String),
}

// ── Query Builder Types ───────────────────────────────────────────────────────

/// Controls how results are ordered.
///
/// Firebase requires `orderBy` to be set before using `limit_to_first`,
/// `limit_to_last`, `start_at`, `end_at`, or `equal_to`.
#[derive(Debug, Clone)]
pub enum OrderBy {
    /// Sort by Firebase push key. Equivalent to `orderBy="$key"`.
    Key,
    /// Sort by node value. Equivalent to `orderBy="$value"`.
    /// Useful when nodes are primitives rather than objects.
    Value,
    /// Sort by Firebase priority. Equivalent to `orderBy="$priority"`.
    Priority,
    /// Sort by a child field. Equivalent to `orderBy="fieldName"`.
    Child(String),
}

impl OrderBy {
    fn as_query_param(&self) -> String {
        match self {
            OrderBy::Key => "\"$key\"".to_string(),
            OrderBy::Value => "\"$value\"".to_string(),
            OrderBy::Priority => "\"$priority\"".to_string(),
            OrderBy::Child(field) => format!("\"{}\"", field),
        }
    }
}

/// A value used with `start_at`, `end_at`, and `equal_to` filters.
///
/// Firebase encodes filter values differently based on type:
/// strings are JSON-quoted, numbers and booleans are bare.
#[derive(Debug, Clone)]
pub enum FilterValue {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
}

impl FilterValue {
    pub fn string(s: impl Into<String>) -> Self {
        FilterValue::String(s.into())
    }

    pub fn number(n: f64) -> Self {
        FilterValue::Number(n)
    }

    pub fn boolean(b: bool) -> Self {
        FilterValue::Bool(b)
    }

    fn as_query_param(&self) -> String {
        match self {
            FilterValue::String(s) => format!("\"{}\"", s),
            FilterValue::Number(n) => n.to_string(),
            FilterValue::Bool(b) => b.to_string(),
            FilterValue::Null => "null".to_string(),
        }
    }
}

// ── GetBuilder ────────────────────────────────────────────────────────────────

/// A builder for filtered GET requests against the Firebase RTDB REST API.
///
/// Created via [`RtdbClient::query`]. Chain filter methods and call `.send().await`.
///
/// # Example
/// ```no_run
/// # use rtdb_rs::{RtdbClient, FilterValue, RtdbError};
/// # async fn example() -> Result<(), rtdb_rs::RtdbError> {
/// # let client = rtdb_rs::RtdbClient::new("https://my-project.firebaseio.com", "token");
/// let results = client
///     .query("orders")
///     .order_by_child("status")
///     .equal_to(rtdb_rs::FilterValue::string("pending"))
///     .limit_to_first(25)
///     .send()
///     .await?;
/// # Ok(()) }
/// ```
pub struct GetBuilder<'a> {
    client: &'a Client,
    base_url: &'a str,
    path: String,
    token: &'a str,
    order_by: Option<OrderBy>,
    limit_to_first: Option<u32>,
    limit_to_last: Option<u32>,
    start_at: Option<FilterValue>,
    end_at: Option<FilterValue>,
    equal_to: Option<FilterValue>,
    shallow: bool,
}

impl<'a> GetBuilder<'a> {
    fn new(client: &'a Client, base_url: &'a str, path: &str, token: &'a str) -> Self {
        Self {
            client,
            base_url,
            path: path.trim_matches('/').to_string(),
            token,
            order_by: None,
            limit_to_first: None,
            limit_to_last: None,
            start_at: None,
            end_at: None,
            equal_to: None,
            shallow: false,
        }
    }

    /// Order results by a child field.
    /// Shorthand for `.order_by(OrderBy::Child("field"))`.
    pub fn order_by_child(mut self, field: &str) -> Self {
        self.order_by = Some(OrderBy::Child(field.to_string()));
        self
    }

    /// Order results by Firebase push key (`$key`).
    pub fn order_by_key(mut self) -> Self {
        self.order_by = Some(OrderBy::Key);
        self
    }

    /// Order results by node value (`$value`).
    /// Use when nodes are primitives, not objects.
    pub fn order_by_value(mut self) -> Self {
        self.order_by = Some(OrderBy::Value);
        self
    }

    /// Set the ordering explicitly via [`OrderBy`].
    pub fn order_by(mut self, order: OrderBy) -> Self {
        self.order_by = Some(order);
        self
    }

    /// Return only the first `n` results (requires `order_by`).
    /// Mutually exclusive with `limit_to_last`.
    pub fn limit_to_first(mut self, n: u32) -> Self {
        self.limit_to_first = Some(n);
        self.limit_to_last = None;
        self
    }

    /// Return only the last `n` results (requires `order_by`).
    /// Mutually exclusive with `limit_to_first`.
    pub fn limit_to_last(mut self, n: u32) -> Self {
        self.limit_to_last = Some(n);
        self.limit_to_first = None;
        self
    }

    /// Filter to results where the ordered field is >= this value.
    pub fn start_at(mut self, val: FilterValue) -> Self {
        self.start_at = Some(val);
        self
    }

    /// Filter to results where the ordered field is <= this value.
    pub fn end_at(mut self, val: FilterValue) -> Self {
        self.end_at = Some(val);
        self
    }

    /// Filter to results where the ordered field exactly equals this value.
    pub fn equal_to(mut self, val: FilterValue) -> Self {
        self.equal_to = Some(val);
        self
    }

    /// Return only keys, not values. Cannot be combined with other query params.
    /// Useful for checking existence or counting nodes without fetching all data.
    pub fn shallow(mut self) -> Self {
        self.shallow = true;
        self
    }

    pub fn build_url(&self) -> Result<String, RtdbError> {
        // shallow cannot be combined with ordering or filtering
        if self.shallow {
            let has_filters = self.order_by.is_some()
                || self.limit_to_first.is_some()
                || self.limit_to_last.is_some()
                || self.start_at.is_some()
                || self.end_at.is_some()
                || self.equal_to.is_some();

            if has_filters {
                return Err(RtdbError::InvalidQuery(
                    "shallow=true cannot be combined with orderBy, limit, or filter params".to_string(),
                ));
            }

            return Ok(format!(
                "{}/{}.json?auth={}&shallow=true",
                self.base_url, self.path, self.token
            ));
        }

        // limit and filter params require orderBy
        let needs_order = self.limit_to_first.is_some()
            || self.limit_to_last.is_some()
            || self.start_at.is_some()
            || self.end_at.is_some()
            || self.equal_to.is_some();

        if needs_order && self.order_by.is_none() {
            return Err(RtdbError::InvalidQuery(
                "limit_to_first, limit_to_last, start_at, end_at, and equal_to all require order_by".to_string(),
            ));
        }

        let mut params = vec![format!("auth={}", self.token)];

        if let Some(ref order) = self.order_by {
            params.push(format!("orderBy={}", order.as_query_param()));
        }
        if let Some(n) = self.limit_to_first {
            params.push(format!("limitToFirst={}", n));
        }
        if let Some(n) = self.limit_to_last {
            params.push(format!("limitToLast={}", n));
        }
        if let Some(ref val) = self.start_at {
            params.push(format!("startAt={}", val.as_query_param()));
        }
        if let Some(ref val) = self.end_at {
            params.push(format!("endAt={}", val.as_query_param()));
        }
        if let Some(ref val) = self.equal_to {
            params.push(format!("equalTo={}", val.as_query_param()));
        }

        Ok(format!(
            "{}/{}.json?{}",
            self.base_url,
            self.path,
            params.join("&")
        ))
    }

    /// Execute the query and return the matched data as a [`serde_json::Value`].
    pub async fn send(self) -> Result<Value, RtdbError> {
        let url = self.build_url()?;

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(RtdbError::Request)?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RtdbError::NotFound(self.path));
        }

        response.json::<Value>().await.map_err(RtdbError::Request)
    }
}

// ── RtdbClient ────────────────────────────────────────────────────────────────

/// A reusable Firebase RTDB client.
///
/// Prefer this over the free functions — it reuses the underlying HTTP client
/// and avoids passing `base_url` and `token` on every call.
///
/// # Example
/// ```no_run
/// # use rtdb_rs::{RtdbClient, RtdbError};
/// # async fn example() -> Result<(), rtdb_rs::RtdbError> {
/// let client = rtdb_rs::RtdbClient::new(
///     "https://my-project.firebaseio.com",
///     "your-oauth2-token",
/// );
///
/// // Simple read
/// let user = client.get("users/alice").await?;
///
/// // Filtered query
/// let recent = client
///     .query("logs")
///     .order_by_child("timestamp")
///     .limit_to_last(50)
///     .send()
///     .await?;
/// # Ok(()) }
/// ```
pub struct RtdbClient {
    base_url: String,
    token: String,
    client: Client,
}

impl RtdbClient {
    /// Create a new client. `base_url` is your project URL,
    /// e.g. `https://my-project.firebaseio.com`.
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
            client: Client::new(),
        }
    }

    /// Update the auth token. Useful when the OAuth2 token is refreshed.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = token.into();
        self
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}.json?auth={}",
            self.base_url,
            path.trim_matches('/'),
            self.token
        )
    }

    /// Read a value at `path`. Returns `null` as `Value::Null` if the node is empty.
    pub async fn get(&self, path: &str) -> Result<Value, RtdbError> {
        let url = self.url(path);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(RtdbError::Request)?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RtdbError::NotFound(path.to_string()));
        }

        response.json::<Value>().await.map_err(RtdbError::Request)
    }

    /// Start a filtered query at `path`. Chain filter methods, then call `.send().await`.
    pub fn query(&self, path: &str) -> GetBuilder<'_> {
        GetBuilder::new(&self.client, &self.base_url, path, &self.token)
    }

    /// Overwrite the value at `path` (HTTP PUT).
    pub async fn put(&self, path: &str, body: &Value) -> Result<Value, RtdbError> {
        let url = self.url(path);
        self.client
            .put(&url)
            .json(body)
            .send()
            .await
            .map_err(RtdbError::Request)?
            .json::<Value>()
            .await
            .map_err(RtdbError::Request)
    }

    /// Update specific fields at `path` without overwriting siblings (HTTP PATCH).
    pub async fn patch(&self, path: &str, body: &Value) -> Result<Value, RtdbError> {
        let url = self.url(path);
        self.client
            .patch(&url)
            .json(body)
            .send()
            .await
            .map_err(RtdbError::Request)?
            .json::<Value>()
            .await
            .map_err(RtdbError::Request)
    }

    /// Append a new child node at `path` with a Firebase-generated push key (HTTP POST).
    /// Returns the generated key wrapped as `{ "name": "-NxPushKey..." }`.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, RtdbError> {
        let url = self.url(path);
        self.client
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(RtdbError::Request)?
            .json::<Value>()
            .await
            .map_err(RtdbError::Request)
    }

    /// Delete the node at `path` (HTTP DELETE).
    pub async fn delete(&self, path: &str) -> Result<(), RtdbError> {
        let url = self.url(path);
        self.client
            .delete(&url)
            .send()
            .await
            .map_err(RtdbError::Request)?;
        Ok(())
    }
}

// ── Types (optional helpers) ──────────────────────────────────────────────────

/// A document returned by the Firebase RTDB REST API.
///
/// Note: these field types mirror a Firestore-style wire format and are
/// provided as optional helpers. Firebase RTDB natively returns plain JSON —
/// using [`serde_json::Value`] directly is often simpler.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RtdbDocument {
    pub name: String,
    pub fields: HashMap<String, RtdbFieldValue>,
}

/// A single field value in a structured document.
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

// ── Free functions (backward compat) ─────────────────────────────────────────

/// Read a value from Firebase RTDB at `path`.
///
/// Consider using [`RtdbClient`] instead — it reuses the HTTP client.
pub async fn get(base_url: &str, path: &str, token: &str) -> Result<Value, RtdbError> {
    RtdbClient::new(base_url, token).get(path).await
}

/// Write (overwrite) a value at `path` using HTTP PUT.
///
/// Consider using [`RtdbClient`] instead — it reuses the HTTP client.
pub async fn put(
    base_url: &str,
    path: &str,
    token: &str,
    body: &Value,
) -> Result<Value, RtdbError> {
    RtdbClient::new(base_url, token).put(path, body).await
}

/// Update specific fields at `path` using HTTP PATCH.
///
/// Consider using [`RtdbClient`] instead — it reuses the HTTP client.
pub async fn patch(
    base_url: &str,
    path: &str,
    token: &str,
    body: &Value,
) -> Result<Value, RtdbError> {
    RtdbClient::new(base_url, token).patch(path, body).await
}

/// Delete the value at `path`.
///
/// Consider using [`RtdbClient`] instead — it reuses the HTTP client.
pub async fn delete(base_url: &str, path: &str, token: &str) -> Result<(), RtdbError> {
    RtdbClient::new(base_url, token).delete(path).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // — RtdbFieldValue constructors —

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

    // — OrderBy —

    #[test]
    fn order_by_key_param() {
        assert_eq!(OrderBy::Key.as_query_param(), "\"$key\"");
    }

    #[test]
    fn order_by_value_param() {
        assert_eq!(OrderBy::Value.as_query_param(), "\"$value\"");
    }

    #[test]
    fn order_by_child_param() {
        assert_eq!(
            OrderBy::Child("created_at".to_string()).as_query_param(),
            "\"created_at\""
        );
    }

    // — FilterValue —

    #[test]
    fn filter_value_string_is_quoted() {
        let v = FilterValue::string("pending");
        assert_eq!(v.as_query_param(), "\"pending\"");
    }

    #[test]
    fn filter_value_number_is_bare() {
        let v = FilterValue::number(42.0);
        assert_eq!(v.as_query_param(), "42");
    }

    #[test]
    fn filter_value_bool_is_bare() {
        let v = FilterValue::boolean(true);
        assert_eq!(v.as_query_param(), "true");
    }

    // — GetBuilder URL construction —

    fn make_builder(path: &str) -> GetBuilder<'static> {
        // We need a static client for test purposes.
        // In real tests, use once_cell or similar for the client.
        // This is a compile-time check only — no HTTP is made.
        static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
        let client = CLIENT.get_or_init(Client::new);
        GetBuilder::new(client, "https://test.firebaseio.com", path, "test-token")
    }

    #[test]
    fn url_simple_get() {
        let url = make_builder("users/alice").build_url().unwrap();
        assert_eq!(
            url,
            "https://test.firebaseio.com/users/alice.json?auth=test-token"
        );
    }

    #[test]
    fn url_with_order_and_limit() {
        let url = make_builder("orders")
            .order_by_child("status")
            .limit_to_last(10)
            .build_url()
            .unwrap();

        assert!(url.contains("orderBy=\"status\""));
        assert!(url.contains("limitToLast=10"));
        assert!(!url.contains("limitToFirst"));
    }

    #[test]
    fn url_limit_to_first_clears_limit_to_last() {
        let url = make_builder("orders")
            .order_by_key()
            .limit_to_last(5)
            .limit_to_first(10)
            .build_url()
            .unwrap();

        assert!(url.contains("limitToFirst=10"));
        assert!(!url.contains("limitToLast"));
    }

    #[test]
    fn url_equal_to_string_is_quoted() {
        let url = make_builder("jobs")
            .order_by_child("status")
            .equal_to(FilterValue::string("active"))
            .build_url()
            .unwrap();

        assert!(url.contains("equalTo=\"active\""));
    }

    #[test]
    fn url_shallow() {
        let url = make_builder("users").shallow().build_url().unwrap();
        assert!(url.contains("shallow=true"));
        assert!(!url.contains("orderBy"));
    }

    #[test]
    fn shallow_with_order_by_is_error() {
        let result = make_builder("users")
            .order_by_key()
            .shallow()
            .build_url();

        assert!(matches!(result, Err(RtdbError::InvalidQuery(_))));
    }

    #[test]
    fn limit_without_order_by_is_error() {
        let result = make_builder("users").limit_to_first(10).build_url();
        assert!(matches!(result, Err(RtdbError::InvalidQuery(_))));
    }

    #[test]
    fn start_at_without_order_by_is_error() {
        let result = make_builder("users")
            .start_at(FilterValue::string("alice"))
            .build_url();
        assert!(matches!(result, Err(RtdbError::InvalidQuery(_))));
    }
}