//! Connector transport tests against a local server. They never contact a provider.

mod mock_provider;

use mock_provider::{Mock, action_stream, serve, serve_sequence, sse};
use serde_json::{Value, json};
use sha2::Digest;
use std::io::Write;
use std::process::{Command, Stdio};

fn request() -> Value {
    json!({
        "model": "gpt-6-luna",
        "decoding": {"temperature": 0, "max_output_tokens": 64, "reasoning_effort": "none"},
        "arm": "A",
        "schema": {"response": "exactly one JSON action", "actions": [{"type": "read"}, {"type": "finish"}]},
        "messages": [
            {"role": "system", "content": "SYSTEM RULES"},
            {"role": "user", "content": {"requirement": "do it", "input_paths": ["input.csv"]}},
            {"role": "assistant", "content": "{\"type\":\"read\"}"},
            {"role": "tool", "content": "bytes"}
        ],
        "limits": {},
        "remaining_wall_seconds": 30.0,
        "synthetic": false
    })
}

fn run(port: u16, request: &Value) -> (std::process::Output, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tooldelegate"))
        .arg("luna-connector")
        .env("LUNA_ACCESS_TOKEN", "test-token")
        .env("LUNA_ACCOUNT_ID", "test-account")
        .env("LUNA_BASE_URL", format!("http://127.0.0.1:{port}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let parsed = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (output, parsed)
}

#[test]
fn maps_conversation_and_records_provider_usage() {
    let response = json!({
        "id": "resp_test",
        "model": "gpt-6-luna",
        "status": "completed",
        "usage": {
            "input_tokens": 120,
            "output_tokens": 8,
            "input_tokens_details": {"cached_tokens": 64}
        },
        "output": []
    });
    let (port, server) = serve(sse(&[
        json!({"type": "response.created"}),
        json!({"type": "response.output_text.delta", "delta": "{\"type\":"}),
        json!({"type": "response.output_text.delta", "delta": "\"finish\"}"}),
        json!({"type": "response.completed", "response": response}),
    ]));
    let (output, reply) = run(port, &request());
    let received = server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(reply["action"], "{\"type\":\"finish\"}");
    assert_eq!(reply["model"], "gpt-6-luna");
    assert_eq!(
        reply["decoding"],
        json!({"temperature": 0, "max_output_tokens": 64, "reasoning_effort": "none"})
    );
    assert_eq!(reply["finish_reason"], "stop");
    assert_eq!(reply["usage"]["input_tokens"], 120);
    assert_eq!(reply["usage"]["output_tokens"], 8);
    assert_eq!(reply["usage"]["cached_input_tokens"], 64);
    assert_eq!(reply["usage"]["source"], "api_reported");
    assert_eq!(reply["provider_response"]["usage"]["input_tokens"], 120);

    assert!(received.starts_with("POST /responses HTTP/1.1"));
    assert!(
        received.contains("Authorization: Bearer test-token"),
        "connector must send the broker credential"
    );
    assert!(received.contains("chatgpt-account-id: test-account"));
    assert!(received.contains("x-codex-beta-features: remote_compaction_v2"));
    let body: Value = serde_json::from_str(received.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body["instructions"],
        "SYSTEM RULES\n\nAction schema (JSON):\n{\"actions\":[{\"type\":\"read\"},{\"type\":\"finish\"}],\"response\":\"exactly one JSON action\"}"
    );
    assert_eq!(body["model"], "gpt-6-luna");
    assert_eq!(body["tools"], json!([]));
    assert!(body.get("include").is_none());
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["max_output_tokens"], 64);
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(
        body["input"][0]["content"][0]["text"],
        json!({"requirement": "do it", "input_paths": ["input.csv"]}).to_string()
    );
    assert_eq!(body["input"][1]["role"], "assistant");
    assert_eq!(body["input"][2]["role"], "user");
    assert_eq!(body["input"][2]["content"][0]["text"], "bytes");
}

#[test]
fn aborts_runaway_provider_stream() {
    let (port, server) = serve(Mock {
        status: "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n",
        headers: "",
        body: "x".repeat(2_200_000),
    });
    let start = std::time::Instant::now();
    let (output, reply) = run(port, &request());
    server.join().unwrap();
    assert!(!output.status.success());
    let error = reply["error"].as_str().unwrap();
    assert!(error.contains("byte cap"), "{error}");
    assert!(start.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn refuses_response_without_provider_usage() {
    let (port, server) = serve(sse(&[json!({
        "type": "response.completed",
        "response": {"id": "resp_test", "model": "gpt-6-luna", "status": "completed", "output": [{"type": "message", "content": [{"type": "output_text", "text": "{\"type\":\"finish\"}"}]}]}
    })]));
    let (output, reply) = run(port, &request());
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(
        reply["error"].as_str().unwrap().contains("input_tokens"),
        "{reply}"
    );
}

#[test]
fn refuses_unpinned_resolved_model() {
    let (port, server) = serve(sse(&[json!({
        "type": "response.completed",
        "response": {"id": "resp_test", "model": "gpt-6-other", "status": "completed", "usage": {"input_tokens": 1, "output_tokens": 1}, "output": []}
    })]));
    let (output, reply) = run(port, &request());
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(
        reply["error"].as_str().unwrap().contains("does not match"),
        "{reply}"
    );
}

#[test]
fn surfaces_quota_rejection_without_usage() {
    let body = json!({"error": {"type": "usage_limit_reached", "message": "The usage limit has been reached", "resets_at": 1794868701}}).to_string();
    let (port, server) = serve(Mock {
        status: "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\n",
        headers: "",
        body,
    });
    let (output, reply) = run(port, &request());
    server.join().unwrap();
    assert!(!output.status.success());
    let error = reply["error"].as_str().unwrap();
    assert!(error.contains("usage_limit_reached"), "{error}");
    assert!(error.contains("resets_at=1794868701"), "{error}");
}

const INPUT: &str = "id,value,secondary,payload\n0,2,0,x\n1,1,0,y\n2,2,0,z\n3,,0,q\n";
const GOOD: &str = "id,value,secondary,payload\n1,1,0,y\n0,2,0,x\n2,2,0,z\n3,,0,q\n";

#[test]
fn connector_runs_through_the_orchestrator_with_api_usage() {
    use std::fs;
    use std::path::Path;
    let root = std::env::temp_dir().join(format!(
        "luna-connector-run-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("task/input")).unwrap();
    fs::write(root.join("task/input/input.csv"), INPUT).unwrap();
    fs::write(
        root.join("task/manifest.json"),
        json!({
            "id": "synthetic-connector", "family": "sorting", "variant": 0,
            "expected_outcome": "success",
            "requirement": "Sort input.csv by signed numeric value ascending, nulls last, stable ties; write output.csv and preserve original input.",
            "input_hashes": {"input.csv": format!("{:x}", sha2::Sha256::digest(INPUT.as_bytes()))}
        })
        .to_string(),
    )
    .unwrap();
    let config: Value = serde_json::from_str(include_str!("../configs/run.luna.json")).unwrap();
    assert_eq!(config["model"], "gpt-6-luna");
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    fs::write(
        root.join("connector.json"),
        json!({"program": env!("CARGO_BIN_EXE_tooldelegate"), "args": ["luna-connector"]}).to_string(),
    )
    .unwrap();
    let actions = [
        json!({"type":"read","path":"input.csv","offset":0,"bytes":32}),
        json!({"type":"put","path":"output.csv","content":&GOOD[..30],"append":false}),
        json!({"type":"put","path":"output.csv","content":&GOOD[30..],"append":true}),
        json!({"type":"submit","commands":null}),
        json!({"type":"finish"}),
    ];
    let (port, server) = serve_sequence(
        actions
            .iter()
            .map(|action| action_stream(&action.to_string()))
            .collect(),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tooldelegate"))
        .args([
            "connector-run",
            root.join("task").to_str().unwrap(),
            root.join("run").to_str().unwrap(),
            "A",
            root.join("config.json").to_str().unwrap(),
            root.join("connector.json").to_str().unwrap(),
        ])
        .env("LUNA_ACCESS_TOKEN", "test-token")
        .env("LUNA_ACCOUNT_ID", "test-account")
        .env("LUNA_BASE_URL", format!("http://127.0.0.1:{port}"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), actions.len());
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["synthetic"], false);
    assert_eq!(summary["success"], true);
    assert_eq!(summary["oracle_pass"], true);
    assert_eq!(summary["metrics"]["calls"], actions.len());
    assert_eq!(summary["metrics"]["total_tokens"], actions.len() * 110);
    assert_eq!(summary["metrics"]["cached_input_tokens"], 0);
    assert_eq!(summary["metrics"]["context_peak"], 110);
    assert_eq!(
        fs::read(root.join("run/work/output.csv")).unwrap(),
        GOOD.as_bytes()
    );
    let trace = fs::read_to_string(root.join("run/trace.jsonl")).unwrap();
    assert!(trace.contains("api_reported"));
    assert!(!trace.contains("test-token"));
    assert!(Path::new(&root.join("run/record.jsonl")).exists());
    fs::remove_dir_all(root).unwrap();
}
