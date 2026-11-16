//! Artificial accounting fixtures, never experimental measurements.
use serde_json::{Value, json};

pub fn config(repetitions: u64) -> Value {
    json!({"bootstrap_seed":42,"bootstrap_samples":200,"minimum_clusters":2,"expected_tasks":2,"expected_repetitions":repetitions,"non_inferiority_margin":0.05,"margin_justification":"Synthetic unit test; no preregistered practical claim","allow_synthetic":true})
}

pub fn records(repetitions: u64) -> Vec<Value> {
    let mut rows = Vec::new();
    for task in 0..2 {
        for arm in ["A", "B", "C", "D"] {
            for rep in 0..repetitions {
                let scale = if task == 0 { 1 } else { 3 };
                let total = match arm {
                    "A" => 100,
                    "B" => 80,
                    "C" => 120,
                    "D" => 50,
                    _ => unreachable!(),
                } * scale;
                let success = arm != "C" || rep % 3 != 2;
                let mut run_config: Value =
                    serde_json::from_str(include_str!("../../configs/run.example.json")).unwrap();
                run_config["model"] = json!("synthetic-fixture-model");
                run_config["repetition"] = json!(rep);
                let metadata = json!({"id":format!("task-{task}"),"family":"sorting","variant":0,"generator_version":"artificial-accounting-fixture","seed":2026,"input_bytes":if task==0{100}else{10000},"size_parameter":if task==0{10}else{1000},"complexity":"simple","expected_outcome":"success","requirement":"Artificial report fixture, no task execution or API measurement","input_hashes":{"input.csv":format!("{task:064x}")}});
                let verified = arm == "C" || arm == "D";
                let metrics = json!({"input_tokens":total-20,"output_tokens":20,"total_tokens":total,"cached_input_tokens":2,"schema_input_tokens":3,"feedback_input_tokens":10,"verification_input_tokens":if verified{8}else{0},"context_peak":total,"calls":1,"attempts":1,"retries":0,"tool_errors":0,"tool_output_bytes":10,"returned_feedback_bytes":20,"verifier_cpu_seconds":if verified{0.001}else{0.0},"verifier_elapsed_seconds":if verified{0.01}else{0.0},"verifier_metrics_complete":true,"last_verifier_ok":if verified{Some(success)}else{None},"retries_after_negative_verification":0,"end_to_end_seconds":1.0,"usage_complete":true,"monetary_cost":null});
                rows.push(json!({"event":"summary","run_id":format!("fixture-{task}-{arm}-{rep}"),"synthetic":true,"arm":arm,"task_id":metadata["id"],"task":metadata,"config":run_config,"repetition":rep,"model":run_config["model"],"decoding":run_config["decoding"],"pricing":null,"complete":true,"success":success,"oracle_pass":success,"oracle_error":null,"termination":"finished","input_hashes":metadata["input_hashes"],"verifier_false_positive":if verified{Some(false)}else{None},"verifier_false_negative":if verified{Some(false)}else{None},"metrics":metrics}));
            }
        }
    }
    rows
}
