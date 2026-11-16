use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const INPUT: &str = "id,value,secondary,payload\n0,2,0,x\n1,1,0,y\n2,2,0,z\n3,,0,q\n";
const GOOD: &str = "id,value,secondary,payload\n1,1,0,y\n0,2,0,x\n2,2,0,z\n3,,0,q\n";
const SORT: &str = "import csv\nwith open('input.csv',newline='') as f: rows=list(csv.reader(f))\nh=rows.pop(0);rows.sort(key=lambda r:(r[1]=='',int(r[1] or 0)))\nwith open('output.csv','w',newline='') as f:\n w=csv.writer(f);w.writerow(h);w.writerows(rows)";

fn setup() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "orchestrator-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("task")).unwrap();
    fs::create_dir(root.join("task/input")).unwrap();
    fs::write(root.join("task/input/input.csv"), INPUT).unwrap();
    fs::write(root.join("task/manifest.json"),json!({"id":"synthetic-test","family":"sorting","variant":0,"expected_outcome":"success","requirement":"Sort input.csv by signed numeric value ascending, nulls last, stable ties; write output.csv and preserve original input.","input_hashes":{"input.csv":format!("{:x}",Sha256::digest(INPUT.as_bytes()))}}).to_string()).unwrap();
    let mut config: Value =
        serde_json::from_str(include_str!("../configs/run.example.json")).unwrap();
    config["model"] = json!("simulation-only-v1");
    config["pricing"] = json!({"currency":"TEST","input_per_million":1.0,"cached_input_per_million":0.1,"output_per_million":2.0,"source":"synthetic test fixture, not market pricing"});
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    root
}

fn replies(root: &Path, actions: &[Value]) {
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("config.json")).unwrap()).unwrap();
    let lines=actions.iter().enumerate().map(|(i,action)|json!({"model":"simulation-only-v1","decoding":config["decoding"],"action":if let Some(raw)=action.as_str(){raw.to_owned()}else{action.to_string()},"finish_reason":"stop","usage":{"input_tokens":100,"output_tokens":10,"cached_input_tokens":10,"schema_tokens":12,"feedback_input_tokens":if i==0{0}else{5},"verification_input_tokens":0,"source":"synthetic_fixture"},"provider_response":{"fixture":true},"provider_request":{"fixture":true}}).to_string()).collect::<Vec<_>>().join("\n");
    fs::write(root.join("replies.jsonl"), lines).unwrap();
}

fn run(root: &Path, arm: &str, name: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_verified-cli-benchmark"))
        .args([
            "replay-run",
            root.join("task").to_str().unwrap(),
            root.join(name).to_str().unwrap(),
            arm,
            root.join("config.json").to_str().unwrap(),
            root.join("replies.jsonl").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn all_arms_count_usage_and_keep_oracle_out_of_context() {
    let root = setup();
    for arm in ["A", "B", "C", "D"] {
        let actions = if arm == "A" {
            vec![
                json!({"type":"read","path":"input.csv","offset":0,"bytes":32}),
                json!({"type":"put","path":"output.csv","content":&GOOD[..30],"append":false}),
                json!({"type":"put","path":"output.csv","content":&GOOD[30..],"append":true}),
                json!({"type":"submit","commands":null}),
                json!({"type":"finish"}),
            ]
        } else {
            vec![
                json!({"type":"read","path":"input.csv","offset":0,"bytes":32}),
                json!({"type":"submit","commands":[["python3","-I","-c",SORT]]}),
                json!({"type":"finish"}),
            ]
        };
        replies(&root, &actions);
        let summary = run(&root, arm, arm);
        assert_eq!(summary["synthetic"], true);
        assert_eq!(summary["success"], true);
        assert_eq!(summary["oracle_pass"], true);
        assert_eq!(summary["metrics"]["calls"], actions.len());
        assert_eq!(summary["metrics"]["total_tokens"], actions.len() * 110);
        assert_eq!(
            summary["metrics"]["cached_input_tokens"],
            actions.len() * 10
        );
        assert_eq!(summary["metrics"]["context_peak"], 110);
        assert!(
            (summary["metrics"]["monetary_cost"].as_f64().unwrap()
                - actions.len() as f64 * 0.000111)
                .abs()
                < 1e-12
        );
        let log = fs::read_to_string(root.join(arm).join("trace.jsonl")).unwrap();
        let events: Vec<Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for event in &events {
            if event["event"] == "model_request" {
                let text = event["request"].to_string();
                assert!(!text.contains("oracle_pass"));
                assert!(!text.contains("expected_outcome"));
                assert!(!text.contains("provider_response"));
                if arm == "A" {
                    assert!(!text.contains("id,value,secondary,payload"));
                }
            }
        }
        assert_eq!(
            events
                .iter()
                .filter(|v| v["event"] == "verification")
                .count(),
            if arm == "C" || arm == "D" { 1 } else { 0 }
        );
        let actual = fs::read(root.join(arm).join("work/output.csv")).unwrap();
        let rows = |bytes: &[u8]| {
            csv::Reader::from_reader(bytes)
                .records()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(rows(&actual), rows(GOOD.as_bytes()));
        let record = fs::read_to_string(root.join(arm).join("record.jsonl")).unwrap();
        assert_eq!(record.lines().count(), 1);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_attempt_can_retry_without_oracle_feedback() {
    let root = setup();
    replies(
        &root,
        &[
            json!({"type":"submit","commands":[["python3","-I","-c","import shutil;shutil.copyfile('input.csv','output.csv')"]]}),
            json!({"type":"submit","commands":[["python3","-I","-c",SORT]]}),
            json!({"type":"finish"}),
        ],
    );
    let summary = run(&root, "D", "retry");
    assert_eq!(summary["success"], true);
    assert_eq!(summary["metrics"]["attempts"], 2);
    assert_eq!(summary["metrics"]["retries"], 1);
    let trace = fs::read_to_string(root.join("retry/trace.jsonl")).unwrap();
    assert!(trace.contains("ORDER_ERROR"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_missing_and_resource_limited_runs_remain_records() {
    let root = setup();
    replies(&root, &[json!("{bad-json")]);
    let summary = run(&root, "A", "malformed");
    assert_eq!(summary["success"], false);
    assert_eq!(summary["termination"], "adapter_error");
    assert_eq!(summary["metrics"]["total_tokens"], 110);
    assert_eq!(summary["metrics"]["usage_complete"], false);
    assert!(summary["metrics"]["monetary_cost"].is_null());
    let trace = fs::read_to_string(root.join("malformed/trace.jsonl")).unwrap();
    assert!(trace.contains("MALFORMED_ACTION"));
    let mut config: Value =
        serde_json::from_slice(&fs::read(root.join("config.json")).unwrap()).unwrap();
    config["max_output_tokens"] = json!(5);
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    replies(&root, &[json!({"type":"finish"})]);
    let summary = run(&root, "A", "limited");
    assert_eq!(summary["termination"], "resource_limit");
    assert_eq!(summary["success"], false);
    assert_eq!(summary["metrics"]["total_tokens"], 110);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_sink_rejects_escape_and_tool_calls_and_requires_completion() {
    let root = setup();
    replies(
        &root,
        &[
            json!({"type":"put","path":"../escaped.txt","content":"bad","append":false}),
            json!({"type":"submit","commands":[["python3","-c","raise AssertionError('must never run')"]]}),
            json!({"type":"put","path":"output.csv","content":GOOD,"append":false}),
            json!({"type":"submit","commands":null}),
            json!({"type":"finish"}),
        ],
    );
    let summary = run(&root, "A", "safe");
    assert_eq!(summary["success"], true);
    assert_eq!(summary["metrics"]["tool_errors"], 2);
    assert!(!root.join("safe/escaped.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("task/input/input.csv")).unwrap(),
        INPUT
    );
    let mut config: Value =
        serde_json::from_slice(&fs::read(root.join("config.json")).unwrap()).unwrap();
    config["max_calls"] = json!(2);
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    replies(
        &root,
        &[
            json!({"type":"put","path":"output.csv","content":GOOD,"append":false}),
            json!({"type":"submit","commands":null}),
        ],
    );
    let summary = run(&root, "A", "incomplete");
    assert_eq!(summary["oracle_pass"], true);
    assert_eq!(summary["success"], false);
    assert_eq!(summary["complete"], false);
    assert_eq!(summary["termination"], "call_limit");
    fs::remove_dir_all(root).unwrap();
}
