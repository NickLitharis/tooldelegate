use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tooldelegate"))
        .args(args)
        .output()
        .unwrap()
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn sandbox_cli_feedback_and_hidden_scoring_are_separate() {
    let root = std::env::temp_dir().join(format!("sandbox-cli-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let dataset = root.join("dataset");
    let generated = run(&["generate-suite", text(&dataset), "2026"]);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let task = dataset.join("sorting-v0-s0");
    let code = "import csv\nwith open('input.csv',newline='') as f: rows=list(csv.reader(f))\nheader=rows.pop(0)\nrows.sort(key=lambda r:(r[1]=='',int(r[1] or 0)))\nwith open('output.csv','w',newline='') as f:\n w=csv.writer(f);w.writerow(header);w.writerows(rows)\nprint('PRIVATE'*2000)";
    let mut previous = None;
    for mode in ["B", "C", "D"] {
        let work = root.join(format!("work-{mode}"));
        let logs = root.join(format!("logs-{mode}"));
        let output = run(&[
            "execute-sandbox",
            text(&task),
            text(&work),
            text(&logs),
            mode,
            "python3",
            "-I",
            "-c",
            code,
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.len() <= 4096);
        let execution: Value =
            serde_json::from_slice(&fs::read(logs.join("execution.json")).unwrap()).unwrap();
        assert_eq!(execution["status"], 0);
        assert_eq!(execution["stdout_bytes"], 14001);
        assert_eq!(execution["stdout_truncated"], true);
        if let Some(ref before) = previous {
            assert_eq!(*before, execution["before"]);
        }
        previous = Some(execution["before"].clone());
        let scored = run(&["score-suite-task", text(&task), text(&work)]);
        assert!(scored.status.success());
        if mode == "D" {
            let certificate: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(certificate, serde_json::json!({"ok":true,"code":"PASS"}));
            assert!(output.stdout.len() <= 96);
        }
        if mode != "B" {
            let verification: Value =
                serde_json::from_slice(&fs::read(logs.join("verification.json")).unwrap()).unwrap();
            assert_eq!(verification["code"], "PASS");
            assert!(verification["cpu_seconds"].as_f64().unwrap() >= 0.0);
            assert!(verification["process_peak_rss_bytes"].as_u64().unwrap() > 0);
        } else {
            assert!(!logs.join("verification.json").exists());
        }
    }
    let work = root.join("bad-work");
    let logs = root.join("bad-logs");
    let wrong = run(&[
        "execute-sandbox",
        text(&task),
        text(&work),
        text(&logs),
        "D",
        "python3",
        "-I",
        "-c",
        "import shutil; shutil.copyfile('input.csv','output.csv')",
    ]);
    assert!(wrong.status.success());
    let certificate: Value = serde_json::from_slice(&wrong.stdout).unwrap();
    assert_eq!(certificate["ok"], false);
    assert_eq!(certificate["code"], "ORDER_ERROR");
    assert!(
        !run(&["score-suite-task", text(&task), text(&work)])
            .status
            .success()
    );
    fs::remove_dir_all(root).unwrap();
}
