# RTDB Rust Ecosystem and Offline Sync Plan

## Purpose

This document defines the planned direction for the RTDB Rust crate ecosystem centered on `rtdb-rs`.

The immediate objective is to finish and release a coherent set of focused crates for Firebase Realtime Database, then use `rtdb-sync` as the primary innovation layer. The first major post-release capability should be durable offline synchronization: local snapshots, crash recovery, queued local mutations, reconnect replay, acknowledgement, and explicit conflict handling.

This is a planning document, not a statement that every capability below is already implemented.

## Ecosystem model

```text
application
   |
   +-- rtdb-sync
   |     synchronized application state
   |     reconnect policy
   |     optimistic updates
   |     planned durable offline sync
   |
   +-- rtdb-typed
   |     Serde-first models
   |     typed CRUD and queries
   |     typed realtime events
   |     typed patch semantics
   |
   +-- rtdb-admin
   |     service-account loading
   |     JWT/OAuth exchange
   |     token refresh and rotation
   |     authenticated client integration
   |
   +-- rtdb-rs
         Firebase RTDB REST transport
         queries
         SSE realtime transport
         emulator-safe integration
```

Each crate should remain useful independently. The ecosystem should compose without forcing applications to adopt every layer.

## Responsibility boundaries

### `rtdb-rs`

Owns raw Firebase Realtime Database transport and protocol behavior.

Responsibilities:

- REST CRUD transport
- Firebase query construction
- SSE/realtime transport
- token attachment and transport-level authentication plumbing
- emulator namespace support
- protocol errors and transport diagnostics

Non-goals:

- service-account credential lifecycle
- application model semantics
- local synchronized state
- durable offline mutation storage

### `rtdb-typed`

Owns typed application data semantics on top of `rtdb-rs`.

Responsibilities:

- Serde serialization/deserialization
- typed CRUD
- typed Firebase collections
- typed query results
- nullable/missing-value semantics
- typed realtime events
- partial PATCH representation and application

Non-goals:

- duplicating REST/SSE transport
- service-account lifecycle
- owning long-lived synchronized application state

### `rtdb-admin`

Owns server-side credential and access-token lifecycle.

Responsibilities:

- service-account validation and loading
- RS256 JWT assertions
- Google OAuth token exchange
- expiry tracking
- refresh-before-expiry
- single-flight refresh behavior
- credential rotation
- health and refresh metrics
- authenticated `rtdb-rs` client integration

Non-goals:

- Firebase RTDB transport implementation
- typed model semantics
- synchronized local application state

### `rtdb-sync`

Owns synchronized Rust application state and eventually durable offline operation.

Responsibilities:

- initial hydration
- application of remote PUT/PATCH/null events
- snapshots and watchers
- connection state
- reconnect/backoff behavior
- graceful cancellation and shutdown
- optimistic writes
- write acknowledgement
- rollback on failure
- conflict/echo handling
- planned durable local persistence and offline replay

Non-goals:

- duplicating Firebase REST/SSE implementation
- duplicating typed serialization logic
- owning Google service-account credentials

## Release baseline

Before expanding the ecosystem, the initial coordinated release should establish a stable baseline.

Required release gates for every crate:

- formatting passes
- clippy passes with warnings denied
- tests pass
- documentation builds
- examples compile
- emulator tests pass where applicable
- package contents are inspected
- `cargo package` passes
- `cargo publish --dry-run` passes against the actual crates.io dependency graph
- README claims match the shipped behavior
- crate metadata, repository links, documentation links, license, and minimum Rust version are correct

The dependency order should be respected:

```text
rtdb-rs
   |
   +-- rtdb-typed
   |      |
   |      +-- rtdb-sync
   |
   +-- rtdb-admin
```

The companion crates should not be published until their crates.io dependencies are available and their release gates pass against the published upstream versions.

## Documentation and ecosystem presentation

Once all crates are release-ready, the READMEs should be updated together so the ecosystem is presented consistently on GitHub, crates.io, and docs.rs.

The discovery model should be simple:

```text
Need raw Firebase RTDB REST/SSE access?
-> rtdb-rs

Need typed Serde models and typed realtime events?
-> rtdb-typed

Need service-account authentication and token lifecycle?
-> rtdb-admin

Need synchronized Rust application state?
-> rtdb-sync
```

The `rtdb-rs` README should be the primary ecosystem discovery page, while each companion crate should contain a smaller cross-linked ecosystem section.

Do not advertise planned offline capabilities as shipped until they satisfy their own release gates.

# Major post-release initiative: durable offline synchronization

## Goal

Make `rtdb-sync` capable of maintaining useful application state through network loss and process restarts, while preserving explicit and testable synchronization semantics.

The target experience is:

```text
application writes locally
        |
        v
local state updates immediately
        |
        v
mutation is durably journaled
        |
        +---- network available ----> Firebase RTDB
        |                               |
        |                               v
        |                         acknowledgement
        |                               |
        |                               v
        |                        journal compaction
        |
        +---- network unavailable
                    |
                    v
             continue locally
                    |
             process may exit
                    |
                    v
              application restarts
                    |
                    v
       restore snapshot + pending journal
                    |
              network reconnects
                    |
                    v
             replay pending writes
                    |
                    v
          reconcile remote changes
```

The feature is successful only if crash recovery is deterministic and tested. Network reconnect alone is not enough.

## Phase A: persistent snapshots

Start with durable read-side state before durable writes.

Deliverables:

- persistence abstraction owned by `rtdb-sync`
- in-memory backend for tests
- one durable reference backend
- versioned snapshot format
- atomic snapshot replacement
- startup restore
- invalid/corrupt snapshot detection
- schema/version compatibility policy
- snapshot age and restore metrics

A likely internal abstraction:

```text
SyncStorage
  load_snapshot(key)
  store_snapshot(key, snapshot)
  delete_snapshot(key)
```

The public API should not permanently couple `rtdb-sync` to one storage engine.

Candidate durable backends can be evaluated later. SQLite, redb, or another embedded Rust-native store are reasonable options, but the first implementation should favor correctness, crash safety, portability, and maintenance quality over novelty.

### Acceptance criteria

- application can hydrate from Firebase, persist a snapshot, terminate, and recover the same typed state without network access
- torn/incomplete writes do not produce silently corrupted state
- corrupted snapshots fail explicitly and can fall back to remote hydration when network is available
- snapshot persistence never changes Firebase transport semantics

## Phase B: durable mutation journal

Add append-only durable recording of local mutations.

Each mutation should have enough metadata for deterministic replay.

Possible mutation record fields:

```text
operation_id
sync_path
operation_kind
relative_path
payload
created_at
attempt_count
local_generation
base_remote_generation or conflict token if available
state = pending | inflight | acknowledged | rejected
```

Operation IDs must remain stable across retries and process restarts.

### Acceptance criteria

- local mutation is persisted before it is considered safely queued
- restart preserves all unacknowledged mutations
- acknowledged mutations are compacted without losing ordering information required by later operations
- the same journal can be replayed repeatedly without silently duplicating application intent

## Phase C: offline local writes

Allow callers to mutate synchronized state while disconnected.

Behavior:

1. validate/serialize the operation
2. durably append it to the journal
3. apply the optimistic state transition locally
4. notify watchers/subscribers
5. defer remote transmission until connectivity exists

The API must make it possible to distinguish:

```text
LocalOnly
PendingRemote
Acknowledged
Rejected
Conflicted
```

Do not make local optimism indistinguishable from server acknowledgement.

### Acceptance criteria

- writes succeed locally while the remote connection is unavailable
- pending state is observable
- multiple queued writes preserve deterministic ordering
- deleting and recreating the same path while offline behaves predictably
- PATCH operations retain their partial semantics

## Phase D: reconnect replay and acknowledgement

When connectivity returns, replay pending mutations in a controlled pipeline.

Required behavior:

- bounded concurrency
- deterministic ordering where operations depend on each other
- retry policy with backoff and cancellation
- acknowledgement tracking
- permanent-failure classification
- no infinite hot retry loops
- journal compaction after acknowledgement
- application-visible failure state

A replay engine should be able to answer:

```text
How many operations are pending?
What is the oldest pending operation?
What is currently inflight?
How many retries have occurred?
Why is a mutation blocked?
```

### Acceptance criteria

- disconnect/reconnect cycles do not lose operations
- process restart during replay resumes correctly
- cancellation during backoff exits cleanly
- repeated reconnects do not produce duplicate logical writes

## Phase E: remote reconciliation

Remote Firebase events may arrive while local operations are pending. `rtdb-sync` must reconcile them explicitly rather than relying on accidental last-write behavior.

Required concepts:

- remote generation / observed sequence metadata where available
- local pending generations
- echo detection
- server acknowledgement correlation
- stale remote event handling
- conflict detection

The system should separate three questions:

1. Is this remote event an echo/acknowledgement of our own write?
2. Is this an unrelated remote update that can be merged safely?
3. Is this a true conflict that requires a policy decision?

## Phase F: conflict policies

Initial policies should be explicit and conservative.

Potential built-in policies:

```text
ServerWins
LocalWins
Reject
LastWriteWins
Custom
```

A custom resolver should receive enough context to make a deterministic decision, including local state, remote state, pending operations, and path metadata.

Field-level or CRDT-style merge behavior should not be added until there is a clear application need and a testable semantic model.

### Acceptance criteria

- every conflict produces a deterministic outcome
- conflicts are observable and countable
- no conflict is silently discarded
- custom resolution failures are surfaced
- replay cannot deadlock permanently on one unresolved item without exposing the blocked state

## Phase G: production observability

Offline synchronization should expose operational health.

Metrics/events should eventually include:

- connection state
- reconnect attempts
- current backoff
- pending mutation count
- inflight mutation count
- oldest pending mutation age
- acknowledgements
- rejected writes
- conflict count
- replay count
- replay failures
- snapshot age
- snapshot restore count
- local journal size
- last successful remote event time
- estimated sync lag

The core crate should expose structured data/hooks rather than hard-wire one metrics framework.

## Phase H: multi-path synchronization manager

Once one durable synchronized path is proven, add coordinated management for multiple paths.

```text
SyncManager
  /users
  /jobs
  /devices
  /configuration
  /presence
```

Potential responsibilities:

- shared lifecycle
- coordinated shutdown
- per-path health
- bounded resource use
- aggregate pending counts
- path-level persistence namespaces
- isolation of one failing path from unrelated paths

## Persistence architecture principles

The persistence layer should follow these rules:

1. Durable state is versioned.
2. Journal writes are crash-safe.
3. Operation ordering is explicit.
4. Replaying after a crash is a supported path, not an edge case.
5. Storage corruption is detected, never silently accepted.
6. Storage engines are replaceable behind a narrow abstraction.
7. Persistence does not leak Firebase credentials.
8. Sensitive application payloads are the application's responsibility unless encryption-at-rest is explicitly added later.
9. Storage migration behavior must be documented before format changes are shipped.

## Testing strategy for offline sync

Offline sync should have unusually aggressive deterministic tests.

### Unit and model tests

Test state-machine transitions for:

- pending -> inflight -> acknowledged
- pending -> inflight -> retry
- pending -> rejected
- pending -> conflicted
- restore after crash
- cancellation during backoff
- snapshot corruption
- journal corruption
- compaction
- duplicate acknowledgement
- stale remote event
- remote echo

### Failure injection

Add deterministic fault injection for:

- network unavailable
- connection drops during write
- process termination between journal append and local apply
- process termination after local apply but before remote send
- process termination after remote success but before local acknowledgement persistence
- disk write failure
- partial/corrupt storage
- server rejection
- rate limiting
- malformed remote event
- repeated reconnect churn

### Emulator integration

Use Firebase Realtime Database Emulator for end-to-end correctness tests.

Test profiles should include:

- offline queue then reconnect
- multiple queued PUT/PATCH/DELETE operations
- concurrent local and remote writers
- reconnect during replay
- namespace isolation
- multiple synchronized paths
- server-side changes while local operations are pending

### Stress tests

Stress tests are correctness tests, not capacity claims.

Profiles should include:

- thousands of queued mutations
- many watchers
- repeated reconnect cycles
- multiple independent synchronized paths
- mixed local/remote writers
- long-running replay with intermittent failure
- rapid process restart/recovery loops

## Security considerations

Offline sync introduces local data retention.

Requirements:

- never persist service-account private keys or OAuth tokens in `rtdb-sync`
- document that synchronized application payloads may be stored locally
- allow applications to choose storage location
- avoid leaking payload contents through Debug/error messages
- design storage abstraction so encrypted backends can be added later
- document deletion/compaction guarantees clearly

`rtdb-admin` remains the credential owner. `rtdb-sync` should consume authenticated transport without taking ownership of credential lifecycle.

## API design principles

- async-first and Tokio-compatible
- typed APIs compose with `rtdb-typed`
- transport remains delegated to `rtdb-rs`
- no hidden global runtime
- cancellation must be explicit
- observable state transitions
- deterministic errors
- no implicit conflict policy that can lose data silently
- persistence should be opt-in until it is proven stable
- pre-1.0 evolution should be documented with migration notes

## Framework integrations after the core is stable

Do not put framework-specific dependencies into the core synchronization engine.

Potential adapters or examples:

- Tauri commands/events
- Dioxus signals
- Leptos signals
- Tokio watch/broadcast patterns
- Axum application state
- CLI/daemon examples
- edge/IoT examples

Tauri is a particularly strong target because durable Rust-side state plus Firebase can support desktop applications that remain useful during temporary network loss.

## Testing utilities as a product feature

The ecosystem can differentiate through testing ergonomics.

Potential reusable utilities:

- safe demo-project emulator runner
- ephemeral namespace helpers
- deterministic test fixtures
- simulated disconnect/reconnect
- retry/backoff control
- remote event injection
- write rejection simulation
- conflict scenarios
- latency simulation

Applications should be able to test their synchronization behavior without a production Firebase project.

## Example application strategy

Prefer one serious end-to-end example over many disconnected snippets.

A future example project should demonstrate:

- typed application models
- service-account authentication for server scenarios
- initial hydration
- live remote updates
- optimistic local writes
- durable snapshot restore
- offline mutation queue
- reconnect replay
- conflict handling
- emulator-backed tests

A task board, device dashboard, or field-service style application would exercise the ecosystem well.

## Potential long-term expansion

Only after the Firebase RTDB ecosystem is stable and offline sync is proven should a generic synchronization core be considered.

The reusable concepts may eventually include:

```text
snapshot persistence
mutation journal
optimistic state
acknowledgement
replay
conflict handling
watchers
connection lifecycle
```

If these abstractions become clearly backend-independent, a future architecture could be:

```text
sync-core
   |
   +-- Firebase RTDB adapter
   +-- WebSocket adapter
   +-- REST adapter
   +-- custom application adapter
```

This should not be attempted prematurely. Firebase RTDB should remain the proving ground until the semantics are battle-tested.

## Possible future companion crates

These are ideas, not commitments.

### `rtdb-macros`

Only if repeated real-world boilerplate justifies it.

Potential derive support:

- model/path metadata
- typed collection keys
- schema helpers

Avoid macros that obscure ordinary Rust behavior.

### `rtdb` meta-crate

Only after the public APIs are stable enough that a convenience layer adds value.

Potential features:

```text
typed
admin
sync
```

Advanced users should always be able to depend on individual crates directly.

## Adoption and project growth

Download spikes are useful signals but should not be treated as proof of unique human users. Growth should be measured with multiple indicators:

- crates.io downloads by crate and version
- GitHub stars
- issues and discussions
- forks
- dependent crates
- external examples/blog posts
- repeat download patterns after releases
- which companion crates gain adoption

The goal is to learn which layer developers actually value: transport, typed access, auth, or synchronized state.

## Near-term execution order

1. Finish current work on all companion crates.
2. Freeze responsibility boundaries.
3. Align crates.io dependency versions.
4. Make CI/package/dry-run gates pass against the real release graph.
5. Update all ecosystem READMEs together.
6. Publish `rtdb-rs` first.
7. Publish companion crates in dependency order.
8. Verify docs.rs and crates.io rendering.
9. Observe real-world usage and issues.
10. Begin durable offline sync with persistent snapshots and crash recovery.
11. Add the durable mutation journal.
12. Add offline writes and reconnect replay.
13. Add reconciliation and explicit conflict policies.
14. Add production observability and multi-path management.
15. Only then evaluate framework adapters, a meta-crate, or a backend-independent sync core.

## First offline-sync milestone

The first milestone should be deliberately narrow and high-value:

```text
Persistent Snapshot + Crash Recovery
```

Definition of done:

- synchronized typed state can be persisted atomically
- application can restart without network and recover the last known state
- state format is versioned
- corrupt/incompatible snapshots are detected
- storage is abstracted behind a replaceable backend interface
- tests cover normal restore, crash-like interruption, corruption, and fallback to remote hydration
- no credentials are persisted

This milestone creates the foundation for every later offline capability without prematurely committing to complex conflict semantics.

## Second offline-sync milestone

```text
Durable Mutation Journal
```

Definition of done:

- local operations receive stable IDs
- pending mutations survive process restart
- ordering is deterministic
- operation state is observable
- acknowledgement can safely compact the journal
- replay is idempotent at the synchronization layer as far as Firebase semantics allow
- tests cover interruption at every important persistence/replay boundary

## Third offline-sync milestone

```text
Offline Writes + Reconnect Replay
```

Definition of done:

- application can mutate state with no network
- local state updates optimistically
- pending writes are visible
- reconnect automatically replays the journal
- retries are bounded and cancellable
- acknowledged/rejected outcomes are surfaced
- restart during replay is safe

## Fourth offline-sync milestone

```text
Reconciliation + Conflict Policies
```

Definition of done:

- echoes are distinguished from unrelated remote writes
- true conflicts are detected
- built-in policies are explicit
- custom policy hook is available
- no silent conflict data loss
- conflict metrics and diagnostic events are exposed

## Project identity

The honest long-term positioning should remain focused:

> A modular async Rust ecosystem for Firebase Realtime Database, covering raw REST/SSE transport, Serde-first typed access, service-account authentication, and synchronized application state, with durable offline synchronization as the major state-management direction.

That is broad enough to support a real ecosystem while remaining precise about what the crates actually own.
