//! Secondary observational relationships, not a causal experiment on invocation complexity.
use crate::analysis::{Config, Random, number, quantile};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn correlation(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 3 {
        return None;
    }
    let x = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
    let y = points.iter().map(|p| p.1).sum::<f64>() / points.len() as f64;
    let covariance = points.iter().map(|p| (p.0 - x) * (p.1 - y)).sum::<f64>();
    let sx = points.iter().map(|p| (p.0 - x).powi(2)).sum::<f64>();
    let sy = points.iter().map(|p| (p.1 - y).powi(2)).sum::<f64>();
    if sx <= 0.0 || sy <= 0.0 {
        None
    } else {
        Some((covariance / (sx * sy).sqrt()).clamp(-1.0, 1.0))
    }
}

fn uncertainty(points: &[Value], outcome: &str, config: &Config) -> Value {
    let mut clusters = BTreeMap::<&str, Vec<(f64, f64)>>::new();
    for point in points {
        if let (Some(x), Some(y)) = (
            point["mean_argument_bytes"].as_f64(),
            point[outcome].as_f64(),
        ) {
            clusters
                .entry(point["task_id"].as_str().unwrap())
                .or_default()
                .push((x.ln_1p(), y));
        }
    }
    let clusters: Vec<_> = clusters.into_values().collect();
    let estimate = correlation(&clusters.iter().flatten().copied().collect::<Vec<_>>());
    let mut random = Random::new(config.bootstrap_seed);
    let mut draws = Vec::new();
    if clusters.len() >= 2 && estimate.is_some() {
        for _ in 0..config.bootstrap_samples {
            let sample: Vec<_> = (0..clusters.len())
                .flat_map(|_| clusters[random.index(clusters.len())].iter().copied())
                .collect();
            if let Some(value) = correlation(&sample) {
                draws.push(value);
            }
        }
    }
    draws.sort_by(f64::total_cmp);
    json!({"estimate":estimate,"task_clusters":clusters.len(),"inference_eligible":clusters.len()>=config.minimum_clusters&&estimate.is_some(),"valid_bootstraps":draws.len(),"ci95":if draws.len()>=(config.bootstrap_samples*4).div_ceil(5){Some([quantile(&draws,0.025),quantile(&draws,0.975)])}else{None},"status":if estimate.is_none(){"not_estimable_insufficient_points_or_variation"}else{"observational_correlation_not_causation"}})
}

pub fn analyse(records: &[Value], size_points: &[Value], config: &Config) -> Value {
    let mut groups = BTreeMap::<(&str, &str), Vec<&Value>>::new();
    for record in records {
        if record["arm"] != "A" {
            groups
                .entry((
                    record["task_id"].as_str().unwrap(),
                    record["arm"].as_str().unwrap(),
                ))
                .or_default()
                .push(record);
        }
    }
    let group_count = groups.len();
    let mut points = Vec::new();
    for ((task, arm), rows) in groups {
        let args = rows
            .iter()
            .map(|r| number(r, "invocation_argument_bytes"))
            .collect::<Option<Vec<_>>>();
        let calls = rows
            .iter()
            .map(|r| number(r, "tool_invocations"))
            .collect::<Option<Vec<_>>>();
        let (Some(args), Some(calls)) = (args, calls) else {
            continue;
        };
        let errors = rows
            .iter()
            .map(|r| number(r, "tool_errors"))
            .collect::<Option<Vec<_>>>();
        points.push(json!({"task_id":task,"arm":arm,"family":rows[0]["task"]["family"],"semantic_complexity":rows[0]["task"]["complexity"],"mean_argument_bytes":args.iter().sum::<f64>()/rows.len() as f64,"mean_tool_requests":calls.iter().sum::<f64>()/rows.len() as f64,"mean_interface_errors":errors.map(|v|v.iter().sum::<f64>()/rows.len() as f64),"success_rate":rows.iter().filter(|r|r["success"]==true).count() as f64/rows.len() as f64}));
    }
    let paired: Vec<_> = points
        .iter()
        .filter(|p| p["arm"] == "D")
        .filter_map(|p| {
            let matched = size_points.iter().find(|s| s["task_id"] == p["task_id"])?;
            let mut point = p.clone();
            point["relative_token_savings_D_A"] = matched["relative_savings"].clone();
            Some(point)
        })
        .collect();
    json!({"status":if points.is_empty(){"not_estimable_missing_invocation_traces"}else{"secondary_observational_only"},"observed_tool_task_arm_groups":group_count,"unmeasured_tool_task_arm_groups":group_count-points.len(),"task_arm_points":points,"paired_D_A_points":paired,"log_argument_bytes_vs_interface_errors":uncertainty(&points,"mean_interface_errors",config),"log_argument_bytes_vs_success":uncertainty(&points,"success_rate",config),"log_argument_bytes_vs_D_A_token_savings":uncertainty(&paired,"relative_token_savings_D_A",config),"warning":"Correlations use log1p serialized argv JSON bytes and bootstrap whole tasks, retaining their arms and repetitions. Missing matrix or trace data limit the available-data summaries. Invocation choices are model-dependent and confounded by difficulty and repairs. No causal complexity penalty is established."})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direction_and_zero_variance_are_not_assumed() {
        assert!((correlation(&[(1.0, 3.0), (2.0, 2.0), (3.0, 1.0)]).unwrap() + 1.0).abs() < 1e-12);
        assert!((correlation(&[(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)]).unwrap() - 1.0).abs() < 1e-12);
        assert!(correlation(&[(1.0, 0.0), (2.0, 0.0), (3.0, 0.0)]).is_none());
    }
}
