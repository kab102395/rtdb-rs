# rtdb-rs

A Firebase Realtime Database REST client for Rust.

Supports service account authentication (JWT RS256 + OAuth2), a full
query builder for filtering, ordering, and paginating results, and covers
the real-world gotchas you will hit using the Firebase REST API.

[![Crates.io](https://img.shields.io/crates/v/rtdb-rs.svg)](https://crates.io/crates/rtdb-rs)
[![Docs.rs](https://docs.rs/rtdb-rs/badge.svg)](https://docs.rs/rtdb-rs)

---

## Installation

```toml
[dependencies]
rtdb-rs = "0.2"
```

---

## Finding Your Database URL

Your Firebase RTDB base URL follows this pattern:

```
https://<project-id>-default-rtdb.firebaseio.com
```

You can confirm it in the **Firebase Console → Realtime Database → Data tab**.
The URL is shown at the top of the data tree. If your project ID is
`my-app-12345`, your URL is:

```
https://my-app-12345-default-rtdb.firebaseio.com
```

---

## Authentication

Firebase RTDB requires a valid OAuth2 access token on every request.
Tokens are generated from a **service account JSON key**, which you download
from **Firebase Console → Project Settings → Service Accounts →
Generate New Private Key**.

### Generating a token

```rust
use rtdb_rs::{generate_jwt, exchange_jwt_for_access_token};

// private_key: the "private_key" field from your service account JSON
// client_email: the "client_email" field from your service account JSON
let jwt = generate_jwt(&private_key, &client_email).await?;
let token = exchange_jwt_for_access_token(&jwt).await?;
```

### Token expiry — important

OAuth2 tokens expire after **1 hour**. In long-running applications you must
re-generate the token before it expires, otherwise all requests will fail
with an auth error. Call `generate_jwt` + `exchange_jwt_for_access_token`
again and update the client:

```rust
let new_token = exchange_jwt_for_access_token(
    &generate_jwt(&private_key, &client_email).await?
).await?;

let client = client.with_token(&new_token);
```

---

## Basic Usage

Create a reusable `RtdbClient` with your database URL and token. The client
reuses the underlying HTTP connection across all requests — prefer it over
the free functions.

```rust
use rtdb_rs::RtdbClient;
use serde_json::json;

let client = RtdbClient::new(
    "https://my-app-12345-default-rtdb.firebaseio.com",
    &token,
);

// Write — overwrites the entire node
client.put("users/alice", &json!({
    "name": "Alice",
    "score": 95,
    "active": true
})).await?;

// Read
let user = client.get("users/alice").await?;
println!("{}", user["name"]); // "Alice"

// Update specific fields without overwriting siblings
client.patch("users/alice", &json!({ "score": 100 })).await?;

// Append a new child with a Firebase-generated push key
// Returns { "name": "-NxGeneratedKey..." }
client.post("logs", &json!({ "event": "login" })).await?;

// Delete
client.delete("users/alice").await?;
```

### Missing nodes return null, not an error

Firebase returns JSON `null` for a path that does not exist — it does not
return an HTTP 404. This means `get()` succeeds and gives you `Value::Null`:

```rust
let val = client.get("users/does_not_exist").await?;
assert!(val.is_null()); // true — no error raised, just null
```

`RtdbError::NotFound` is only returned if Firebase sends an actual HTTP 404,
which is rare. Do not rely on it for existence checks — use `.is_null()`
instead.

---

## Query Builder

Use `client.query(path)` to filter, sort, and paginate results. Chain filter
methods and call `.send().await` to execute.

### Order by child field

```rust
use rtdb_rs::FilterValue;

let results = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .send()
    .await?;
```

### Order by key

```rust
let first_ten = client
    .query("users")
    .order_by_key()
    .limit_to_first(10)
    .send()
    .await?;
```

### Order by value

Use this when nodes are primitives rather than objects:

```rust
let top_five = client
    .query("scores")
    .order_by_value()
    .limit_to_last(5)
    .send()
    .await?;
```

### Range filtering with start_at and end_at

```rust
let range = client
    .query("events")
    .order_by_child("timestamp")
    .start_at(FilterValue::number(1_700_000_000.0))
    .end_at(FilterValue::number(1_800_000_000.0))
    .send()
    .await?;
```

### Shallow reads

Returns only the keys at a path, not the full values. Useful for existence
checks or counting nodes without downloading all data.

```rust
let keys = client.query("users").shallow().send().await?;
// Returns { "alice": true, "bob": true, "carol": true }
```

`shallow` cannot be combined with `order_by`, `limit`, or filter params —
`rtdb-rs` will return `RtdbError::InvalidQuery` before sending if you try.

### Debugging a query before sending

`build_url()` is public so you can inspect the full URL that will be sent,
which is useful when a query is not returning what you expect:

```rust
let url = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .limit_to_first(25)
    .build_url()?;

println!("{}", url);
// https://my-app.firebaseio.com/orders.json?auth=...&orderBy="status"&equalTo="pending"&limitToFirst=25
```

---

## FilterValue

Firebase encodes filter values differently depending on type. Strings must
be JSON-quoted in the URL; numbers and booleans must be bare. `FilterValue`
handles this automatically — do not manually quote values.

| Constructor               | URL wire format |
|---------------------------|-----------------|
| `FilterValue::string(s)`  | `"quoted"`      |
| `FilterValue::number(n)`  | `42` (bare)     |
| `FilterValue::boolean(b)` | `true` (bare)   |
| `FilterValue::Null`       | `null`          |

```rust
// These produce correctly encoded URLs:
.equal_to(FilterValue::string("active"))   // equalTo="active"
.equal_to(FilterValue::number(90.0))       // equalTo=90
.equal_to(FilterValue::boolean(false))     // equalTo=false
.equal_to(FilterValue::Null)               // equalTo=null
```

---

## OrderBy

| Variant             | Firebase equivalent | Use when                          |
|---------------------|---------------------|-----------------------------------|
| `OrderBy::Key`      | `"$key"`            | Ordering by push key              |
| `OrderBy::Value`    | `"$value"`          | Nodes are primitives, not objects |
| `OrderBy::Priority` | `"$priority"`       | Using Firebase priority field     |
| `OrderBy::Child(f)` | `"fieldName"`       | Ordering by a child field         |

`order_by_key()`, `order_by_value()`, and `order_by_child()` are shorthand
methods. You can also pass `OrderBy` directly via `.order_by(OrderBy::Priority)`.

---

## Firebase Index Rules — Required for order_by_child

> **This will silently fail without the correct database rules. Read this.**

When you use `order_by_child("field")`, Firebase requires that field to be
declared as an index in your **Database Rules**. Without it, Firebase returns
an error object with HTTP 200 — meaning your Rust code sees `Ok(...)` but the
value contains an error string instead of your data:

```
Ok(Object {"error": String("Index not defined, add \".indexOn\": \"score\",
for path \"/your_path\", to the rules")})
```

This is a Firebase server-side constraint, not a bug in your code or in
`rtdb-rs`. The HTTP status is 200 so `rtdb-rs` cannot catch it automatically
— you must check the response value yourself (see Error Handling below).

To fix it, go to **Firebase Console → Realtime Database → Rules** and add
`.indexOn` for every field you intend to query:

```json
{
  "rules": {
    ".read": true,
    ".write": true,
    "your_path": {
      ".indexOn": ["field_one", "field_two", "field_three"]
    }
  }
}
```

For example, if you query a `users` node by `name`, `score`, and `active`:

```json
{
  "rules": {
    ".read": true,
    ".write": true,
    "users": {
      ".indexOn": ["name", "score", "active"]
    }
  }
}
```

Click **Publish** after saving. Changes take effect immediately.

> **`order_by_key()`, `order_by_value()`, and `shallow()` do not require
> indexes.** Firebase always indexes keys natively. Only `order_by_child()`
> triggers this requirement.

---

## Query Validation Rules

`rtdb-rs` validates query parameters before sending and returns
`RtdbError::InvalidQuery` immediately on violations, rather than letting
Firebase reject the request with a vague error:

- `limit_to_first`, `limit_to_last`, `start_at`, `end_at`, and `equal_to`
  all require `order_by` to be set first
- `shallow` cannot be combined with `order_by`, `limit_to_first`,
  `limit_to_last`, `start_at`, `end_at`, or `equal_to`
- `limit_to_first` and `limit_to_last` are mutually exclusive — setting one
  clears the other automatically

---

## Error Handling

Firebase sometimes returns errors as JSON objects with HTTP 200 (notably the
missing index error above). Always check `Ok` values for an `"error"` key
when using `order_by_child`:

```rust
use rtdb_rs::RtdbError;

match client.query("users").order_by_child("score").send().await {
    Ok(value) => {
        // Check for Firebase error objects returned with HTTP 200
        if let Some(err) = value.get("error") {
            eprintln!("Firebase error: {}", err);
            // Likely cause: missing .indexOn rule in database rules
        } else {
            println!("{}", value);
        }
    }
    Err(RtdbError::NotFound(path))    => eprintln!("not found: {}", path),
    Err(RtdbError::Auth(msg))         => eprintln!("auth error: {}", msg),
    Err(RtdbError::InvalidQuery(msg)) => eprintln!("bad query: {}", msg),
    Err(RtdbError::Parse(msg))        => eprintln!("parse error: {}", msg),
    Err(RtdbError::Request(e))        => eprintln!("http error: {}", e),
}
```

---

## Free Functions (v0.1 Compatibility)

The original free functions from v0.1 are still available. They now delegate
to `RtdbClient` internally — behavior is unchanged, but each call creates a
new HTTP client. Prefer `RtdbClient` in new code.

```rust
use rtdb_rs::{get, put, patch, delete};

let value = get(&base_url, "users/alice", &token).await?;
put(&base_url, "users/alice", &token, &body).await?;
patch(&base_url, "users/alice", &token, &body).await?;
delete(&base_url, "users/alice", &token).await?;
```

---

## Changelog

### 0.2.0
- Added `RtdbClient` — reusable HTTP client with token management via `with_token()`
- Added `GetBuilder` query builder via `client.query()`
- Added `OrderBy` enum — `Key`, `Value`, `Priority`, `Child`
- Added `FilterValue` enum — `string`, `number`, `boolean`, `Null`
- Added `client.post()` for Firebase push-key appends
- Added `RtdbError::InvalidQuery` with pre-send validation
- Made `build_url()` public for query debugging
- Free functions now delegate to `RtdbClient` (no behavior change)

### 0.1.0
- Initial release: `generate_jwt`, `exchange_jwt_for_access_token`
- Free functions: `get`, `put`, `patch`, `delete`

---

## License

MIT
