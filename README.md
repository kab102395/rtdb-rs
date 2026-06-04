# rtdb-rs

Firebase Realtime Database REST client for Rust. Handles auth, CRUD, queries, and real-time SSE streaming.

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

Get a service account JSON key from **Firebase Console → Project Settings → Service Accounts**.

```rust
use rtdb_rs::{generate_jwt, exchange_jwt_for_access_token};

let jwt = generate_jwt(&private_key, &client_email).await?;
let token = exchange_jwt_for_access_token(&jwt).await?;
```

Tokens expire after **1 hour**. Call `with_token()` to refresh:

```rust
let client = client.with_token(&new_token);
```

---

## Basic Usage

Your database URL is `https://<project-id>-default-rtdb.firebaseio.com`.

```rust
use rtdb_rs::RtdbClient;
use serde_json::json;

let client = RtdbClient::new("https://my-project-default-rtdb.firebaseio.com", &token);

client.put("users/alice", &json!({ "name": "Alice", "score": 95 })).await?;

let user = client.get("users/alice").await?;

client.patch("users/alice", &json!({ "score": 100 })).await?;

client.post("logs", &json!({ "event": "login" })).await?; // Firebase push key

client.delete("users/alice").await?;
```

> Missing nodes return `Value::Null`, not an error.

---

## Queries

```rust
use rtdb_rs::FilterValue;

// Filter
let results = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .limit_to_first(25)
    .send()
    .await?;

// Range
let range = client
    .query("events")
    .order_by_child("timestamp")
    .start_at(FilterValue::number(1_700_000_000.0))
    .end_at(FilterValue::number(1_800_000_000.0))
    .send()
    .await?;

// Keys only
let keys = client.query("users").shallow().send().await?;
```

`FilterValue` types: `string`, `number`, `boolean`, `Null`.

`OrderBy` options: `order_by_child("field")`, `order_by_key()`, `order_by_value()`.

> **`order_by_child` requires `.indexOn` rules in your Firebase database rules or it silently returns an error object with HTTP 200.** Add indexes at **Firebase Console → Realtime Database → Rules**:
> ```json
> { "rules": { "users": { ".indexOn": ["name", "score"] } } }
> ```

---

## SSE Streaming

Firebase pushes real-time updates over an open HTTP connection.

```rust
use futures_util::StreamExt;

// Simple stream
let mut stream = client.stream("users/alice").await?;
tokio::pin!(stream); // required — AsyncStream is !Unpin

// Filtered stream
let mut stream = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .stream()
    .await?;
tokio::pin!(stream); // required — AsyncStream is !Unpin

while let Some(event) = stream.next().await {
    match event? {
        RtdbEvent::Put { path, data }   => println!("put at {}: {}", path, data),
        RtdbEvent::Patch { path, data } => println!("patch at {}: {}", path, data),
        RtdbEvent::KeepAlive            => {}
        RtdbEvent::Cancel               => break, // token expired — re-auth and reconnect
    }
}
```

The first event is always a `Put` with the full current value. Subsequent events reflect changes as they happen.
---

## Debugging

`build_url()` is public — inspect the URL before sending if something isn't working:

```rust
let url = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .build_url()?;

println!("{}", url);
```

---

## Errors

```rust
match client.get("users/alice").await {
    Ok(v) if v.get("error").is_some() => eprintln!("Firebase error: {}", v["error"]),
    Ok(v)  => println!("{}", v),
    Err(RtdbError::Auth(e))            => eprintln!("auth: {}", e),
    Err(RtdbError::InvalidQuery(e))    => eprintln!("bad query: {}", e),
    Err(e)                             => eprintln!("error: {}", e),
}
```

> Firebase can return errors as JSON with HTTP 200 — always check for `"error"` in `Ok` responses when using `order_by_child`.

---

## Changelog

### 0.3.0
- SSE streaming via `client.stream()` and `query().stream()`
- `RtdbEvent` enum: `Put`, `Patch`, `KeepAlive`, `Cancel`

### 0.2.0
- `RtdbClient` with reusable HTTP connection
- Query builder: `order_by_child`, `limit_to_first/last`, `start_at`, `end_at`, `equal_to`, `shallow`
- `FilterValue` and `OrderBy` enums
- `post()` for Firebase push keys
- `RtdbError::InvalidQuery` with pre-send validation
- `build_url()` public for debugging

### 0.1.0
- `generate_jwt`, `exchange_jwt_for_access_token`
- `get`, `put`, `patch`, `delete`

---

## License

MIT
