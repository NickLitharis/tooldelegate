use sha2::{Digest, Sha256};
use std::{env, error::Error, fs, path::Path};

mod aggregation_task;
mod analysis;
mod boundary;
mod certificates;
mod collect;
mod dataset;
mod executor;
mod feedback;
mod filesystem_task;
mod invocation;
mod luna_connector;
mod metrics;
mod model_adapter;
mod oracle;
mod orchestrator;
mod pilot;
mod report;
mod schedule;
mod suite;
mod text_task;
mod verifier;

fn generate(destination: &Path, seed: u64, rows: usize) -> Result<(), Box<dyn Error>> {
    if rows == 0 || rows > 1_000_000 {
        return Err("rows must be between 1 and 1000000".into());
    }
    // Creating the directory exclusively prevents accidental overwrites.
    fs::create_dir(destination)?;
    let mut state = seed;
    let mut csv = String::from("id,value\n");
    for id in 0..rows {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let value = (state % (2 * rows as u64 + 1)) as i64 - rows as i64;
        csv.push_str(&format!("{id},{value}\n"));
    }
    let hash = format!("{:x}", Sha256::digest(csv.as_bytes()));
    let manifest = serde_json::json!({
        "id": "numeric-sort", "seed": seed, "rows": rows,
        "generator": "lcg64-v1", "input": "input.csv", "input_sha256": hash,
        "requirement": "Sort rows by value as a signed integer, ascending; preserve original order for equal values.",
        "invariants": ["preserve header", "preserve every row", "preserve schema", "stable ties"]
    });
    fs::write(destination.join("input.csv"), csv)?;
    fs::write(
        destination.join("manifest.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() == 2 && args[1] == "luna-connector" {
        return luna_connector::serve();
    }
    if args.len() == 7 && args[1] == "pilot-plan" {
        return pilot::plan(
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
            args[5].parse()?,
            args[6].parse()?,
        );
    }
    if args.len() == 6 && args[1] == "pilot-run" {
        return pilot::execute(
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
            Path::new(&args[5]),
        );
    }
    if args.len() == 5 && args[1] == "analyze" {
        return report::generate(
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
        );
    }
    if args.len() >= 4 && args[1] == "collect-records" {
        return collect::collect(
            &args[3..]
                .iter()
                .map(std::path::PathBuf::from)
                .collect::<Vec<_>>(),
            Path::new(&args[2]),
        );
    }
    if args.len() == 6 && args[1] == "plan-runs" {
        return schedule::generate(
            Path::new(&args[2]),
            Path::new(&args[3]),
            args[4].parse()?,
            args[5].parse()?,
        );
    }
    if args.len() == 4 && args[1] == "score-internal" {
        let value = match suite::score(Path::new(&args[2]), Path::new(&args[3])) {
            Ok(pass) => serde_json::json!({"pass":pass}),
            Err(error) => serde_json::json!({"pass":false,"artifact_error":error.to_string()}),
        };
        println!("{value}");
        return Ok(());
    }
    if args.len() == 7 && (args[1] == "replay-run" || args[1] == "connector-run") {
        let config: orchestrator::Config = serde_json::from_slice(&fs::read(&args[5])?)?;
        let arm: orchestrator::Arm =
            serde_json::from_value(serde_json::Value::String(args[4].clone()))?;
        let mut adapter: Box<dyn model_adapter::ModelAdapter> = if args[1] == "replay-run" {
            Box::new(model_adapter::Replay::load(Path::new(&args[6]))?)
        } else {
            if config.model.starts_with("REPLACE_") {
                return Err("select a pinned model before using a live connector".into());
            }
            let connector: serde_json::Value = serde_json::from_slice(&fs::read(&args[6])?)?;
            Box::new(model_adapter::Connector::from_config(
                &connector,
                std::time::Duration::from_secs(config.task_wall_seconds.min(90)),
            )?)
        };
        let summary = orchestrator::run(
            Path::new(&args[2]),
            Path::new(&args[3]),
            arm,
            &config,
            adapter.as_mut(),
        )?;
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }
    if args.len() == 4 && args[1] == "verify-internal" {
        println!(
            "{}",
            verifier::verify(Path::new(&args[2]), Path::new(&args[3])).record()
        );
        return Ok(());
    }
    if args.len() >= 7 && args[1] == "execute-sandbox" {
        let mode = match args[5].as_str() {
            "B" => feedback::Mode::RawTool,
            "C" => feedback::Mode::RawVerifier,
            "D" => feedback::Mode::Certificate,
            _ => return Err("mode must be B, C or D".into()),
        };
        let work = executor::fresh_work(Path::new(&args[2]), Path::new(&args[3]))?;
        let log = Path::new(&args[4]);
        fs::create_dir(log)?;
        if fs::canonicalize(log)?.starts_with(&work) {
            return Err("logs must be outside the sandbox".into());
        }
        let limits = executor::Limits::default();
        fs::write(
            log.join("invocation.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"args":&args[6..],"mode":args[5],"wall_seconds":limits.wall.as_secs_f64(),"cpu_seconds":limits.cpu_seconds,"memory_bytes":limits.memory_bytes,"file_bytes":limits.file_bytes,"tree_bytes":limits.tree_bytes,"tree_entries":limits.tree_entries,"processes":limits.processes,"open_files":limits.open_files,"feedback_bytes":limits.feedback_bytes}),
            )?,
        )?;
        let run = match executor::execute(&work, &args[6..], &limits) {
            Ok(run) => run,
            Err(error) => {
                fs::write(
                    log.join("rejection.json"),
                    serde_json::to_vec(&serde_json::json!({"error":error.to_string()}))?,
                )?;
                return Err(error);
            }
        };
        executor::save_trace(&run, &log.join("execution.json"))?;
        let verification = if matches!(mode, feedback::Mode::RawTool) {
            None
        } else {
            Some(verifier::verify_isolated(Path::new(&args[2]), &work))
        };
        if let Some(ref v) = verification {
            fs::write(
                log.join("verification.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"code":v.code.name(),"details":v.details,"elapsed_seconds":v.elapsed_seconds,"cpu_seconds":v.cpu_seconds,"process_peak_rss_bytes":v.process_peak_rss_bytes}),
                )?,
            )?;
        }
        use std::io::Write;
        let returned = feedback::render(mode, &run, verification.as_ref(), limits.feedback_bytes)?;
        fs::write(log.join("feedback.bin"), &returned)?;
        std::io::stdout().write_all(&returned)?;
        return Ok(());
    }
    if args.len() == 4 && args[1] == "reference-suite-task" {
        return suite::reference(Path::new(&args[2]), Path::new(&args[3]));
    }
    if args.len() == 4 && args[1] == "score-suite-task" {
        if !suite::score(Path::new(&args[2]), Path::new(&args[3]))? {
            return Err("oracle rejected output".into());
        }
        println!("PASS");
        return Ok(());
    }
    if args.len() == 4 && args[1] == "generate-suite" {
        return dataset::generate(Path::new(&args[2]), args[3].parse()?);
    }
    if args.len() == 5 && args[1] == "generate-aggregation" {
        return aggregation_task::generate(Path::new(&args[2]), args[3].parse()?, args[4].parse()?);
    }
    if args.len() == 4 && args[1] == "score-aggregation" {
        if !aggregation_task::score(Path::new(&args[2]), Path::new(&args[3]))? {
            return Err("oracle rejected output".into());
        }
        println!("PASS");
        return Ok(());
    }
    if args.len() == 4 && args[1] == "generate-filesystem" {
        return filesystem_task::generate(Path::new(&args[2]), args[3].parse()?);
    }
    if args.len() == 4 && args[1] == "score-filesystem" {
        if !filesystem_task::score(Path::new(&args[2]), Path::new(&args[3]))? {
            return Err("oracle rejected output".into());
        }
        println!("PASS");
        return Ok(());
    }
    if args.len() == 4 && args[1] == "generate-text" {
        return text_task::generate(Path::new(&args[2]), args[3].parse()?);
    }
    if args.len() == 4 && args[1] == "score-text" {
        if !text_task::score(Path::new(&args[2]), Path::new(&args[3]))? {
            return Err("oracle rejected output".into());
        }
        println!("PASS");
        return Ok(());
    }
    if args.len() == 4 && args[1] == "score" {
        if !oracle::numeric_sort(Path::new(&args[2]), Path::new(&args[3]))? {
            return Err("oracle rejected output".into());
        }
        println!("PASS");
        return Ok(());
    }
    if args.len() != 4 {
        return Err(
            "usage: verified-cli-benchmark DESTINATION SEED ROWS (destination must not exist)"
                .into(),
        );
    }
    generate(Path::new(&args[1]), args[2].parse()?, args[3].parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn draft_configs_record_required_decisions() {
        let experiment: serde_json::Value =
            serde_json::from_str(include_str!("../configs/experiment.json")).unwrap();
        let model: serde_json::Value =
            serde_json::from_str(include_str!("../configs/models.json")).unwrap();
        assert_eq!(experiment["status"], "draft");
        assert_eq!(experiment["arms"], serde_json::json!(["A", "B", "C", "D"]));
        assert_eq!(experiment["main"]["tasks"], 48);
        assert_eq!(experiment["main"]["repetitions_per_arm"], 3);
        assert_eq!(model["model_version"], "gpt-6-luna");
        assert_eq!(model["catalog_id"], "openai/gpt-6-luna");
        assert_eq!(model["status"], "access_and_accounting_verified");
        assert!(model["decoding"].get("temperature").is_none());
        let probe: serde_json::Value =
            serde_json::from_str(include_str!("../reports/luna-connector-probe.json")).unwrap();
        assert_eq!(probe["benchmark_run"], false);
        assert_eq!(probe["synthetic"], false);
        assert_eq!(probe["usage_source"], "api_reported");
        assert_eq!(probe["resolved_model"], "gpt-6-luna");
        assert_eq!(probe["pinned_model_matches"], true);
        assert!(probe["reply"]["usage"]["input_tokens"].as_u64().unwrap() > 0);
        assert!(probe["reply"]["usage"]["output_tokens"].as_u64().unwrap() > 0);
        assert_eq!(probe["rejected_request"]["decoding"]["temperature"], 0);
        let smoke: serde_json::Value =
            serde_json::from_str(include_str!("../reports/smoke-run.json")).unwrap();
        assert_eq!(smoke["pilot_run"], false);
        assert_eq!(smoke["benchmark_run"], false);
        assert_eq!(smoke["direct_arm_attempts"].as_array().unwrap().len(), 3);
        assert_eq!(smoke["arm_checks"].as_array().unwrap().len(), 5);
        assert_eq!(smoke["direct_arm_attempts"][2]["success"], true);
        assert_eq!(smoke["direct_arm_attempts"][2]["calls"], 5);
        assert_eq!(smoke["direct_arm_attempts"][2]["verification_code"], "PASS");
        assert_eq!(smoke["arm_checks"][3]["arm"], "B");
        assert_eq!(smoke["arm_checks"][3]["success"], true);
        assert_eq!(smoke["arm_checks"][4]["arm"], "C");
        assert_eq!(smoke["arm_checks"][4]["success"], true);
        let access: serde_json::Value =
            serde_json::from_str(include_str!("../reports/luna-access-check.json")).unwrap();
        assert_eq!(access["observed_opencode_requests"], 2);
        assert_eq!(access["benchmark_run"], false);
        assert!(access["usage"].is_null());
        assert!(
            access["requests"]
                .as_array()
                .unwrap()
                .iter()
                .all(|request| request["response"]["data"]["text"] == "LUNA_OK")
        );
    }

    #[test]
    fn reproducible_and_refuses_overwrite() {
        let root = env::temp_dir().join(format!(
            "verified-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        generate(&a, 2026, 10).unwrap();
        generate(&b, 2026, 10).unwrap();
        let bytes = fs::read(a.join("input.csv")).unwrap();
        assert_eq!(bytes, fs::read(b.join("input.csv")).unwrap());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(a.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest["input_sha256"],
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert_eq!(
            manifest["input_sha256"],
            "0deea8c548b685e9bd4bd1918e5e7663f1dd1688cbc63466294984975c499478"
        );
        assert_eq!(
            fs::read(a.join("manifest.json")).unwrap(),
            fs::read(b.join("manifest.json")).unwrap()
        );
        assert_eq!(String::from_utf8(bytes).unwrap().lines().count(), 11);
        assert!(generate(&a, 1, 10).is_err());
        assert!(generate(&root.join("invalid"), 1, 0).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
