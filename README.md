# rtdb-rs

A Rust client for the [Firebase Realtime Database](https://firebase.google.com/docs/database) REST API.

Built to solve a specific problem: connecting a Rust backend to Firebase RTDB using 
service account JWT authentication. Existing crates either use simpler auth models 
or focus on Firestore rather than RTDB. This crate handles the full 
service account → JWT → OAuth2 token → RTDB REST flow.
## Features

- Service account authentication via JWT + OAuth2
- Read, write, patch, and delete operations against any RTDB path
- Fully async via `tokio`
- Typed error handling via `thiserror`
- Zero unsafe code

## Install

Add to your `Cargo.toml`:

```toml
[dependencies]
rtdb-rs = "0.1.0"
tokio = { version = "1", features = ["full"] }
serde_json = "1.0"
```

## Quickstart

```rust
use firebase_rtdb::{generate_jwt, exchange_jwt_for_access_token, get, put, patch, delete};

#[tokio::main]
async fn main() {
    // Load credentials from environment variables
    let private_key = std::env::var("FIREBASE_PRIVATE_KEY").unwrap();
    let client_email = std::env::var("FIREBASE_CLIENT_EMAIL").unwrap();
    let base_url = std::env::var("FIREBASE_BASE_URL").unwrap();
    // e.g. https://your-project.firebaseio.com

    // Authenticate
    let jwt = generate_jwt(&private_key, &client_email).await.unwrap();
    let token = exchange_jwt_for_access_token(&jwt).await.unwrap();

    // Write a value
    let body = serde_json::json!({
        "name": "Alice",
        "active": true
    });
    put(&base_url, "users/alice", &token, &body).await.unwrap();

    // Read it back
    let data = get(&base_url, "users/alice", &token).await.unwrap();
    println!("{}", data);

    // Update a specific field without overwriting the whole node
    let update = serde_json::json!({ "active": false });
    patch(&base_url, "users/alice", &token, &update).await.unwrap();

    // Delete
    delete(&base_url, "users/alice", &token).await.unwrap();
}
```

## Auth Setup

This crate authenticates using a Firebase service account. To get your credentials:

1. Go to the [Firebase Console](https://console.firebase.google.com)
2. Project Settings → Service Accounts
3. Generate a new private key — this downloads a JSON file
4. Extract the `private_key` and `client_email` fields from that JSON

Set them as environment variables:

```bash
FIREBASE_PRIVATE_KEY="-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----\n"
FIREBASE_CLIENT_EMAIL="firebase-adminsdk-xxxxx@your-project.iam.gserviceaccount.com"
FIREBASE_BASE_URL="https://your-project.firebaseio.com"
```

> **Never commit your private key to source control.** Use a `.env` file locally
> and environment variables in production. Add `*.pem` and `.env` to your `.gitignore`.

## API

### Auth

```rust
// Generate a signed JWT from your service account credentials
pub async fn generate_jwt(private_key: &str, client_email: &str) -> Result<String, RtdbError>

// Exchange the JWT for an OAuth2 access token
pub async fn exchange_jwt_for_access_token(jwt: &str) -> Result<String, RtdbError>
```

### CRUD

```rust
// Read a value at path
pub async fn get(base_url: &str, path: &str, token: &str) -> Result<Value, RtdbError>

// Write (overwrite) a value at path
pub async fn put(base_url: &str, path: &str, token: &str, body: &Value) -> Result<Value, RtdbError>

// Update specific fields at path without overwriting siblings
pub async fn patch(base_url: &str, path: &str, token: &str, body: &Value) -> Result<Value, RtdbError>

// Delete the value at path
pub async fn delete(base_url: &str, path: &str, token: &str) -> Result<(), RtdbError>
```

### Error handling

```rust
use firebase_rtdb::RtdbError;

match get(&base_url, "users/alice", &token).await {
    Ok(data) => println!("{}", data),
    Err(RtdbError::NotFound(path)) => println!("Not found: {}", path),
    Err(RtdbError::Auth(msg)) => println!("Auth error: {}", msg),
    Err(RtdbError::Request(e)) => println!("HTTP error: {}", e),
    Err(RtdbError::Parse(msg)) => println!("Parse error: {}", msg),
}
```

## Types

`RtdbFieldValue` provides typed constructors for building field values:

```rust
use firebase_rtdb::RtdbFieldValue;

let s = RtdbFieldValue::string("hello");
let i = RtdbFieldValue::integer(42);
let b = RtdbFieldValue::boolean(true);
```

## Status

`v0.1.0` — auth and CRUD operations tested against Firebase RTDB.
Integration test suite and additional helpers planned for future releases.

Contributions welcome — open an issue or PR on [GitHub](https://github.com/kab102395/firebase-rtdb-rs).

## Why this exists

Built to solve a specific problem: connecting a Rust backend to Firebase Realtime 
Database using service account JWT authentication. Existing crates either used 
simpler auth models or focused on Firestore rather than RTDB. This crate handles 
the full service account → JWT → OAuth2 token → RTDB REST flow.

## License

MIT — see [LICENSE](LICENSE)
