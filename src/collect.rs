//! Collect summaries without silently dropping runs interrupted before summary persistence.
use crate::{
    analysis::{Result, load},
    metrics::Metrics,
    model_adapter::Usage,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

fn diagnostic(value: &Value) -> String {
    value.as_str().unwrap_or("").chars().take(500).collect()
}

pub fn collect(roots: &[PathBuf], destination: &Path) -> Result<()> {
    if roots.is_empty() {
        return Err("no run directories supplied".into());
    }
    let mut records = Vec::new();
    for root in roots {
        let trace_path = root.join("trace.jsonl");
        let mut start = None;
        let mut summary = None;
        let mut events = Vec::new();
        let mut hash = Sha256::new();
        let mut truncated = false;
        let mut metrics = Metrics::start();
        metrics.usage_complete = false;
        metrics.verifier_metrics_complete = false;
        let mut invocations = 0u64;
        let mut argument_bytes = 0u64;
        let mut peak = None::<u64>;
        let mut peak_known = true;
        if trace_path.exists() {
            let mut reader = BufReader::new(fs::File::open(&trace_path)?);
            let mut line = Vec::new();
            let mut index = 0;
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                index += 1;
                hash.update(&line);
                let event: Value = match serde_json::from_slice(&line) {
                    Ok(value) => value,
                    Err(_) if !line.ends_with(b"\n") => {
                        truncated = true;
                        break;
                    }
                    Err(error) => {
                        return Err(format!(
                            "corrupt trace {} line {index}: {error}",
                            trace_path.display()
                        )
                        .into());
                    }
                };
                match event["event"].as_str() {
                    Some("start")=>{if start.replace(event).is_some(){return Err("duplicate start event".into());}},
                    Some("summary")=>{if summary.replace(event).is_some(){return Err("duplicate summary event".into());}},
                    Some("model_request")=>metrics.calls+=1,
                    Some("tool_request")=>{invocations+=1;argument_bytes+=serde_json::to_vec(&event["args"])?.len() as u64;},
                    Some("model_response")=>{if let Ok(usage)=serde_json::from_value::<Usage>(event["reply"]["usage"].clone()){metrics.add(&usage).map_err(|e|e.to_string())?;}},
                    Some("feedback")=>{metrics.returned_feedback_bytes+=event["bytes"].as_u64().ok_or("invalid feedback count")?;},
                    Some("malformed_action")=>events.push(json!({"category":"syntax_or_parsing_failure","line":index,"detail":diagnostic(&event["error"])})),
                    Some("adapter_error")=>{
                        let detail=diagnostic(&event["error"]);
                        events.push(json!({"category":adapter_failure_category(&detail),"line":index,"detail":detail}));
                    }
                    Some("action_error")=>{
                        let detail=diagnostic(&event["error"]);
                        let category=if detail.contains("unauthorized command")||detail.contains("action unavailable") {"tool_selection_failure"}
                            else if detail.contains("invalid relative path")||detail.contains("unsafe path")||detail.contains("cannot execute commands") {"permission_or_safety_rejection"}
                            else if detail.contains("budget")||detail.contains("limit") {"resource_limit_failure"}
                            else if detail.contains("invalid command batch") {"argument_construction_failure"}else{"unclassified_action_failure"};
                        events.push(json!({"category":category,"line":index,"detail":detail}));
                    }
                    Some("verification")=>{
                        let record=&event["record"];
                        if let Some(rss)=record["process_peak_rss_bytes"].as_u64(){peak=Some(peak.unwrap_or(0).max(rss));}else{peak_known=false;}
                        if let Some(elapsed)=record["elapsed_seconds"].as_f64(){metrics.verifier_elapsed_seconds+=elapsed;}
                        if let Some(cpu)=record["cpu_seconds"].as_f64(){metrics.verifier_cpu_seconds+=cpu;}
                        let code=record["code"].as_str().unwrap_or("VERIFIER_ERROR");
                        if code!="PASS" {
                            let category=match code {"FORMAT_ERROR"=>"syntax_or_parsing_failure","SAFETY_ERROR"=>"permission_or_safety_rejection","VERIFIER_ERROR"=>"unclassified_verifier_failure",_=>"incorrect_transformation_semantics"};
                            events.push(json!({"category":category,"line":index,"detail":code,"reported_by_verifier_not_oracle":true}));
                        }
                    }
                    _=>(),
                }
            }
        }
        let trace_hash = if trace_path.exists() {
            Some(format!("{:x}", hash.finalize()))
        } else {
            None
        };
        let mut recovered_summary_file = false;
        let mut record = if root.join("record.jsonl").exists() {
            let record_path = root.join("record.jsonl");
            let loaded = match load(&record_path) {
                Ok(records) if !records.is_empty() => records,
                result => {
                    let raw = fs::read(&record_path)?;
                    let complete = summary.as_ref().map(|v| format!("{v}\n"));
                    if let (Some(complete), Some(summary)) = (complete, summary.as_ref())
                        && raw.len() < complete.len()
                        && complete.as_bytes().starts_with(&raw)
                    {
                        recovered_summary_file = true;
                        vec![summary.clone()]
                    } else {
                        return Err(result
                            .err()
                            .unwrap_or_else(|| "empty summary record".into()));
                    }
                }
            };
            if loaded.len() != 1 {
                return Err("each run directory must contain exactly one summary record".into());
            }
            if summary.as_ref().is_some_and(|s| *s != loaded[0]) {
                return Err("trace summary differs from saved record".into());
            }
            loaded.into_iter().next().unwrap()
        } else if let Some(summary) = summary {
            summary
        } else {
            let start = start.as_ref().ok_or_else(|| {
                format!("cannot recover {} without a start event", root.display())
            })?;
            let mut measured = serde_json::to_value(&metrics)?;
            for key in [
                "attempts",
                "retries",
                "tool_errors",
                "tool_output_bytes",
                "end_to_end_seconds",
                "last_verifier_ok",
                "retries_after_negative_verification",
            ] {
                measured[key] = Value::Null;
            }
            json!({"event":"summary","run_id":format!("recovered-{}",trace_hash.as_deref().ok_or("missing trace hash")?),"recovered":true,"synthetic":start["synthetic"],"arm":start["arm"],"task_id":start["task"]["id"],"task":start["task"],"config":start["config"],"repetition":start["config"]["repetition"],"model":start["config"]["model"],"decoding":start["config"]["decoding"],"pricing":start["config"]["pricing"],"termination":"interrupted","complete":false,"success":false,"oracle_pass":null,"oracle_error":"run interrupted before final scoring","metrics":measured,"input_hashes":start["task"]["input_hashes"],"final_hashes":null,"verifier_false_positive":null,"verifier_false_negative":null})
        };
        if recovered_summary_file {
            record["recovered_summary_file"] = json!(true);
        }
        if record["recovered"] == true {
            events.push(json!({"category":"unclassified_interruption","detail":"No final summary; interruption cause is unknown"}));
        }
        if let Some(start) = &start {
            if start["task"] != record["task"]
                || start["config"] != record["config"]
                || start["arm"] != record["arm"]
                || start["synthetic"] != record["synthetic"]
            {
                return Err("trace metadata differs from summary".into());
            }
            record["environment"] = start["environment"].clone();
        }
        record["metrics"]["tool_invocations"] = if trace_path.exists() {
            json!(invocations)
        } else {
            Value::Null
        };
        record["metrics"]["invocation_argument_bytes"] = if trace_path.exists() {
            json!(argument_bytes)
        } else {
            Value::Null
        };
        record["metrics"]["verifier_peak_rss_bytes_max"] =
            if peak_known && record["recovered"] != true {
                json!(peak)
            } else {
                Value::Null
            };
        if ["resource_limit", "context_limit", "call_limit"]
            .iter()
            .any(|reason| record["termination"] == *reason)
        {
            events.push(json!({"category":if record["termination"]=="resource_limit" {"resource_limit_failure"}else{"model_context_or_output_limit_failure"},"detail":record["termination"]}));
        }
        // Trusted host logs only; never recurse into the model's work directory.
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("tool-") || !name.ends_with(".json") {
                continue;
            }
            let tool: Value = serde_json::from_slice(&fs::read(entry.path())?)?;
            if tool["status"] != 0 || tool["timed_out"] == true || tool["resource_limited"] == true
            {
                events.push(json!({"category":if tool["timed_out"]==true||tool["resource_limited"]==true {"resource_limit_failure"}else{"unclassified_execution_failure"},"tool_log":name,"detail":diagnostic(&tool["stderr_bounded"])}));
            }
        }
        events.sort_by_key(Value::to_string);
        record["source_trace"] = json!(trace_path.to_string_lossy());
        record["source_trace_sha256"] = json!(trace_hash);
        record["truncated_trace"] = json!(truncated);
        record["failure_events"] = if trace_path.exists() {
            json!(events)
        } else {
            Value::Null
        };
        records.push(record);
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    for record in records {
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}
