# rtdb-rs

Firebase Realtime Database REST client for Rust. Handles service account auth, CRUD operations, filtered queries, push keys, and real-time SSE streaming over the Firebase REST API.

[![Crates.io](https://img.shields.io/crates/v/rtdb-rs.svg)](https://crates.io/crates/rtdb-rs)
[![Docs.rs](https://docs.rs/rtdb-rs/badge.svg)](https://docs.rs/rtdb-rs)

---

## Installation

```toml
[dependencies]
rtdb-rs = "0.3"
```

---

## Auth

Get a service account JSON key from:

**Firebase Console → Project Settings → Service Accounts**

Then generate a signed JWT and exchange it for a Google OAuth2 access token:

```rust
use rtdb_rs::{generate_jwt, exchange_jwt_for_access_token};

let jwt = generate_jwt(&private_key, &client_email).await?;
let token = exchange_jwt_for_access_token(&jwt).await?;
```

Create a reusable client:

```rust
use rtdb_rs::RtdbClient;

let client = RtdbClient::new(
    "https://my-project-default-rtdb.firebaseio.com",
    &token,
);
```

OAuth2 access tokens expire after about **1 hour**. Refresh the token and call `with_token()` when needed:

```rust
let client = client.with_token(&new_token);
```

`rtdb-rs` supports both common Firebase REST token styles:

* Google OAuth2 access tokens, usually beginning with `ya29`, are sent using `access_token=...`.
* Firebase ID tokens and other token styles are sent using `auth=...`.

For a local Realtime Database emulator, set the namespace explicitly. An empty
token omits authentication, which is appropriate when emulator rules allow
public access:

```rust
let client = RtdbClient::new("http://127.0.0.1:9000", "")
    .with_namespace("demo-rtdb-typed");
```

---

## Basic Usage

Your database URL should look like this:

```text
https://<project-id>-default-rtdb.firebaseio.com
```

Example CRUD usage:

```rust
use rtdb_rs::RtdbClient;
use serde_json::json;

let client = RtdbClient::new(
    "https://my-project-default-rtdb.firebaseio.com",
    &token,
);

// PUT: overwrite a node
client
    .put("users/alice", &json!({
        "name": "Alice",
        "score": 95
    }))
    .await?;

// GET: read a node
let user = client.get("users/alice").await?;

// PATCH: update specific fields without removing siblings
client
    .patch("users/alice", &json!({
        "score": 100
    }))
    .await?;

// POST: create a Firebase push-key child
let pushed = client
    .post("logs", &json!({
        "event": "login"
    }))
    .await?;

// DELETE: remove a node
client.delete("users/alice").await?;
```

Missing nodes return `serde_json::Value::Null`, not a `NotFound` error.

---

## Queries

Use `client.query(path)` to build Firebase REST queries.

```rust
use rtdb_rs::FilterValue;

// Filter by child value
let results = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .limit_to_first(25)
    .send()
    .await?;

// Range query
let range = client
    .query("events")
    .order_by_child("timestamp")
    .start_at(FilterValue::number(1_700_000_000.0))
    .end_at(FilterValue::number(1_800_000_000.0))
    .send()
    .await?;

// Keys only
let keys = client
    .query("users")
    .shallow()
    .send()
    .await?;
```

Supported filter values:

```rust
FilterValue::string("pending")
FilterValue::number(42.0)
FilterValue::boolean(true)
FilterValue::Null
```

Supported ordering methods:

```rust
.order_by_child("field")
.order_by_key()
.order_by_value()
.order_by(OrderBy::Priority)
```

Firebase requires `order_by` before `limit_to_first`, `limit_to_last`, `start_at`, `end_at`, or `equal_to`. `rtdb-rs` validates this before sending the request and returns `RtdbError::InvalidQuery` for invalid combinations.

### Indexing rules

For production use, Firebase recommends indexing any child fields used with `order_by_child`.

Example Firebase Realtime Database rules:

```json
{
  "rules": {
    "orders": {
      ".indexOn": ["status", "timestamp"]
    },
    "users": {
      ".indexOn": ["name", "score", "active"]
    }
  }
}
```

`order_by_key()` does not require `.indexOn`.

---

## SSE Streaming

Firebase Realtime Database supports real-time updates over Server-Sent Events through the REST API. `rtdb-rs` exposes this through `client.stream(path)` and `query(...).stream()`.

```rust
use futures_util::StreamExt;
use rtdb_rs::RtdbEvent;

// Simple stream
let stream = client.stream("users/alice").await?;
tokio::pin!(stream);

while let Some(event) = stream.next().await {
    match event? {
        RtdbEvent::Put { path, data } => {
            println!("put at {}: {}", path, data);
        }
        RtdbEvent::Patch { path, data } => {
            println!("patch at {}: {}", path, data);
        }
        RtdbEvent::KeepAlive => {
            // Safe to ignore.
        }
        RtdbEvent::Cancel => {
            // Token expired, permission changed, or stream was cancelled.
            // Re-authenticate and reconnect.
            break;
        }
    }
}
```

The first event is normally a `Put` containing the current value at the streamed path. If the node is empty, the first `Put` contains `null`.

Subsequent events reflect changes:

* `Put` means the streamed node or child path was replaced.
* `Patch` means fields were updated without replacing the full node.
* `KeepAlive` is a Firebase heartbeat.
* `Cancel` means the stream was cancelled by Firebase.

### Filtered streams

Queries can also be streamed:

```rust
use futures_util::StreamExt;
use rtdb_rs::{FilterValue, RtdbEvent};

let stream = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .stream()
    .await?;

tokio::pin!(stream);

while let Some(event) = stream.next().await {
    match event? {
        RtdbEvent::Put { path, data } => println!("put at {}: {}", path, data),
        RtdbEvent::Patch { path, data } => println!("patch at {}: {}", path, data),
        RtdbEvent::KeepAlive => {}
        RtdbEvent::Cancel => break,
    }
}
```

Filtered streams follow the same indexing requirements as normal Firebase queries. If you use `order_by_child("status")`, add `.indexOn: ["status"]` at the matching database path.

For tests or simple filtered streaming without `.indexOn`, prefer `order_by_key()`:

```rust
let stream = client
    .query("orders")
    .order_by_key()
    .equal_to(FilterValue::string("order_2"))
    .stream()
    .await?;
```

---

## Debugging

`build_url()` is public so you can inspect the exact Firebase REST URL before sending a request:

```rust
let url = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .build_url()?;

println!("{}", url);
```

Query parameters are percent-encoded. For example:

```text
orderBy=%22status%22
equalTo=%22pending%22
```

This is expected. The encoded values represent Firebase’s required JSON-style query syntax:

```text
orderBy="status"
equalTo="pending"
```

---

## Errors

Common error handling pattern:

```rust
use rtdb_rs::RtdbError;

match client.get("users/alice").await {
    Ok(v) => {
        if let Some(error) = v.get("error") {
            eprintln!("Firebase error: {}", error);
        } else {
            println!("{}", v);
        }
    }
    Err(RtdbError::Auth(e)) => {
        eprintln!("auth error: {}", e);
    }
    Err(RtdbError::InvalidQuery(e)) => {
        eprintln!("invalid query: {}", e);
    }
    Err(RtdbError::NotFound(path)) => {
        eprintln!("not found: {}", path);
    }
    Err(e) => {
        eprintln!("request failed: {}", e);
    }
}
```

Firebase missing nodes usually return JSON `null`. They do not normally produce HTTP 404.

---

## Live Testing

A separate live test harness can be used against a real Firebase Realtime Database project.

Set:

```text
RTDB_BASE_URL
RTDB_PRIVATE_KEY
RTDB_CLIENT_EMAIL
```

Example:

```bash
RTDB_BASE_URL="https://my-project-default-rtdb.firebaseio.com" \
RTDB_PRIVATE_KEY="-----BEGIN PRIVATE KEY-----..." \
RTDB_CLIENT_EMAIL="service-account@my-project.iam.gserviceaccount.com" \
cargo run
```

On Windows PowerShell:

```powershell
$env:RTDB_BASE_URL="https://my-project-default-rtdb.firebaseio.com"
$env:RTDB_PRIVATE_KEY="-----BEGIN PRIVATE KEY-----..."
$env:RTDB_CLIENT_EMAIL="service-account@my-project.iam.gserviceaccount.com"
cargo run
```

The live harness validates:

* Auth token generation and exchange
* GET, PUT, PATCH, POST, DELETE
* Filtered queries
* Shallow queries
* SSE initial `Put`
* SSE `Put` after write
* SSE `Patch` after patch
* SSE delete as `Put` with `null`
* Child-path streams
* Sequential stream events
* Stream reconnect behavior
* Large streamed payloads

---

## Changelog

### 0.3.1

* Fixed Firebase SSE authentication for Google OAuth2 access tokens by using `access_token=...`.
* Preserved `auth=...` behavior for Firebase ID tokens and other token styles.
* Added percent-encoding for auth tokens and query parameters.
* Improved URL construction for filtered GET and SSE requests.
* Updated tests to expect encoded Firebase query parameters.
* Verified live SSE behavior against Firebase Realtime Database.

### 0.3.0

* Added SSE streaming via `client.stream()` and `query().stream()`.
* Added `RtdbEvent` enum:

  * `Put`
  * `Patch`
  * `KeepAlive`
  * `Cancel`

### 0.2.0

* Added `RtdbClient` with reusable HTTP connection.
* Added query builder:

  * `order_by_child`
  * `order_by_key`
  * `order_by_value`
  * `limit_to_first`
  * `limit_to_last`
  * `start_at`
  * `end_at`
  * `equal_to`
  * `shallow`
* Added `FilterValue` and `OrderBy` enums.
* Added `post()` for Firebase push keys.
* Added `RtdbError::InvalidQuery` with pre-send validation.
* Made `build_url()` public for debugging.

### 0.1.0

* Added service account JWT generation.
* Added JWT-to-access-token exchange.
* Added basic REST helpers:

  * `get`
  * `put`
  * `patch`
  * `delete`

---

## License

MIT
