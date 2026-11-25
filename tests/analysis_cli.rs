mod support;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "analysis-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    path
}
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tooldelegate"))
        .args(args)
        .output()
        .unwrap()
}
fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn jsonl(values: &[Value]) -> String {
    values
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn report_regenerates_identically_with_seven_charts_and_no_script_injection() {
    let root = root();
    let records = root.join("records.jsonl");
    let config = root.join("config.json");
    let mut values = support::records(3);
    for row in &mut values {
        row["task"]["family"] = json!("<script>alert(1)</script>");
        if row["arm"] == "D" {
            row["success"] = json!(false);
            row["oracle_pass"] = json!(false);
            row["metrics"]["last_verifier_ok"] = json!(false);
        }
    }
    values[0]["failure_events"] = json!([{"category":"syntax_or_parsing_failure","detail":"</pre><script>alert(1)</script>\n```"}]);
    fs::write(&records, jsonl(&values)).unwrap();
    fs::write(&config, support::config(3).to_string()).unwrap();
    let first = root.join("first");
    let output = run(&["analyze", path(&records), path(&first), path(&config)]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let second = root.join("second");
    let output = run(&[
        "analyze",
        path(&first.join("records.jsonl")),
        path(&second),
        path(&first.join("config.json")),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut svg_count = 0;
    for entry in fs::read_dir(&first).unwrap() {
        let entry = entry.unwrap();
        let bytes = fs::read(entry.path()).unwrap();
        assert_eq!(bytes, fs::read(second.join(entry.file_name())).unwrap());
        if entry.path().extension().is_some_and(|ext| ext == "svg") {
            svg_count += 1;
            let svg = String::from_utf8(bytes).unwrap();
            assert!(svg.starts_with("<svg "));
            assert!(!svg.contains("NaN"));
            assert!(!svg.contains("<script>"));
            assert!(!svg.contains("Infinity"));
        }
    }
    assert_eq!(svg_count, 7);
    let html = fs::read_to_string(first.join("report.html")).unwrap();
    assert!(html.contains("SYNTHETIC FIXTURES"));
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    let md = fs::read_to_string(first.join("report.md")).unwrap();
    assert_eq!(
        md.lines().filter(|line| line.starts_with("## ")).count(),
        14
    );
    assert!(md.contains("this report does not infer them from the presence of data"));
    assert!(!md.contains("protocol and margin remain unfrozen"));
    assert!(!md.contains("no claim that the main experiment has been run"));
    let summary: Value =
        serde_json::from_slice(&fs::read(first.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["confirmatory"], false);
    assert_eq!(summary["arms"]["C"]["token_efficiency"], 360.0);
    assert!(summary["arms"]["D"]["token_efficiency"].is_null());
    assert!(md.contains("Arm D: zero successful runs"));
    assert!(
        !run(&["analyze", path(&records), path(&first), path(&config)])
            .status
            .success()
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_missingness_cli_reports_partial_usage_without_imputing() {
    let root = root();
    let mut records = support::records(1);
    records[0]["metrics"]["usage_complete"] = json!(false);
    records[0]["metrics"]["total_tokens"] = json!(30);
    records[0]["failure_events"] = json!([{"category":"provider_content_rejection"}]);
    records[3]["metrics"]["usage_complete"] = json!(false);
    records[3]["metrics"]["total_tokens"] = json!(0);
    records[3]["failure_events"] = json!([{"category":"adapter_transport_failure"}]);
    let input = root.join("records.jsonl");
    fs::write(&input, jsonl(&records)).unwrap();
    let output = root.join("missingness.md");

    let result = run(&["usage-missingness", path(&input), path(&output)]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = fs::read_to_string(&output).unwrap();
    assert!(report.contains("Records: **8**"));
    assert!(report.contains("Usage incomplete: **2"));
    assert!(report.contains("provider_content_rejection"));
    assert!(report.contains("adapter_transport_failure"));
    assert!(report.contains("Known partial token subtotal within incomplete records: **30**"));
    assert!(report.contains("A zero subtotal does not prove zero token usage"));
    assert!(report.contains("| D vs A | 2 | 1 | 1 |"));
    assert!(report.contains("| A | 1 | 0 | 0 |"));
    assert!(
        !run(&["usage-missingness", path(&input), path(&output),])
            .status
            .success()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn collection_recovers_interrupted_runs_instead_of_dropping_them() {
    let root = root();
    let values = support::records(3);
    let finished = &values[0];
    let mut roots = Vec::new();
    for name in ["complete", "interrupted"] {
        let directory = root.join(name);
        fs::create_dir(&directory).unwrap();
        roots.push(directory.clone());
        let mut start = json!({"event":"start","synthetic":true,"arm":finished["arm"],"task":finished["task"],"config":finished["config"],"environment":{"fixture":true}});
        if name == "interrupted" {
            start["arm"] = json!("D");
        }
        let mut trace = jsonl(&[
            start,
            json!({"event":"model_request","call":0}),
            json!({"event":"model_response","reply":{"usage":{"input_tokens":80,"output_tokens":20,"cached_input_tokens":2,"schema_tokens":3,"feedback_input_tokens":0,"verification_input_tokens":0,"source":"synthetic_fixture"}}}),
        ]);
        if name == "complete" {
            trace += &jsonl(std::slice::from_ref(finished));
            fs::write(
                directory.join("record.jsonl"),
                jsonl(std::slice::from_ref(finished)),
            )
            .unwrap();
        } else {
            trace += &jsonl(&[
                json!({"event":"malformed_action","error":"invalid action JSON"}),
                json!({"event":"model_request","call":1}),
            ]);
            trace += "{\"event\":\"model_response\"";
        }
        fs::write(directory.join("trace.jsonl"), trace).unwrap();
    }
    let collected = root.join("collected.jsonl");
    let output = run(&[
        "collect-records",
        path(&collected),
        path(&roots[0]),
        path(&roots[1]),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<Value> = fs::read_to_string(&collected)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1]["termination"], "interrupted");
    assert_eq!(records[1]["truncated_trace"], true);
    assert_eq!(records[1]["metrics"]["total_tokens"], 100);
    assert_eq!(records[1]["metrics"]["usage_complete"], false);
    assert!(records[1]["metrics"]["end_to_end_seconds"].is_null());
    let config = root.join("config.json");
    fs::write(&config, support::config(3).to_string()).unwrap();
    let report = root.join("report");
    let output = run(&["analyze", path(&collected), path(&report), path(&config)]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&fs::read(report.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["records"], 2);
    assert_eq!(summary["arms"]["D"]["incomplete_runs"], 1);
    assert!(summary["arms"]["D"]["total_tokens"].is_null());
    assert_eq!(
        summary["failure_taxonomy"]["syntax_or_parsing_failure"]["known_events"],
        1
    );
    let complete = jsonl(std::slice::from_ref(finished));
    fs::write(roots[0].join("record.jsonl"), &complete.as_bytes()[..30]).unwrap();
    let repaired = root.join("recovered-write.jsonl");
    let output = run(&["collect-records", path(&repaired), path(&roots[0])]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value = serde_json::from_str(fs::read_to_string(repaired).unwrap().trim()).unwrap();
    assert_eq!(record["recovered_summary_file"], true);
    assert_eq!(record["success"], true);
    let mut bad = finished.clone();
    bad["success"] = json!(false);
    fs::write(roots[0].join("record.jsonl"), jsonl(&[bad])).unwrap();
    assert!(
        !run(&[
            "collect-records",
            path(&root.join("bad.jsonl")),
            path(&roots[0])
        ])
        .status
        .success()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn synthetic_opt_in_and_duplicate_rejection_do_not_create_a_report() {
    let root = root();
    let records = root.join("records.jsonl");
    let config = root.join("config.json");
    let destination = root.join("report");
    let mut values = support::records(3);
    fs::write(&records, jsonl(&values)).unwrap();
    let mut settings = support::config(3);
    settings["allow_synthetic"] = json!(false);
    fs::write(&config, settings.to_string()).unwrap();
    assert!(
        !run(&["analyze", path(&records), path(&destination), path(&config)])
            .status
            .success()
    );
    assert!(!destination.exists());
    settings["allow_synthetic"] = json!(true);
    fs::write(&config, settings.to_string()).unwrap();
    values.push(values[0].clone());
    fs::write(&records, jsonl(&values)).unwrap();
    assert!(
        !run(&["analyze", path(&records), path(&destination), path(&config)])
            .status
            .success()
    );
    assert!(!destination.exists());
    fs::remove_dir_all(root).unwrap();
}
