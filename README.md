# rtdb-rs

Async Firebase Realtime Database REST and SSE transport for Rust.

[![Crates.io](https://img.shields.io/crates/v/rtdb-rs.svg)](https://crates.io/crates/rtdb-rs)
[![Docs.rs](https://docs.rs/rtdb-rs/badge.svg)](https://docs.rs/rtdb-rs)

`rtdb-rs` is the transport foundation of a small Rust ecosystem for Firebase Realtime Database. It provides reusable HTTP clients, CRUD operations, Firebase query construction, push-key writes, namespaced emulator support, and realtime Server-Sent Events without imposing an ORM or application-state model.

The current 0.3.2 release line also carries the transport capabilities required by the companion typed, admin, and synchronization crates.

## RTDB Rust ecosystem

```text
application
   |
   +-- rtdb-sync
   |     synchronized state, durable snapshots, offline journal,
   |     reconnect/replay, reconciliation, conflict policy
   |
   +-- rtdb-typed
   |     Serde models, typed CRUD, collections, queries, realtime events
   |
   +-- rtdb-admin
   |     service-account loading, OAuth exchange, token lifecycle
   |
   `-- rtdb-rs
         Firebase REST + query + SSE transport
                  |
                  v
          Firebase Realtime Database
```

Each crate has a deliberately narrow responsibility:

| Crate | Responsibility |
| --- | --- |
| [`rtdb-rs`](https://github.com/kab102395/rtdb-rs) | Raw Firebase RTDB REST, query, push-key, namespace, and SSE transport |
| [`rtdb-typed`](https://github.com/kab102395/rtdb-typed) | Serde-first typed models, collections, queries, patches, and realtime events |
| [`rtdb-admin`](https://github.com/kab102395/rtdb-admin) | Service-account credentials, OAuth exchange, expiry, refresh, and authenticated client lifecycle |
| [`rtdb-sync`](https://github.com/kab102395/rtdb-sync) | Realtime synchronized Rust state, local writes, reconnect, durability, offline replay, and reconciliation |

Use only the layers an application needs. `rtdb-rs` can be used by itself for maximum transport control. The companion crates build on it rather than duplicating its HTTP or SSE implementation.

## Installation

```toml
[dependencies]
rtdb-rs = "0.3.2"
```

## Client

```rust
use rtdb_rs::RtdbClient;

let client = RtdbClient::new(
    "https://my-project-default-rtdb.firebaseio.com",
    &token,
);
```

For long-running service-account applications, prefer [`rtdb-admin`](https://github.com/kab102395/rtdb-admin) for credential loading and automatic token lifecycle management. `rtdb-rs` retains its lower-level JWT/OAuth helpers for callers that want to manage authentication directly.

```rust
use rtdb_rs::{exchange_jwt_for_access_token, generate_jwt};

let jwt = generate_jwt(&private_key, &client_email).await?;
let token = exchange_jwt_for_access_token(&jwt).await?;
```

OAuth2 access tokens are short-lived. A refreshed token can be applied with:

```rust
let client = client.with_token(&new_token);
```

Google OAuth2 access tokens are sent using `access_token=...`; Firebase ID tokens and other token styles use `auth=...`.

## CRUD

```rust
use rtdb_rs::RtdbClient;
use serde_json::json;

let client = RtdbClient::new(
    "https://my-project-default-rtdb.firebaseio.com",
    &token,
);

client.put("users/alice", &json!({
    "name": "Alice",
    "score": 95
})).await?;

let user = client.get("users/alice").await?;

client.patch("users/alice", &json!({
    "score": 100
})).await?;

let pushed = client.post("logs", &json!({
    "event": "login"
})).await?;

client.delete("users/alice").await?;
```

Firebase missing nodes normally deserialize as `serde_json::Value::Null` rather than producing an HTTP 404.

## Queries

`client.query(path)` builds Firebase REST queries while preserving the client namespace and persistent query parameters.

```rust
use rtdb_rs::FilterValue;

let results = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .limit_to_first(25)
    .send()
    .await?;

let range = client
    .query("events")
    .order_by_child("timestamp")
    .start_at(FilterValue::number(1_700_000_000.0))
    .end_at(FilterValue::number(1_800_000_000.0))
    .send()
    .await?;

let keys = client
    .query("users")
    .shallow()
    .send()
    .await?;
```

Supported ordering includes child, key, value, and priority ordering. Invalid Firebase query combinations are rejected before the request with `RtdbError::InvalidQuery`.

For production queries using `order_by_child`, configure the matching Firebase `.indexOn` rule.

## Realtime SSE

Firebase Realtime Database exposes realtime changes over Server-Sent Events. `rtdb-rs` supports both direct-path and filtered-query streams.

```rust
use futures_util::StreamExt;
use rtdb_rs::RtdbEvent;

let stream = client.stream("users/alice").await?;
tokio::pin!(stream);

while let Some(event) = stream.next().await {
    match event? {
        RtdbEvent::Put { path, data } => {
            println!("put at {path}: {data}");
        }
        RtdbEvent::Patch { path, data } => {
            println!("patch at {path}: {data}");
        }
        RtdbEvent::KeepAlive => {}
        RtdbEvent::Cancel => break,
    }
}
```

The first stream event is normally a `Put` containing the current value. `Put` represents replacement/deletion, `Patch` represents partial updates, `KeepAlive` is the Firebase heartbeat, and `Cancel` terminates the stream.

Filtered streams use the same query builder:

```rust
let stream = client
    .query("orders")
    .order_by_child("status")
    .equal_to(FilterValue::string("pending"))
    .stream()
    .await?;
```

## Emulator namespaces and persistent parameters

For the local Firebase Realtime Database emulator, an empty token is supported when emulator rules permit unauthenticated access.

```rust
let client = RtdbClient::new("http://127.0.0.1:9000", "")
    .with_namespace("demo-rtdb-test");
```

`with_namespace()` is propagated through CRUD, queries, shallow reads, and SSE requests. This allows multiple isolated logical databases to share one emulator process.

Persistent parameters can be attached to every request:

```rust
let client = RtdbClient::new("http://127.0.0.1:9000", "")
    .with_namespace("demo-rtdb-test")
    .with_query_param("auth_variable_override", r#"{"uid":"test-user"}"#);
```

Namespace names, parameter names, values, auth values, and Firebase query values are percent-encoded.

## Typed, admin, and synchronized usage

Applications that do not want to work directly with `serde_json::Value` can use [`rtdb-typed`](https://github.com/kab102395/rtdb-typed), which maps the same transport into typed Serde models, collections, queries, `TypedPatch`, and typed realtime events.

Server-side applications can use [`rtdb-admin`](https://github.com/kab102395/rtdb-admin) to own service-account credentials, concurrent refresh, expiry handling, and authenticated `RtdbClient` replacement.

Applications that need maintained realtime state can use [`rtdb-sync`](https://github.com/kab102395/rtdb-sync). Its 0.4.0 line adds opt-in durable snapshots, persistent pending mutations, process-restart recovery, offline queueing, replay on reconnect, acknowledgement durability, explicit conflict policy, and synchronized typed state while delegating Firebase transport back to this crate.

## Validation

The repository contains deterministic tests plus an official local Firebase Realtime Database Emulator harness covering:

- namespaced CRUD and namespace isolation
- filtered and shallow queries
- query/auth URL encoding
- empty-token emulator requests
- SSE initial state and subsequent `Put`/`Patch`/delete delivery
- child-path and filtered streams
- SSE fan-out
- concurrent CRUD stress

Run the emulator suite with:

```bash
./scripts/test-emulator.sh
```

The runner accepts only `demo-*` project IDs and refuses to start when required emulator ports are already occupied.

The wider four-crate ecosystem has also been exercised together in `rtdb-sync` against the local emulator with mixed raw/typed/admin/sync traffic, real local sync writes, active subscribers, token refresh/client replacement, durable offline process-restart replay, concurrent remote writes during replay, repeated connection-boundary testing, and long-duration soak profiles. Those measurements are local correctness/stress evidence, not universal Firebase production-capacity guarantees.

## 0.3.2

The 0.3.2 line adds the transport capabilities needed by the companion ecosystem:

- `RtdbClient::with_namespace()` for Firebase emulator namespaces
- `RtdbClient::with_query_param()` for persistent REST/SSE parameters
- propagation of namespace and persistent parameters through CRUD, query, shallow, and SSE paths
- omission of auth parameters for empty-token emulator clients
- expanded official emulator and concurrency coverage
- CI gates for formatting, Clippy, tests, and package validation

## Scope

`rtdb-rs` targets Firebase Realtime Database. It is not a Firestore, Storage, FCM, Remote Config, Functions deployment, or full Firebase Admin SDK replacement.

## License

MIT
