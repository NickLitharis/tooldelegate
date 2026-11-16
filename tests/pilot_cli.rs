//! Pilot planning and resumed execution against a local provider double.

mod mock_provider;

use mock_provider::{Mock, action_stream, serve_mocks, serve_sequence};
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const INPUT: &str = "id,value,secondary,payload\n0,2,0,x\n1,1,0,y\n2,2,0,z\n3,,0,q\n";
const GOOD: &str = "id,value,secondary,payload\n1,1,0,y\n0,2,0,x\n2,2,0,z\n3,,0,q\n";
const SORT: &str = "import csv\nwith open('input.csv',newline='') as f: rows=list(csv.reader(f))\nh=rows.pop(0);rows.sort(key=lambda r:(r[1]=='',int(r[1] or 0)))\nwith open('output.csv','w',newline='') as f:\n w=csv.writer(f);w.writerow(h);w.writerows(rows)";

fn setup() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pilot-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let dataset = root.join("dataset");
    fs::create_dir_all(dataset.join("synthetic-task/input")).unwrap();
    fs::write(dataset.join("synthetic-task/input/input.csv"), INPUT).unwrap();
    fs::write(
        dataset.join("synthetic-task/manifest.json"),
        json!({
            "id": "synthetic-task", "family": "sorting", "variant": 0,
            "expected_outcome": "success",
            "requirement": "Sort input.csv by signed numeric value ascending, nulls last, stable ties; write output.csv and preserve original input.",
            "input_hashes": {"input.csv": format!("{:x}", sha2::Sha256::digest(INPUT.as_bytes()))}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dataset.join("dataset.json"),
        json!({"tasks":["synthetic-task"],"seed":2026}).to_string(),
    )
    .unwrap();
    fs::write(
        root.join("selection.json"),
        json!({"tasks":["synthetic-task"]}).to_string(),
    )
    .unwrap();
    fs::write(
        root.join("connector.json"),
        json!({"program": env!("CARGO_BIN_EXE_verified-cli-benchmark"), "args": ["luna-connector"]})
            .to_string(),
    )
    .unwrap();
    let config: Value = serde_json::from_str(include_str!("../configs/run.luna.json")).unwrap();
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    root
}

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_verified-cli-benchmark"))
}

fn plan(root: &Path, pilot: &Path) -> Output {
    binary()
        .args([
            "pilot-plan",
            root.join("dataset").to_str().unwrap(),
            root.join("selection.json").to_str().unwrap(),
            pilot.to_str().unwrap(),
            "7",
            "1",
        ])
        .output()
        .unwrap()
}

fn pilot_run(root: &Path, pilot: &Path, port: u16) -> Output {
    binary()
        .args([
            "pilot-run",
            root.join("dataset").to_str().unwrap(),
            pilot.to_str().unwrap(),
            root.join("config.json").to_str().unwrap(),
            root.join("connector.json").to_str().unwrap(),
        ])
        .env("LUNA_ACCESS_TOKEN", "test-token")
        .env("LUNA_ACCOUNT_ID", "test-account")
        .env("LUNA_BASE_URL", format!("http://127.0.0.1:{port}"))
        .output()
        .unwrap()
}

fn actions(arm: &str) -> Vec<Value> {
    let read = json!({"type":"read","path":"input.csv","offset":0,"bytes":32});
    let mut actions = vec![read];
    if arm == "A" {
        actions
            .push(json!({"type":"put","path":"output.csv","content":&GOOD[..30],"append":false}));
        actions.push(json!({"type":"put","path":"output.csv","content":&GOOD[30..],"append":true}));
        actions.push(json!({"type":"submit","commands":null}));
    } else {
        actions.push(json!({"type":"submit","commands":[["python3","-I","-c",SORT]]}));
    }
    actions.push(json!({"type":"finish"}));
    actions
}

#[test]
fn pilot_plan_runs_all_arms_and_resumes_without_repeating() {
    let root = setup();
    let pilot = root.join("pilot");
    let output = plan(&root, &pilot);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: Value =
        serde_json::from_slice(&fs::read(pilot.join("plan.json")).unwrap()).unwrap();
    let runs = parsed["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 4);
    assert_eq!(
        runs.iter()
            .map(|run| run["arm"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        ["A", "B", "C", "D"].into_iter().collect()
    );
    let bodies = runs
        .iter()
        .flat_map(|run| {
            actions(run["arm"].as_str().unwrap())
                .iter()
                .map(|action| action_stream(&action.to_string()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let expected_calls = bodies.len();
    let (port, server) = serve_sequence(bodies);
    let output = pilot_run(&root, &pilot, port);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let received = server.join().unwrap();
    assert_eq!(received.len(), expected_calls);
    let index = fs::read_to_string(pilot.join("index.jsonl")).unwrap();
    let entries = index
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 4);
    for (entry, planned) in entries.iter().zip(runs) {
        assert_eq!(entry["status"], "completed");
        assert_eq!(entry["task_id"], planned["task_id"]);
        assert_eq!(entry["arm"], planned["arm"]);
        assert_eq!(entry["summary"]["success"], true);
        assert_eq!(entry["summary"]["oracle_pass"], true);
        assert_eq!(
            entry["summary"]["metrics"]["total_tokens"],
            if planned["arm"] == "A" { 550 } else { 330 }
        );
    }
    let summary: Value =
        serde_json::from_slice(&fs::read(pilot.join("pilot-summary.json")).unwrap()).unwrap();
    assert_eq!(summary["runs"], 4);
    assert_eq!(summary["planned"], 4);
    assert_eq!(summary["remaining"], 0);
    assert_eq!(summary["success"], 4);
    assert_eq!(summary["incomplete"], 0);
    assert_eq!(summary["usage_incomplete"], 0);
    assert_eq!(summary["by_arm"]["A"]["calls"], 5);
    assert_eq!(summary["by_arm"]["D"]["calls"], 3);
    assert_eq!(
        fs::read_to_string(pilot.join("index.jsonl")).unwrap(),
        index
    );

    // Resuming a complete pilot must not spend another call or duplicate records.
    let output = pilot_run(&root, &pilot, 9);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(pilot.join("index.jsonl")).unwrap(),
        index
    );
    let state = fs::read_to_string(pilot.join("pilot-state.jsonl")).unwrap();
    assert_eq!(state.matches("\"event\":\"complete\"").count(), 2);
    fs::remove_dir_all(root).unwrap();
}

fn sse_mocks(actions: &[Value]) -> Vec<Mock> {
    actions
        .iter()
        .map(|action| Mock {
            status: "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n",
            headers: "",
            body: action_stream(&action.to_string()),
        })
        .collect()
}

#[test]
fn main_plan_executes_and_resumes_with_run_artifacts() {
    let root = setup();
    let plan = root.join("main-plan.json");
    fs::write(
        &plan,
        json!({
            "status": "draft_not_authorized_for_execution",
            "algorithm": "splitmix64-fisher-yates-v1",
            "randomization_seed": 42,
            "repetitions": 1,
            "dataset": {"tasks": ["synthetic-task"], "seed": 2026},
            "runs": [{"task_id": "synthetic-task", "arm": "B", "repetition": 2}]
        })
        .to_string(),
    )
    .unwrap();
    let main = root.join("main");
    let (port, server) = serve_sequence(
        actions("B")
            .iter()
            .map(|action| action_stream(&action.to_string()))
            .collect(),
    );
    let output = binary()
        .args([
            "run-plan",
            root.join("dataset").to_str().unwrap(),
            plan.to_str().unwrap(),
            main.to_str().unwrap(),
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
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
    let normalized: Value =
        serde_json::from_slice(&fs::read(main.join("plan.json")).unwrap()).unwrap();
    assert_eq!(normalized["kind"], "experiment_plan");
    assert_eq!(normalized["runs"][0]["index"], 0);
    assert!(normalized["dataset_manifest_sha256"].is_string());
    let entries: Vec<Value> = fs::read_to_string(main.join("index.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["status"], "completed");
    assert_eq!(entries[0]["repetition"], 2);
    assert_eq!(entries[0]["summary"]["success"], true);
    let summary: Value =
        serde_json::from_slice(&fs::read(main.join("run-summary.json")).unwrap()).unwrap();
    assert_eq!(summary["runs"], 1);
    assert_eq!(summary["remaining"], 0);
    assert_eq!(summary["success"], 1);
    assert!(main.join("run-state.jsonl").exists());

    let output = binary()
        .args([
            "run-plan",
            root.join("dataset").to_str().unwrap(),
            plan.to_str().unwrap(),
            main.to_str().unwrap(),
            root.join("config.json").to_str().unwrap(),
            root.join("connector.json").to_str().unwrap(),
        ])
        .env("LUNA_ACCESS_TOKEN", "test-token")
        .env("LUNA_ACCOUNT_ID", "test-account")
        .env("LUNA_BASE_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    assert!(!output.status.success(), "plan root must not be reused");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quota_stops_pilot_without_scoring_and_resumes_on_a_new_attempt() {
    let root = setup();
    let pilot = root.join("pilot");
    assert!(plan(&root, &pilot).status.success());
    let parsed: Value =
        serde_json::from_slice(&fs::read(pilot.join("plan.json")).unwrap()).unwrap();
    let runs = parsed["runs"].as_array().unwrap().clone();

    let mut mocks = sse_mocks(&actions(runs[0]["arm"].as_str().unwrap()));
    mocks.push(Mock {
        status: "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\n",
        headers: "",
        body: json!({"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","resets_at":1794868701}}).to_string(),
    });
    let (port, server) = serve_mocks(mocks);
    let output = pilot_run(&root, &pilot, port);
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("\"reason\":\"quota\""),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let entries: Vec<Value> = fs::read_to_string(pilot.join("index.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["status"], "completed");
    let summary: Value =
        serde_json::from_slice(&fs::read(pilot.join("pilot-summary.json")).unwrap()).unwrap();
    assert_eq!(summary["runs"], 1);
    assert_eq!(summary["remaining"], 3);
    let state = fs::read_to_string(pilot.join("pilot-state.jsonl")).unwrap();
    assert!(state.contains("\"reason\":\"quota\""));
    assert!(state.contains("\"resets_at\":1794868701"));
    let blocked = fs::read_dir(pilot.join("runs"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("quota-blocked.json").exists())
        .collect::<Vec<_>>();
    assert_eq!(blocked.len(), 1);

    let mut resumed = Vec::new();
    for run in &runs[1..] {
        resumed.extend(sse_mocks(&actions(run["arm"].as_str().unwrap())));
    }
    let (port, server) = serve_mocks(resumed);
    let output = pilot_run(&root, &pilot, port);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
    let entries: Vec<Value> = fs::read_to_string(pilot.join("index.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries.len(), 4);
    let attempts = fs::read_dir(pilot.join("runs"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{:04}-", runs[1]["index"].as_u64().unwrap()))
        })
        .count();
    assert_eq!(attempts, 2, "quota evidence is retained and retried");
    let summary: Value =
        serde_json::from_slice(&fs::read(pilot.join("pilot-summary.json")).unwrap()).unwrap();
    assert_eq!(summary["runs"], 4);
    assert_eq!(summary["remaining"], 0);
    assert_eq!(summary["success"], 4);

    let mut config: Value =
        serde_json::from_slice(&fs::read(root.join("config.json")).unwrap()).unwrap();
    config["model"] = json!("gpt-6-luna-changed");
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    let output = pilot_run(&root, &pilot, 9);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("changed during the pilot"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
