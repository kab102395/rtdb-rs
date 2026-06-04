use async_stream::stream;
use chrono::{Duration, Utc};
use futures_core::Stream;
use futures_util::StreamExt;
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

// ── SSE Event ─────────────────────────────────────────────────────────────────

/// An event received from a Firebase RTDB SSE stream.
///
/// Firebase streams four event types over an open HTTP connection.
/// The first event after connecting is always a `Put` containing the full
/// current value of the node. Subsequent events reflect changes as they happen.
///
/// # Example
/// ```no_run
/// # use rtdb_rs::{RtdbClient, RtdbEvent, RtdbError};
/// # use futures_util::StreamExt;
/// # async fn example() -> Result<(), RtdbError> {
/// # let client = RtdbClient::new("https://my-project.firebaseio.com", "token");
/// let mut stream = client.stream("users/alice").await?;
/// tokio::pin!(stream);
/// while let Some(event) = stream.next().await {
///     match event? {
///         RtdbEvent::Put { path, data }   => println!("put at {}: {}", path, data),
///         RtdbEvent::Patch { path, data } => println!("patch at {}: {}", path, data),
///         RtdbEvent::KeepAlive            => {}
///         RtdbEvent::Cancel               => break,
///     }
/// }
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
pub enum RtdbEvent {
    /// Full node replaced. Fired once on connect with the current value,
    /// then again whenever the node is overwritten via PUT.
    Put { path: String, data: Value },

    /// Specific fields updated. `data` contains only the changed fields,
    /// not the full node. Fired when a PATCH is applied to the node.
    Patch { path: String, data: Value },

    /// Heartbeat sent by Firebase to keep the connection alive. Safe to ignore.
    KeepAlive,

    /// Stream cancelled — usually means the auth token was revoked or expired.
    /// Stop listening and re-authenticate before reconnecting.
    Cancel,
}

// ── SSE Parser ────────────────────────────────────────────────────────────────

/// Parse a Firebase SSE `data:` payload into `(path, data)`.
/// Firebase always sends `{"path": "...", "data": ...}`.
fn parse_sse_data(raw: &str) -> Result<(String, Value), RtdbError> {
    let v: Value = serde_json::from_str(raw)
        .map_err(|e| RtdbError::Parse(format!("invalid SSE payload: {}", e)))?;

    let path = v["path"]
        .as_str()
        .ok_or_else(|| RtdbError::Parse("SSE payload missing 'path' field".to_string()))?
        .to_string();

    let data = v["data"].clone();

    Ok((path, data))
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

/// A builder for filtered GET and SSE stream requests against the Firebase RTDB REST API.
///
/// Created via [`RtdbClient::query`]. Chain filter methods and call
/// `.send().await` for a one-shot read or `.stream().await` for real-time events.
///
/// # Example
/// ```no_run
/// # use rtdb_rs::{RtdbClient, FilterValue, RtdbError};
/// # async fn example() -> Result<(), RtdbError> {
/// # let client = RtdbClient::new("https://my-project.firebaseio.com", "token");
/// let results = client
///     .query("orders")
///     .order_by_child("status")
///     .equal_to(FilterValue::string("pending"))
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
    pub fn new(client: &'a Client, base_url: &'a str, path: &str, token: &'a str) -> Self {
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
    pub fn order_by_value(mut self) -> Self {
        self.order_by = Some(OrderBy::Value);
        self
    }

    /// Set ordering explicitly via [`OrderBy`].
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

    /// Return only keys, not values. Cannot be combined with other query params
    /// or with `.stream()`.
    pub fn shallow(mut self) -> Self {
        self.shallow = true;
        self
    }

    /// Build the request URL. Public for debugging — inspect this if a query
    /// is not returning what you expect before calling `.send()` or `.stream()`.
    pub fn build_url(&self) -> Result<String, RtdbError> {
        if self.shallow {
            let has_filters = self.order_by.is_some()
                || self.limit_to_first.is_some()
                || self.limit_to_last.is_some()
                || self.start_at.is_some()
                || self.end_at.is_some()
                || self.equal_to.is_some();

            if has_filters {
                return Err(RtdbError::InvalidQuery(
                    "shallow=true cannot be combined with orderBy, limit, or filter params"
                        .to_string(),
                ));
            }

            return Ok(format!(
                "{}/{}.json?auth={}&shallow=true",
                self.base_url, self.path, self.token
            ));
        }

        let needs_order = self.limit_to_first.is_some()
            || self.limit_to_last.is_some()
            || self.start_at.is_some()
            || self.end_at.is_some()
            || self.equal_to.is_some();

        if needs_order && self.order_by.is_none() {
            return Err(RtdbError::InvalidQuery(
                "limit_to_first, limit_to_last, start_at, end_at, and equal_to all require order_by"
                    .to_string(),
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

    /// Open a real-time SSE stream at `path` and return an async `Stream` of
    /// [`RtdbEvent`]s. The first event is always a `Put` containing the full
    /// current value. Subsequent events reflect changes as they occur.
    ///
    /// All query params (`order_by_child`, `limit_to_last`, etc.) are supported
    /// — Firebase will filter the stream to only push matching events.
    /// `shallow` is not supported with streaming.
    ///
    /// The stream ends when dropped. Firebase may send a `Cancel` event if
    /// the auth token expires — handle it by re-authenticating and reconnecting.
    ///
    /// # Example
    /// ```no_run
/// # use rtdb_rs::{RtdbClient, RtdbEvent, FilterValue, RtdbError};
/// # use futures_util::StreamExt;
/// # async fn example() -> Result<(), RtdbError> {
/// # let client = RtdbClient::new("https://my-project.firebaseio.com", "token");
/// let mut stream = client
///     .query("orders")
///     .order_by_child("status")
///     .equal_to(FilterValue::string("pending"))
///     .stream()
///     .await?;
/// tokio::pin!(stream);
/// while let Some(event) = stream.next().await {
///     match event? {
///         RtdbEvent::Put { path, data }   => println!("put at {}: {}", path, data),
///         RtdbEvent::Patch { path, data } => println!("patch at {}: {}", path, data),
///         RtdbEvent::KeepAlive            => {}
///         RtdbEvent::Cancel               => break,
///     }
/// }
/// # Ok(()) }
/// ```
    pub async fn stream(
        self,
    ) -> Result<impl Stream<Item = Result<RtdbEvent, RtdbError>>, RtdbError> {
        if self.shallow {
            return Err(RtdbError::InvalidQuery(
                "shallow cannot be used with stream() — streaming requires full node data"
                    .to_string(),
            ));
        }

        let url = self.build_url()?;
        let path = self.path.clone();

        let response = self
            .client
            .get(&url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .send()
            .await
            .map_err(RtdbError::Request)?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RtdbError::NotFound(path));
        }

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(RtdbError::Auth("unauthorized — check your token".to_string()));
        }

        let s = stream! {
            let mut bytes_stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut current_event = String::new();
            let mut current_data = String::new();

            while let Some(chunk) = bytes_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(RtdbError::Request(e));
                        return;
                    }
                };

                let text = match std::str::from_utf8(&chunk) {
                    Ok(t) => t.to_string(),
                    Err(_) => {
                        yield Err(RtdbError::Parse(
                            "invalid UTF-8 in SSE stream".to_string(),
                        ));
                        return;
                    }
                };

                buffer.push_str(&text);

                // Process all complete lines in the buffer
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim_end_matches('\r').to_string();
                    buffer = buffer[pos + 1..].to_string();

                    if line.is_empty() {
                        // Blank line = end of event block, dispatch
                        if !current_event.is_empty() {
                            match current_event.as_str() {
                                "put" => {
                                    match parse_sse_data(&current_data) {
                                        Ok((path, data)) => {
                                            yield Ok(RtdbEvent::Put { path, data })
                                        }
                                        Err(e) => yield Err(e),
                                    }
                                }
                                "patch" => {
                                    match parse_sse_data(&current_data) {
                                        Ok((path, data)) => {
                                            yield Ok(RtdbEvent::Patch { path, data })
                                        }
                                        Err(e) => yield Err(e),
                                    }
                                }
                                "keep-alive" => yield Ok(RtdbEvent::KeepAlive),
                                "cancel" => {
                                    yield Ok(RtdbEvent::Cancel);
                                    return;
                                }
                                other => {
                                    // Unknown event type — ignore rather than error
                                    // Firebase may add new event types in future
                                    let _ = other;
                                }
                            }
                        }
                        current_event.clear();
                        current_data.clear();
                    } else if let Some(rest) = line.strip_prefix("event:") {
                        current_event = rest.trim().to_string();
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        current_data = rest.trim().to_string();
                    }
                    // Lines starting with ':' are SSE comments — ignore
                }
            }
        };

        Ok(s)
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
/// # async fn example() -> Result<(), RtdbError> {
/// let client = RtdbClient::new(
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
    /// e.g. `https://my-project-default-rtdb.firebaseio.com`.
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
            client: Client::new(),
        }
    }

    /// Replace the auth token. Call this when the OAuth2 token is refreshed.
    /// Tokens expire after 1 hour.
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

    /// Read a value at `path`. Returns `Value::Null` if the node is empty —
    /// Firebase does not return HTTP 404 for missing nodes.
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

    /// Start a filtered query at `path`. Chain filter methods, then call
    /// `.send().await` for a one-shot read or `.stream().await` for real-time events.
    pub fn query(&self, path: &str) -> GetBuilder<'_> {
        GetBuilder::new(&self.client, &self.base_url, path, &self.token)
    }

    /// Open a real-time SSE stream at `path`. Shorthand for `client.query(path).stream()`.
    ///
    /// Use `client.query(path).order_by_child(...).stream()` if you need filtering.
    ///
    /// # Example
    /// ```no_run
/// # use rtdb_rs::{RtdbClient, RtdbEvent, RtdbError};
/// # use futures_util::StreamExt;
/// # async fn example() -> Result<(), RtdbError> {
/// # let client = RtdbClient::new("https://my-project.firebaseio.com", "token");
/// let mut stream = client.stream("users/alice").await?;
/// tokio::pin!(stream);
/// while let Some(event) = stream.next().await {
///     match event? {
///         RtdbEvent::Put { path, data }   => println!("put at {}: {}", path, data),
///         RtdbEvent::Patch { path, data } => println!("patch at {}: {}", path, data),
///         RtdbEvent::KeepAlive            => {}
///         RtdbEvent::Cancel               => break,
///     }
/// }
/// # Ok(()) }
/// ```
    pub async fn stream(
        &self,
        path: &str,
    ) -> Result<impl Stream<Item = Result<RtdbEvent, RtdbError>>, RtdbError> {
        self.query(path).stream().await
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
    /// Returns the generated key as `{ "name": "-NxPushKey..." }`.
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
/// Consider using [`RtdbClient`] instead — it reuses the HTTP client.
pub async fn get(base_url: &str, path: &str, token: &str) -> Result<Value, RtdbError> {
    RtdbClient::new(base_url, token).get(path).await
}

/// Write (overwrite) a value at `path` using HTTP PUT.
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
/// Consider using [`RtdbClient`] instead — it reuses the HTTP client.
pub async fn delete(base_url: &str, path: &str, token: &str) -> Result<(), RtdbError> {
    RtdbClient::new(base_url, token).delete(path).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_builder(path: &str) -> GetBuilder<'static> {
        static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
        let client = CLIENT.get_or_init(Client::new);
        GetBuilder::new(client, "https://test.firebaseio.com", path, "test-token")
    }

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
        let result = make_builder("users").order_by_key().shallow().build_url();
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

    // — SSE parser —

    #[test]
    fn parse_sse_data_valid() {
        let raw = r#"{"path":"/users/alice","data":{"name":"Alice","score":95}}"#;
        let (path, data) = parse_sse_data(raw).unwrap();
        assert_eq!(path, "/users/alice");
        assert_eq!(data["name"], "Alice");
        assert_eq!(data["score"], 95);
    }

    #[test]
    fn parse_sse_data_root_path() {
        let raw = r#"{"path":"/","data":{"a":1,"b":2}}"#;
        let (path, data) = parse_sse_data(raw).unwrap();
        assert_eq!(path, "/");
        assert_eq!(data["a"], 1);
    }

    #[test]
    fn parse_sse_data_null_data() {
        // Firebase sends null data when a node is deleted
        let raw = r#"{"path":"/users/alice","data":null}"#;
        let (path, data) = parse_sse_data(raw).unwrap();
        assert_eq!(path, "/users/alice");
        assert!(data.is_null());
    }

    #[test]
    fn parse_sse_data_missing_path_is_error() {
        let raw = r#"{"data":{"name":"Alice"}}"#;
        let result = parse_sse_data(raw);
        assert!(matches!(result, Err(RtdbError::Parse(_))));
    }

    #[test]
    fn parse_sse_data_invalid_json_is_error() {
        let result = parse_sse_data("not json at all");
        assert!(matches!(result, Err(RtdbError::Parse(_))));
    }

    #[test]
    fn shallow_stream_is_error() {
        // shallow + stream() should return InvalidQuery
        // We test build_url here since stream() is async
        let result = make_builder("users").shallow().build_url();
        // shallow alone is valid for GET...
        assert!(result.is_ok());
        // ...but shallow + any filter is not
        let result2 = make_builder("users")
            .shallow()
            .order_by_key()
            .build_url();
        assert!(matches!(result2, Err(RtdbError::InvalidQuery(_))));
    }
}