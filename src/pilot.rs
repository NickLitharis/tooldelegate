//! Pilot planning and execution.
//!
//! Planning is separate from execution so a quota block cannot cost the randomization
//! seed or the matrix. Execution resumes from `index.jsonl`, never overwrites evidence,
//! and stops on subscription-quota exhaustion instead of recording censored runs as
//! model results.

use crate::model_adapter::Connector;
use crate::orchestrator::{self, Arm, Config};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

fn read_json(path: &Path) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn dataset_ids(dataset: &Path) -> Result<(Value, Vec<String>), Box<dyn Error>> {
    let manifest = read_json(&dataset.join("dataset.json"))?;
    let tasks = manifest["tasks"]
        .as_array()
        .ok_or("dataset manifest has no tasks")?;
    let ids = tasks
        .iter()
        .map(|task| {
            task.as_str()
                .map(str::to_owned)
                .ok_or("invalid dataset task id")
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((manifest, ids))
}

pub fn plan(
    dataset: &Path,
    selection: &Path,
    root: &Path,
    seed: u64,
    repetitions: u64,
) -> Result<(), Box<dyn Error>> {
    if !(1..=100).contains(&repetitions) {
        return Err("pilot repetitions must be between 1 and 100".into());
    }
    let (manifest, available) = dataset_ids(dataset)?;
    let selection = read_json(selection)?;
    let requested = selection["tasks"]
        .as_array()
        .ok_or("selection must contain a tasks array")?
        .iter()
        .map(|task| {
            task.as_str()
                .map(str::to_owned)
                .ok_or("selection task must be a string")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if requested.is_empty() {
        return Err("pilot selection is empty".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for task in &requested {
        if !safe_id(task) {
            return Err("unsafe pilot task id".into());
        }
        if !available.iter().any(|id| id == task) {
            return Err(format!("task {task} is not in the dataset").into());
        }
        if !seen.insert(task) {
            return Err("duplicate pilot task".into());
        }
    }
    let mut order = Vec::new();
    for task in &requested {
        for repetition in 0..repetitions {
            for arm in ["A", "B", "C", "D"] {
                order.push(json!({"task_id":task,"arm":arm,"repetition":repetition}));
            }
        }
    }
    crate::schedule::shuffle(&mut order, seed);
    for (index, run) in order.iter_mut().enumerate() {
        run["index"] = json!(index);
    }
    fs::create_dir(root)?;
    let content = json!({
        "kind": "pilot_plan",
        "algorithm": "splitmix64-fisher-yates-v1",
        "randomization_seed": seed,
        "repetitions": repetitions,
        "tasks": requested,
        "dataset_seed": manifest["seed"],
        "dataset_manifest_sha256": format!("{:x}", Sha256::digest(fs::read(dataset.join("dataset.json"))?)),
        "runs": order
    });
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(root.join("plan.json"))?;
    file.write_all(&serde_json::to_vec_pretty(&content)?)?;
    println!(
        "{}",
        json!({"status":"planned","root":root,"runs":content["runs"].as_array().map(Vec::len),"plan":root.join("plan.json")})
    );
    Ok(())
}

fn dataset_key(dataset: &Path) -> Result<String, Box<dyn Error>> {
    Ok(format!(
        "{:x}",
        Sha256::digest(fs::read(dataset.join("dataset.json"))?)
    ))
}

fn terminal(index: &Path) -> Result<std::collections::BTreeMap<u64, Value>, Box<dyn Error>> {
    let mut done = std::collections::BTreeMap::new();
    if !index.exists() {
        return Ok(done);
    }
    for line in fs::read_to_string(index)?.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(line)?;
        let value = entry["index"].as_u64().ok_or("index entry has no index")?;
        if done.insert(value, entry).is_some() {
            return Err("duplicate index entry".into());
        }
    }
    Ok(done)
}

fn append(path: &Path, value: &Value) -> Result<(), Box<dyn Error>> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn state(root: &Path, value: Value) -> Result<(), Box<dyn Error>> {
    append(&root.join("pilot-state.jsonl"), &value)
}

fn run_dir(root: &Path, entry: &Value) -> PathBuf {
    let index = entry["index"].as_u64().unwrap_or_default();
    let name = format!(
        "{index:04}-{}-{}-r{}",
        entry["task_id"].as_str().unwrap_or("unknown"),
        entry["arm"].as_str().unwrap_or("?"),
        entry["repetition"].as_u64().unwrap_or_default()
    );
    let base = root.join("runs").join(name);
    if !base.exists() {
        return base;
    }
    for attempt in 2..1000u32 {
        let candidate = root.join("runs").join(format!(
            "{}-a{attempt}",
            base.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
    root.join("runs").join("exhausted")
}

fn existing_dirs(root: &Path, entry: &Value) -> Vec<PathBuf> {
    let prefix = format!("{:04}-", entry["index"].as_u64().unwrap_or_default());
    let mut found = fs::read_dir(root.join("runs"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().starts_with(&prefix))
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// True when this run was stopped by quota rather than model behavior. Such runs are
/// retried on resume; they are never scored or discarded.
fn quota_marker(run_dir: &Path) -> Option<Value> {
    let marker = run_dir.join("quota-blocked.json");
    fs::read_to_string(marker)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn quota_reset(run_dir: &Path) -> Option<i64> {
    let trace = fs::read_to_string(run_dir.join("trace.jsonl")).ok()?;
    for line in trace.lines() {
        if !line.contains("usage_limit_reached") {
            continue;
        }
        let position = line.find("resets_at=")? + "resets_at=".len();
        let digits: String = line[position..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        return digits.parse().ok();
    }
    None
}

fn pending_entries(
    root: &Path,
    plan: &Value,
    done: &std::collections::BTreeMap<u64, Value>,
) -> Result<Vec<Value>, Box<dyn Error>> {
    let runs = plan["runs"].as_array().ok_or("plan has no runs")?;
    let mut pending = Vec::new();
    for (position, entry) in runs.iter().enumerate() {
        let index = entry["index"].as_u64().ok_or("plan entry has no index")?;
        if index != position as u64 {
            return Err("plan indices are not contiguous".into());
        }
        if done.contains_key(&index) {
            continue;
        }
        // A crash can leave a finished run without an index line. Recover its summary
        // instead of spending another live call; quota-blocked attempts stay pending.
        let mut recovered = false;
        for directory in existing_dirs(root, entry) {
            if quota_marker(&directory).is_some() {
                continue;
            }
            if let Ok(summary) = read_json(&directory.join("summary.json")) {
                append(
                    &root.join("index.jsonl"),
                    &json!({
                        "index": index,
                        "task_id": entry["task_id"],
                        "arm": entry["arm"],
                        "repetition": entry["repetition"],
                        "run_dir": directory,
                        "status": "recovered",
                        "summary": summary
                    }),
                )?;
                recovered = true;
                break;
            }
        }
        if recovered {
            continue;
        }
        pending.push(entry.clone());
    }
    Ok(pending)
}

fn summarize(root: &Path, plan: &Value) -> Result<Value, Box<dyn Error>> {
    let index = root.join("index.jsonl");
    let mut counts = std::collections::BTreeMap::<String, Value>::new();
    let (mut runs, mut success, mut oracle, mut incomplete, mut usage_incomplete) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut calls, mut tokens) = (0u64, 0u64);
    if index.exists() {
        for line in fs::read_to_string(&index)?
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            let entry: Value = serde_json::from_str(line)?;
            let arm = entry["arm"].as_str().unwrap_or("?").to_owned();
            let counter = counts
                .entry(arm)
                .or_insert_with(|| json!({"runs":0,"success":0,"oracle_pass":0,"incomplete":0,"calls":0,"total_tokens":0}));
            runs += 1;
            counter["runs"] = json!(counter["runs"].as_u64().unwrap_or(0) + 1);
            if let Some(summary) = entry.get("summary")
                && summary.is_object()
            {
                if summary["success"] == true {
                    success += 1;
                    counter["success"] = json!(counter["success"].as_u64().unwrap_or(0) + 1);
                }
                if summary["oracle_pass"] == true {
                    oracle += 1;
                    counter["oracle_pass"] =
                        json!(counter["oracle_pass"].as_u64().unwrap_or(0) + 1);
                }
                if summary["complete"] == false {
                    incomplete += 1;
                    counter["incomplete"] = json!(counter["incomplete"].as_u64().unwrap_or(0) + 1);
                }
                if summary["metrics"]["usage_complete"] == false {
                    usage_incomplete += 1;
                }
                let run_calls = summary["metrics"]["calls"].as_u64().unwrap_or(0);
                let run_tokens = summary["metrics"]["total_tokens"].as_u64().unwrap_or(0);
                calls += run_calls;
                tokens += run_tokens;
                counter["calls"] = json!(counter["calls"].as_u64().unwrap_or(0) + run_calls);
                counter["total_tokens"] =
                    json!(counter["total_tokens"].as_u64().unwrap_or(0) + run_tokens);
            }
        }
    }
    let planned = plan["runs"].as_array().map(Vec::len).unwrap_or(0) as u64;
    Ok(json!({
        "runs": runs,
        "planned": planned,
        "remaining": planned.saturating_sub(runs),
        "success": success,
        "oracle_pass": oracle,
        "incomplete": incomplete,
        "usage_incomplete": usage_incomplete,
        "calls": calls,
        "total_tokens": tokens,
        "by_arm": counts
    }))
}

pub fn execute(
    dataset: &Path,
    root: &Path,
    config_path: &Path,
    connector_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let plan = read_json(&root.join("plan.json"))?;
    if plan["kind"] != "pilot_plan" {
        return Err("root does not contain a pilot plan".into());
    }
    if plan["dataset_manifest_sha256"].as_str() != Some(dataset_key(dataset)?.as_str()) {
        return Err("dataset manifest changed since the plan was created".into());
    }
    let config_value = read_json(config_path)?;
    let config: Config = serde_json::from_value(config_value.clone())?;
    orchestrator::validate(&config)?;
    if config.model.starts_with("REPLACE_") {
        return Err("select a pinned model before pilot execution".into());
    }
    let connector_value = read_json(connector_path)?;
    let connector = Connector::from_config(
        &connector_value,
        std::time::Duration::from_secs(config.task_wall_seconds.min(90)),
    )?;
    let protocol = json!({"config":config_value,"connector":connector_value});
    let protocol_path = root.join("protocol.json");
    if protocol_path.exists() {
        if read_json(&protocol_path)? != protocol {
            return Err("run configuration or connector changed during the pilot".into());
        }
    } else {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&protocol_path)?;
        file.write_all(&serde_json::to_vec_pretty(&protocol)?)?;
    }
    fs::create_dir_all(root.join("runs"))?;
    state(root, json!({"event":"execute_start","unix_seconds":now()}))?;

    let done = terminal(&root.join("index.jsonl"))?;
    let pending = pending_entries(root, &plan, &done)?;
    if pending.is_empty() {
        let summary = summarize(root, &plan)?;
        write_summary(root, &summary)?;
        state(root, json!({"event":"complete","summary":summary}))?;
        println!(
            "{}",
            json!({"status":"complete","summary":root.join("pilot-summary.json")})
        );
        return Ok(());
    }

    let mut consecutive_adapter_errors = 0u64;
    for entry in pending {
        let index = entry["index"].as_u64().ok_or("entry index missing")?;
        let task_id = entry["task_id"].as_str().ok_or("entry task missing")?;
        let arm: Arm = serde_json::from_value(Value::String(
            entry["arm"].as_str().ok_or("entry arm missing")?.to_owned(),
        ))?;
        let repetition = entry["repetition"]
            .as_u64()
            .ok_or("entry repetition missing")?;
        let mut run_config = config.clone();
        run_config.repetition = repetition;
        let directory = run_dir(root, &entry);
        let started = Instant::now();
        let summary = match orchestrator::run(
            &dataset.join(task_id),
            &directory,
            arm,
            &run_config,
            &mut connector.clone(),
        ) {
            Ok(summary) => summary,
            Err(error) => {
                append(
                    &root.join("index.jsonl"),
                    &json!({
                        "index": index,
                        "task_id": task_id,
                        "arm": entry["arm"],
                        "repetition": repetition,
                        "run_dir": directory,
                        "status": "error",
                        "error": error.to_string()
                    }),
                )?;
                let summary = summarize(root, &plan)?;
                write_summary(root, &summary)?;
                state(
                    root,
                    json!({"event":"stopped","reason":"run_setup_error","index":index,"error":error.to_string()}),
                )?;
                return Err(format!("pilot stopped at run {index}: {error}").into());
            }
        };
        if let Some(reset) = quota_reset(&directory) {
            let marker =
                json!({"reason":"quota","resets_at":reset,"index":index,"run_dir":directory});
            fs::write(
                directory.join("quota-blocked.json"),
                serde_json::to_vec_pretty(&marker)?,
            )?;
            let summary = summarize(root, &plan)?;
            write_summary(root, &summary)?;
            state(
                root,
                json!({"event":"blocked","reason":"quota","index":index,"resets_at":reset}),
            )?;
            println!(
                "{}",
                json!({"status":"blocked","reason":"quota","index":index,"resets_at":reset,"summary":root.join("pilot-summary.json")})
            );
            return Err("pilot stopped by the subscription quota; resume after reset".into());
        }
        let termination = summary["termination"].as_str().unwrap_or("unknown");
        consecutive_adapter_errors = if termination == "adapter_error" {
            consecutive_adapter_errors + 1
        } else {
            0
        };
        append(
            &root.join("index.jsonl"),
            &json!({
                "index": index,
                "task_id": task_id,
                "arm": entry["arm"],
                "repetition": repetition,
                "run_dir": directory,
                "status": "completed",
                "elapsed_seconds": started.elapsed().as_secs_f64(),
                "summary": summary
            }),
        )?;
        if consecutive_adapter_errors >= 3 {
            let summary = summarize(root, &plan)?;
            write_summary(root, &summary)?;
            state(
                root,
                json!({"event":"blocked","reason":"adapter_failures","index":index}),
            )?;
            println!(
                "{}",
                json!({"status":"blocked","reason":"adapter_failures","index":index,"summary":root.join("pilot-summary.json")})
            );
            return Err("pilot stopped after three consecutive adapter failures".into());
        }
    }
    let summary = summarize(root, &plan)?;
    write_summary(root, &summary)?;
    state(root, json!({"event":"complete","summary":summary}))?;
    println!(
        "{}",
        json!({"status":"complete","summary":root.join("pilot-summary.json")})
    );
    Ok(())
}

fn write_summary(root: &Path, summary: &Value) -> Result<(), Box<dyn Error>> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(root.join("pilot-summary.json"))?;
    file.write_all(&serde_json::to_vec_pretty(summary)?)?;
    Ok(())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(seed: u64) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "pilot-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dataset = root.join("dataset");
        fs::create_dir_all(dataset.join("sorting-a")).unwrap();
        fs::create_dir_all(dataset.join("sorting-b")).unwrap();
        fs::write(
            dataset.join("dataset.json"),
            json!({"tasks":["sorting-a","sorting-b"],"seed":seed}).to_string(),
        )
        .unwrap();
        let selection = root.join("selection.json");
        fs::write(
            selection.clone(),
            json!({"tasks":["sorting-a","sorting-b"]}).to_string(),
        )
        .unwrap();
        (root, dataset, selection)
    }

    #[test]
    fn plan_is_reproducible_and_rejects_unknown_tasks() {
        let (root, dataset, selection) = fixture(2026);
        let a = root.join("a");
        let b = root.join("b");
        plan(&dataset, &selection, &a, 42, 2).unwrap();
        plan(&dataset, &selection, &b, 42, 2).unwrap();
        assert_eq!(
            fs::read(a.join("plan.json")).unwrap(),
            fs::read(b.join("plan.json")).unwrap()
        );
        let parsed: Value = read_json(&a.join("plan.json")).unwrap();
        let runs = parsed["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 16);
        let unique = runs
            .iter()
            .map(Value::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), 16);
        assert!(runs.windows(2).all(
            |pair| pair[0]["index"].as_u64().unwrap() + 1 == pair[1]["index"].as_u64().unwrap()
        ));
        let bad = root.join("bad.json");
        fs::write(&bad, json!({"tasks":["missing"]}).to_string()).unwrap();
        assert!(plan(&dataset, &bad, &root.join("c"), 42, 2).is_err());
        fs::write(&bad, json!({"tasks":["sorting-a","sorting-a"]}).to_string()).unwrap();
        assert!(plan(&dataset, &bad, &root.join("d"), 42, 2).is_err());
        assert!(plan(&dataset, &selection, &root.join("e"), 42, 0).is_err());
        let changed = root.join("c");
        assert!(plan(&dataset, &selection, &changed, 42, 1).is_ok());
        fs::write(
            dataset.join("dataset.json"),
            json!({"tasks":["sorting-a","sorting-b"],"seed":2027}).to_string(),
        )
        .unwrap();
        assert!(
            execute(
                &dataset,
                &changed,
                &root.join("cfg.json"),
                &root.join("con.json")
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
