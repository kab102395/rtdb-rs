use futures_util::StreamExt;
use rtdb_rs::{FilterValue, RtdbClient, RtdbError, RtdbEvent};
use serde_json::{json, Value};
use std::pin::Pin;
use std::time::Duration;

const BASE_URL: &str = "http://127.0.0.1:9000";
const NAMESPACE_A: &str = "demo-rtdb-rs-a";
const NAMESPACE_B: &str = "demo-rtdb-rs-b";

fn client(namespace: &str) -> RtdbClient {
    RtdbClient::new(BASE_URL, "").with_namespace(namespace)
}

async fn read_event_with_value<S>(
    mut stream: Pin<&mut S>,
    expected: &Value,
) -> Result<RtdbEvent, RtdbError>
where
    S: futures_core::Stream<Item = Result<RtdbEvent, RtdbError>> + ?Sized,
{
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.next().await {
            let event = event?;
            if matches!(&event, RtdbEvent::Put { data, .. } if data == expected)
                || matches!(&event, RtdbEvent::Patch { data, .. } if data == expected)
            {
                return Ok(event);
            }
        }
        Err(RtdbError::Parse(
            "SSE stream ended before expected event".into(),
        ))
    })
    .await
    .map_err(|_| RtdbError::Parse("timed out waiting for SSE event".into()))?
}

#[tokio::test]
#[ignore = "requires the Firebase Realtime Database emulator; run scripts/test-emulator.sh"]
async fn namespaced_emulator_crud_query_and_sse_stress() -> Result<(), RtdbError> {
    let crud_path = "integration/namespace-crud";
    let stream_path = "integration/namespace-stream";

    let first = client(NAMESPACE_A);
    let second = client(NAMESPACE_B);

    first.put(crud_path, &json!({"value": 1})).await?;
    assert_eq!(first.get(crud_path).await?, json!({"value": 1}));
    assert_eq!(second.get(crud_path).await?, Value::Null);

    first
        .patch(crud_path, &json!({"value": 2, "patched": true}))
        .await?;
    assert_eq!(
        first.get(crud_path).await?,
        json!({"value": 2, "patched": true})
    );
    first.delete(crud_path).await?;
    assert_eq!(first.get(crud_path).await?, Value::Null);

    first
        .put("integration/query/a", &json!({"status": "ready", "n": 1}))
        .await?;
    first
        .put("integration/query/b", &json!({"status": "waiting", "n": 2}))
        .await?;
    let filtered = first
        .query("integration/query")
        .order_by_key()
        .equal_to(FilterValue::string("a"))
        .send()
        .await?;
    assert_eq!(filtered, json!({"a": {"status": "ready", "n": 1}}));
    let filtered_url = first
        .query("integration/query")
        .order_by_key()
        .equal_to(FilterValue::string("a"))
        .build_url()?;
    assert!(filtered_url.contains("ns=demo-rtdb-rs-a"));

    let custom_url = RtdbClient::new(BASE_URL, "")
        .with_namespace("demo namespace/encoded")
        .with_query_param("auth_variable_override", r#"{"uid":"test user"}"#)
        .query("users/profile")
        .build_url()?;
    assert!(custom_url.contains("ns=demo%20namespace%2Fencoded"));
    assert!(custom_url.contains("auth_variable_override=%7B%22uid%22%3A%22test%20user%22%7D"));
    assert!(!custom_url.contains("auth="));

    let stream = client(NAMESPACE_A).stream(stream_path).await?;
    tokio::pin!(stream);
    let initial = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .map_err(|_| RtdbError::Parse("timed out waiting for initial SSE Put".into()))?
        .ok_or_else(|| RtdbError::Parse("SSE stream ended before initial Put".into()))??;
    assert!(matches!(initial, RtdbEvent::Put { data, .. } if data == Value::Null));

    client(NAMESPACE_A)
        .put(stream_path, &json!({"value": 7}))
        .await?;
    let mutation = read_event_with_value(stream.as_mut(), &json!({"value": 7})).await?;
    assert!(matches!(mutation, RtdbEvent::Put { .. }));

    let fanout_count = 6;
    let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel(fanout_count);
    let mut fanout = Vec::with_capacity(fanout_count);
    for index in 0..fanout_count {
        let ready_tx = ready_tx.clone();
        fanout.push(tokio::spawn(async move {
            let path = format!("integration/fanout/{index}");
            let stream = client(NAMESPACE_A).stream(&path).await?;
            tokio::pin!(stream);
            ready_tx.send(()).await.map_err(|_| {
                RtdbError::Parse("fan-out readiness channel closed unexpectedly".into())
            })?;
            let expected = json!({"fanout": index});
            read_event_with_value(stream.as_mut(), &expected).await
        }));
    }
    drop(ready_tx);
    for _ in 0..fanout_count {
        tokio::time::timeout(Duration::from_secs(10), ready_rx.recv())
            .await
            .map_err(|_| RtdbError::Parse("timed out waiting for SSE fan-out readiness".into()))?
            .ok_or_else(|| RtdbError::Parse("fan-out readiness channel closed".into()))?;
    }
    for index in 0..fanout_count {
        client(NAMESPACE_A)
            .put(
                &format!("integration/fanout/{index}"),
                &json!({"fanout": index}),
            )
            .await?;
    }
    for task in fanout {
        let event = task
            .await
            .map_err(|error| RtdbError::Parse(error.to_string()))??;
        assert!(matches!(event, RtdbEvent::Put { .. }));
    }

    let stress_count = 20;
    let jobs = (0..stress_count).map(|index| async move {
        let path = format!("integration/stress/{index}");
        let worker = client(NAMESPACE_B);
        worker.put(&path, &json!({"step": 1})).await?;
        worker
            .patch(&path, &json!({"step": 2, "index": index}))
            .await?;
        assert_eq!(worker.get(&path).await?, json!({"step": 2, "index": index}));
        worker.delete(&path).await
    });
    futures_util::future::try_join_all(jobs).await?;

    client(NAMESPACE_A).delete("integration/query/a").await?;
    client(NAMESPACE_A).delete("integration/query/b").await?;
    client(NAMESPACE_A).delete(stream_path).await?;
    Ok(())
}
