//! Exploratory analysis only. No confirmatory inference without the future protocol freeze.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    io::{BufRead, BufReader},
    path::Path,
};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub const ARMS: [&str; 4] = ["A", "B", "C", "D"];

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
pub(crate) mod fixtures;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bootstrap_seed: u64,
    pub bootstrap_samples: usize,
    pub minimum_clusters: usize,
    pub expected_tasks: usize,
    pub expected_repetitions: u64,
    pub non_inferiority_margin: f64,
    pub margin_justification: String,
    pub allow_synthetic: bool,
}

pub fn load(path: &Path) -> Result<Vec<Value>> {
    BufReader::new(fs::File::open(path)?)
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let line = line?;
            if line.trim().is_empty() {
                return Err(format!("blank record at line {}", index + 1).into());
            }
            serde_json::from_str(&line)
                .map_err(|e| format!("invalid JSON at line {}: {e}", index + 1).into())
        })
        .collect()
}

pub fn number(record: &Value, key: &str) -> Option<f64> {
    record["metrics"][key].as_f64().filter(|n| n.is_finite())
}
fn flag(record: &Value, key: &str) -> bool {
    record[key] == true
}
fn tokens(record: &Value) -> Option<f64> {
    if record["metrics"]["usage_complete"] == true {
        number(record, "total_tokens")
    } else {
        None
    }
}

pub struct Random(u64);
impl Random {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn index(&mut self, bound: usize) -> usize {
        let bound = bound as u64;
        let threshold = bound.wrapping_neg() % bound;
        loop {
            self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
            let mut value = self.0;
            value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
            value ^= value >> 31;
            if value >= threshold {
                return (value % bound) as usize;
            }
        }
    }
}

pub fn quantile(sorted: &[f64], p: f64) -> f64 {
    let index = p * (sorted.len() - 1) as f64;
    let lower = index.floor() as usize;
    let upper = index.ceil() as usize;
    sorted[lower] + (sorted[upper] - sorted[lower]) * (index - lower as f64)
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let values: Vec<_> = values.collect();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn uncertainty(values: &[f64], config: &Config) -> Value {
    if values.len() < 2 {
        return json!({"estimate":mean(values.iter().copied()),"ci95":null,"one_sided95_lower":null,"clusters":values.len()});
    }
    let mut random = Random::new(config.bootstrap_seed);
    let mut draws = Vec::with_capacity(config.bootstrap_samples);
    for _ in 0..config.bootstrap_samples {
        draws.push(
            (0..values.len())
                .map(|_| values[random.index(values.len())])
                .sum::<f64>()
                / values.len() as f64,
        );
    }
    draws.sort_by(f64::total_cmp);
    json!({"estimate":mean(values.iter().copied()),"ci95":[quantile(&draws,0.025),quantile(&draws,0.975)],"one_sided95_lower":quantile(&draws,0.05),"clusters":values.len(),"resamples":draws.len()})
}

fn aggregate(rows: &[&Value]) -> Value {
    let successes = rows.iter().filter(|r| flag(r, "success")).count();
    let total_known = rows
        .iter()
        .filter_map(|r| number(r, "total_tokens"))
        .sum::<f64>();
    let exact = rows.iter().all(|r| tokens(r).is_some());
    let mut metrics = serde_json::Map::new();
    for key in [
        "input_tokens",
        "output_tokens",
        "cached_input_tokens",
        "schema_input_tokens",
        "feedback_input_tokens",
        "verification_input_tokens",
        "tool_output_bytes",
        "returned_feedback_bytes",
        "verifier_cpu_seconds",
        "verifier_elapsed_seconds",
        "tool_errors",
        "attempts",
        "retries",
        "retries_after_negative_verification",
        "monetary_cost",
        "tool_invocations",
        "invocation_argument_bytes",
    ] {
        let values: Vec<_> = rows.iter().filter_map(|r| number(r, key)).collect();
        let measured = values.len();
        let reliable = if key == "verifier_cpu_seconds" {
            rows.iter()
                .all(|r| r["metrics"]["verifier_metrics_complete"] == true)
        } else if key == "verifier_elapsed_seconds" {
            rows.iter().all(|r| r["recovered"] != true)
        } else if [
            "input_tokens",
            "output_tokens",
            "cached_input_tokens",
            "schema_input_tokens",
            "feedback_input_tokens",
            "verification_input_tokens",
            "monetary_cost",
        ]
        .contains(&key)
        {
            exact
        } else {
            true
        };
        metrics.insert(key.into(),json!({"sum":if measured==rows.len()&&reliable {Some(values.iter().sum::<f64>())} else {None},"known_sum":values.iter().sum::<f64>(),"measured_runs":measured,"missing_runs":rows.len()-measured}));
    }
    for key in [
        "context_peak",
        "end_to_end_seconds",
        "verifier_peak_rss_bytes_max",
    ] {
        let values: Vec<_> = rows.iter().filter_map(|r| number(r, key)).collect();
        metrics.insert(key.into(),json!({"mean":if key=="context_peak"&&!exact{None}else{mean(values.iter().copied())},"maximum":if key=="context_peak"&&!exact{None}else{values.iter().copied().reduce(f64::max)},"known_maximum":values.iter().copied().reduce(f64::max),"measured_runs":values.len(),"missing_runs":rows.len()-values.len()}));
    }
    json!({"runs":rows.len(),"successes":successes,"success_rate":successes as f64/rows.len() as f64,"incomplete_runs":rows.iter().filter(|r|!flag(r,"complete")).count(),"oracle_unknown_runs":rows.iter().filter(|r|r["oracle_pass"].is_null()).count(),"usage_incomplete_runs":rows.iter().filter(|r|tokens(r).is_none()).count(),"known_total_tokens":total_known,"total_tokens":if exact {Some(total_known)} else {None},"mean_tokens":if exact {Some(total_known/rows.len() as f64)} else {None},"token_efficiency":if exact&&successes>0 {Some(total_known/successes as f64)} else {None},"token_efficiency_reason":if successes==0 {Some("zero_successful_runs")} else if !exact {Some("incomplete_usage")} else {None},"metrics":metrics})
}

fn validate(records: &[Value], config: &Config) -> Result<()> {
    if records.is_empty() {
        return Err("no run records".into());
    }
    if config.bootstrap_samples < 200
        || config.bootstrap_samples > 100_000
        || config.minimum_clusters < 2
        || !(1..=10000).contains(&config.expected_tasks)
        || !(1..=100).contains(&config.expected_repetitions)
        || !(0.0..=0.2).contains(&config.non_inferiority_margin)
        || config.margin_justification.trim().is_empty()
    {
        return Err("invalid analysis configuration".into());
    }
    let first = &records[0];
    let mut base = first["config"].clone();
    base.as_object_mut()
        .ok_or("missing run config")?
        .remove("repetition");
    let run_config: crate::orchestrator::Config = serde_json::from_value(first["config"].clone())?;
    crate::orchestrator::validate(&run_config)?;
    let mut ids = BTreeSet::new();
    let mut cells = BTreeSet::new();
    let mut tasks = BTreeMap::new();
    let mut instances = BTreeMap::new();
    let synthetic = first["synthetic"]
        .as_bool()
        .ok_or("missing synthetic flag")?;
    if synthetic && !config.allow_synthetic {
        return Err(
            "synthetic input requires allow_synthetic=true; never treat fixtures as measurements"
                .into(),
        );
    }
    let mut count_sum = 0u64;
    let mut numeric_sums = BTreeMap::<&str, f64>::new();
    for record in records {
        let id = record["run_id"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or("missing run_id")?;
        let task = record["task_id"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or("missing task_id")?;
        let arm = record["arm"].as_str().ok_or("missing arm")?;
        let repetition = record["repetition"].as_u64().ok_or("missing repetition")?;
        if record["event"] != "summary"
            || !ARMS.contains(&arm)
            || repetition >= config.expected_repetitions
        {
            return Err("not a summary, invalid arm, or repetition outside planned range".into());
        }
        if !ids.insert(id) || !cells.insert((task, arm, repetition)) {
            return Err("duplicate run or task/arm/repetition cell".into());
        }
        if record["synthetic"].as_bool() != Some(synthetic) {
            return Err("mixed synthetic and real records are forbidden".into());
        }
        let mut settings = record["config"].clone();
        settings
            .as_object_mut()
            .ok_or("missing run config")?
            .remove("repetition");
        if settings != base
            || record["model"] != first["model"]
            || record["decoding"] != first["decoding"]
            || record["model"] != record["config"]["model"]
            || record["decoding"] != record["config"]["decoding"]
            || record["config"]["repetition"] != repetition
        {
            return Err("incompatible model, decoding, run limits or repetition settings".into());
        }
        if let Some(previous) = tasks.insert(task, &record["task"])
            && previous != &record["task"]
        {
            return Err("task definitions changed between runs".into());
        }
        if record["input_hashes"] != record["task"]["input_hashes"]
            || !record["input_hashes"].is_object()
        {
            return Err("missing or inconsistent input hashes".into());
        }
        if !record["task"]["requirement"].is_string() {
            return Err("missing semantic task requirement".into());
        }
        let signature =
            json!({"inputs":record["input_hashes"],"requirement":record["task"]["requirement"]})
                .to_string();
        if instances
            .insert(signature, task)
            .is_some_and(|previous| previous != task)
        {
            return Err(
                "identical inputs and requirements assigned multiple task-instance IDs".into(),
            );
        }
        if record["task"]["id"] != task
            || record["task"]["input_bytes"]
                .as_u64()
                .is_none_or(|n| n == 0)
            || record["task"]["size_parameter"]
                .as_u64()
                .is_none_or(|n| n == 0)
            || !record["task"]["family"].is_string()
            || !["simple", "multi-step"]
                .iter()
                .any(|v| record["task"]["complexity"] == *v)
            || !record["task"]["expected_outcome"].is_string()
        {
            return Err("missing task strata or input-size metadata".into());
        }
        for key in ["success", "complete"] {
            if !record[key].is_boolean() {
                return Err(format!("missing {key} flag").into());
            }
        }
        if !record["oracle_pass"].is_boolean() && !record["oracle_pass"].is_null() {
            return Err("invalid oracle outcome".into());
        }
        if flag(record, "success") != (flag(record, "complete") && record["oracle_pass"] == true) {
            return Err("inconsistent success/completeness/oracle flags".into());
        }
        if !record["metrics"]["last_verifier_ok"].is_boolean()
            && !record["metrics"]["last_verifier_ok"].is_null()
        {
            return Err("invalid verifier judgment".into());
        }
        if ["A", "B"].contains(&arm)
            && (record["metrics"]["last_verifier_ok"].is_boolean()
                || number(record, "verification_input_tokens").is_some_and(|v| v > 0.0))
        {
            return Err("verification exposed in an arm without a verifier".into());
        }
        if let (Some(last), Some(oracle)) = (
            record["metrics"]["last_verifier_ok"].as_bool(),
            record["oracle_pass"].as_bool(),
        ) {
            if record["verifier_false_positive"] != (last && !oracle)
                || record["verifier_false_negative"] != (!last && oracle)
            {
                return Err("inconsistent verifier disagreement flags".into());
            }
        } else if !record["verifier_false_positive"].is_null()
            || !record["verifier_false_negative"].is_null()
        {
            return Err("verifier disagreement without both judgments".into());
        }
        if !record["metrics"]["usage_complete"].is_boolean() {
            return Err("missing usage completeness flag".into());
        }
        let mut counts = Vec::new();
        for key in ["input_tokens", "output_tokens", "total_tokens"] {
            let value = record["metrics"][key]
                .as_u64()
                .ok_or("missing integer usage")?;
            if value > 9_007_199_254_740_991 {
                return Err("token count exceeds exact f64 integer range".into());
            }
            counts.push(value);
        }
        if counts[0].checked_add(counts[1]) != Some(counts[2]) {
            return Err("token total differs from input + output".into());
        }
        if record["metrics"]["usage_complete"] == true
            && number(record, "context_peak").is_some_and(|n| n > counts[2] as f64)
        {
            return Err("context peak exceeds complete run token total".into());
        }
        count_sum = count_sum
            .checked_add(counts[2])
            .ok_or("token sum overflow")?;
        if count_sum > 9_007_199_254_740_991 {
            return Err("token sum exceeds exact f64 integer range".into());
        }
        for key in [
            "cached_input_tokens",
            "feedback_input_tokens",
            "verification_input_tokens",
            "schema_input_tokens",
        ] {
            if !record["metrics"][key].is_null()
                && record["metrics"][key]
                    .as_u64()
                    .is_none_or(|n| n > counts[0])
            {
                return Err(format!("invalid {key}").into());
            }
        }
        for key in [
            "context_peak",
            "calls",
            "attempts",
            "retries",
            "tool_errors",
            "tool_output_bytes",
            "returned_feedback_bytes",
            "retries_after_negative_verification",
            "tool_invocations",
            "invocation_argument_bytes",
            "verifier_peak_rss_bytes_max",
        ] {
            if !record["metrics"][key].is_null() && record["metrics"][key].as_u64().is_none() {
                return Err(format!("noninteger count: {key}").into());
            }
        }
        if record["pricing"] != record["config"]["pricing"] {
            return Err("inconsistent pricing assumptions".into());
        }
        if let Some(cost) = number(record, "monetary_cost") {
            let pricing = run_config
                .pricing
                .as_ref()
                .ok_or("cost without recorded pricing")?;
            let cached = record["metrics"]["cached_input_tokens"]
                .as_f64()
                .ok_or("cost without cache accounting")?;
            if record["metrics"]["usage_complete"] != true {
                return Err("cost with incomplete usage".into());
            }
            let expected = ((counts[0] as f64 - cached) * pricing.input_per_million
                + cached * pricing.cached_input_per_million
                + counts[1] as f64 * pricing.output_per_million)
                / 1e6;
            if !expected.is_finite() || (cost - expected).abs() > 1e-9 * expected.abs().max(1.0) {
                return Err("cost differs from recorded pricing and usage".into());
            }
        }
        for (key, value) in record["metrics"].as_object().ok_or("missing metrics")? {
            if value.is_number() {
                let number = value
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .ok_or_else(|| format!("invalid nonnegative metric {key}"))?;
                let total = numeric_sums.entry(key).or_default();
                *total += number;
                if !total.is_finite() {
                    return Err(format!("metric sum overflow: {key}").into());
                }
            }
        }
    }
    if tasks.len() > config.expected_tasks {
        return Err("more task instances than planned".into());
    }
    Ok(())
}

fn comparison(
    records: &[Value],
    reference: &str,
    treatment: &str,
    config: &Config,
    matrix_complete: bool,
) -> Value {
    let mut groups: BTreeMap<&str, BTreeMap<u64, BTreeMap<&str, &Value>>> = BTreeMap::new();
    for record in records {
        groups
            .entry(record["task_id"].as_str().unwrap())
            .or_default()
            .entry(record["repetition"].as_u64().unwrap())
            .or_default()
            .insert(record["arm"].as_str().unwrap(), record);
    }
    let (mut delta, mut success, mut savings) = (Vec::new(), Vec::new(), Vec::new());
    let (mut pair_count, mut unknown, mut undefined, mut missing) = (0, 0, 0, 0);
    let mut points = Vec::new();
    let extra_names = [
        "end_to_end_seconds",
        "context_peak",
        "attempts",
        "tool_output_bytes",
        "monetary_cost",
        "verification_input_tokens",
        "verifier_cpu_seconds",
        "verifier_elapsed_seconds",
    ];
    let mut extra: BTreeMap<&str, Vec<f64>> =
        extra_names.iter().map(|name| (*name, Vec::new())).collect();
    let mut extra_missing: BTreeMap<&str, usize> =
        extra_names.iter().map(|name| (*name, 0)).collect();
    for (task, repetitions) in groups {
        let (mut local_delta, mut local_success, mut local_savings) =
            (Vec::new(), Vec::new(), Vec::new());
        let (mut base_tokens, mut target_tokens) = (Vec::new(), Vec::new());
        let mut metadata = None;
        let (mut base_success, mut target_success) = (0usize, 0usize);
        let mut local_extra: BTreeMap<&str, Vec<f64>> =
            extra_names.iter().map(|name| (*name, Vec::new())).collect();
        for rep in 0..config.expected_repetitions {
            let Some(pair) = repetitions.get(&rep) else {
                missing += 1;
                continue;
            };
            let (Some(base), Some(target)) = (pair.get(reference), pair.get(treatment)) else {
                missing += 1;
                continue;
            };
            pair_count += 1;
            metadata = Some(&base["task"]);
            base_success += usize::from(flag(base, "success"));
            target_success += usize::from(flag(target, "success"));
            for name in extra_names {
                let reliable = if name == "context_peak" {
                    tokens(base).is_some() && tokens(target).is_some()
                } else if name == "verifier_cpu_seconds" {
                    base["metrics"]["verifier_metrics_complete"] == true
                        && target["metrics"]["verifier_metrics_complete"] == true
                } else if name == "verifier_elapsed_seconds" {
                    base["recovered"] != true && target["recovered"] != true
                } else {
                    true
                };
                if let (Some(a), Some(d), true) =
                    (number(base, name), number(target, name), reliable)
                {
                    local_extra.get_mut(name).unwrap().push(d - a);
                } else {
                    *extra_missing.get_mut(name).unwrap() += 1;
                }
            }
            local_success.push(
                f64::from(u8::from(flag(target, "success")))
                    - f64::from(u8::from(flag(base, "success"))),
            );
            if let (Some(a), Some(d)) = (tokens(base), tokens(target)) {
                local_delta.push(d - a);
                base_tokens.push(a);
                target_tokens.push(d);
                if a > 0.0 {
                    local_savings.push(1.0 - d / a);
                } else {
                    undefined += 1;
                }
            } else {
                unknown += 1;
            }
        }
        let cluster_usage_complete = local_delta.len() == local_success.len();
        let cluster_pairs = local_success.len();
        if let Some(value) = mean(local_success.into_iter()) {
            success.push(value);
        }
        if let Some(value) = mean(local_delta.into_iter()) {
            delta.push(value);
        }
        if let Some(value) = mean(local_savings.into_iter()) {
            savings.push(value);
        }
        for (name, values) in local_extra {
            if let Some(value) = mean(values.into_iter()) {
                extra.get_mut(name).unwrap().push(value);
            }
        }
        if let (Some(metadata), Some(a), Some(d)) = (
            metadata,
            mean(base_tokens.into_iter()),
            mean(target_tokens.into_iter()),
        ) && cluster_usage_complete
        {
            points.push(json!({"task_id":task,"family":metadata["family"],"complexity":metadata["complexity"],"expected_outcome":metadata["expected_outcome"],"input_bytes":metadata["input_bytes"],"reference_mean_tokens":a,"treatment_mean_tokens":d,"reference_success_rate":base_success as f64/cluster_pairs as f64,"treatment_success_rate":target_success as f64/cluster_pairs as f64,"relative_savings":if a>0.0 {Some(1.0-d/a)} else {None}}));
        }
    }
    let success_interval = uncertainty(&success, config);
    let mut ni_lower = success_interval["one_sided95_lower"].as_f64();
    // No observed worse cluster does not establish zero risk on unseen tasks.
    if !success.is_empty()
        && (success.iter().all(|v| *v >= 0.0) || success.iter().all(|v| *v == success[0]))
    {
        let minimum = success.iter().copied().reduce(f64::min).unwrap();
        let covered = 0.05_f64.powf(1.0 / success.len() as f64);
        let conservative = minimum * covered - (1.0 - covered);
        ni_lower = ni_lower.map(|bound| bound.min(conservative));
    }
    let exact = unknown == 0;
    let token_interval = if exact {
        uncertainty(&delta, config)
    } else {
        json!({"estimate":null,"ci95":null,"reason":"incomplete_usage; known lower bounds not substituted for totals"})
    };
    let relative_interval = if exact && undefined == 0 {
        uncertainty(&savings, config)
    } else {
        json!({"estimate":null,"ci95":null,"reason":"incomplete_usage_or_zero_reference_tokens"})
    };
    let enough = success.len() >= config.minimum_clusters && matrix_complete;
    let ni = enough && ni_lower.is_some_and(|v| v > -config.non_inferiority_margin);
    let fewer = enough && token_interval["ci95"][1].as_f64().is_some_and(|v| v < 0.0);
    let other = extra
        .into_iter()
        .map(|(name, values)| {
            (
                name,
                if extra_missing[name] == 0 {
                    uncertainty(&values, config)
                } else {
                    json!({"estimate":null,"ci95":null,"missing_pairs":extra_missing[name]})
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    json!({"reference":reference,"treatment":treatment,"pairs":pair_count,"task_clusters":success.len(),"missing_pairs_for_observed_tasks":missing,"unknown_usage_pairs":unknown,"zero_reference_token_pairs":undefined,"delta_tokens":token_interval,"delta_success":success_interval,"paired_relative_savings":relative_interval,"other_paired_deltas":other,"non_inferiority_lower_bound":ni_lower,"non_inferiority_margin":config.non_inferiority_margin,"non_inferiority_supported_exploratory":ni,"token_reduction_supported_exploratory":fewer,"overall_support_exploratory":ni&&fewer,"inference_eligible":enough,"size_points":points})
}

fn analyse_inner(records: &[Value], config: &Config) -> Result<Value> {
    validate(records, config)?;
    let synthetic = records[0]["synthetic"].as_bool().unwrap();
    let expected = config
        .expected_tasks
        .checked_mul(4)
        .and_then(|n| n.checked_mul(config.expected_repetitions as usize))
        .ok_or("matrix size overflow")?;
    let task_ids: BTreeSet<_> = records
        .iter()
        .map(|r| r["task_id"].as_str().unwrap())
        .collect();
    let matrix_complete = records.len() == expected;
    let mut arms = serde_json::Map::new();
    for arm in ARMS {
        let rows: Vec<_> = records.iter().filter(|r| r["arm"] == arm).collect();
        arms.insert(arm.into(),if rows.is_empty(){json!({"runs":0,"successes":0,"success_rate":null,"total_tokens":null,"token_efficiency":null})}else{aggregate(&rows)});
    }
    let mut strata = serde_json::Map::new();
    for field in ["family", "size_parameter", "complexity", "expected_outcome"] {
        let mut groups: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
        for record in records {
            let label = record["task"][field]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| record["task"][field].to_string());
            groups
                .entry(format!("{label} / {}", record["arm"].as_str().unwrap()))
                .or_default()
                .push(record);
        }
        strata.insert(
            field.into(),
            json!(
                groups
                    .into_iter()
                    .map(|(name, rows)| (name, aggregate(&rows)))
                    .collect::<BTreeMap<_, _>>()
            ),
        );
    }
    let comparisons: Vec<_> = [("A", "D"), ("A", "B"), ("B", "C"), ("C", "D"), ("B", "D")]
        .into_iter()
        .map(|(base, target)| comparison(records, base, target, config, matrix_complete))
        .collect();
    let primary = &comparisons[0];
    let boundary = if primary["unknown_usage_pairs"].as_u64() == Some(0) {
        crate::boundary::fit(primary["size_points"].as_array().unwrap(), config)
    } else {
        json!({"status":"not_estimable_incomplete_usage"})
    };
    let mut terminations = BTreeMap::new();
    let mut examples = BTreeMap::<String, Vec<Value>>::new();
    for record in records {
        let reason = record["termination"]
            .as_str()
            .ok_or("missing termination")?
            .to_owned();
        *terminations.entry(reason.clone()).or_insert(0usize) += 1;
        if !flag(record, "success") {
            let sample = examples.entry(reason).or_default();
            if sample.len() < 3 {
                sample.push(json!({"run_id":record["run_id"],"task_id":record["task_id"],"arm":record["arm"],"termination":record["termination"],"oracle_pass":record["oracle_pass"],"oracle_error":record["oracle_error"]}));
            }
        }
    }
    let mut missing = Vec::new();
    let cells: BTreeSet<_> = records
        .iter()
        .map(|r| {
            (
                r["task_id"].as_str().unwrap(),
                r["arm"].as_str().unwrap(),
                r["repetition"].as_u64().unwrap(),
            )
        })
        .collect();
    for task in &task_ids {
        for arm in ARMS {
            for rep in 0..config.expected_repetitions {
                if !cells.contains(&(*task, arm, rep)) {
                    missing.push(json!({"task_id":task,"arm":arm,"repetition":rep}));
                }
            }
        }
    }
    let protocol_violations = records
        .iter()
        .filter(|r| {
            r["termination"] == "model_configuration_mismatch" || r["termination"] == "usage_error"
        })
        .count();
    let h1 = if synthetic {
        "not_evaluated_synthetic"
    } else if !matrix_complete || protocol_violations > 0 {
        "not_established_protocol_incomplete"
    } else if primary["overall_support_exploratory"] == true {
        "exploratory_support_not_confirmatory"
    } else if primary["non_inferiority_supported_exploratory"] != true {
        "non_inferiority_not_established"
    } else {
        "token_reduction_not_established"
    };
    Ok(
        json!({"analysis_version":"exploratory-v1","synthetic":synthetic,"confirmatory":false,"config":config,"model":records[0]["model"],"decoding":records[0]["decoding"],"run_config":records[0]["config"],"records":records.len(),"expected_records":expected,"matrix_complete":matrix_complete,"observed_tasks":task_ids.len(),"missing_cells":missing,"unidentified_missing_tasks":config.expected_tasks-task_ids.len(),"protocol_violation_runs":protocol_violations,"arms":arms,"strata":strata,"comparisons":comparisons,"boundary":boundary,"terminations":terminations,"failure_examples":examples,"verifier_disagreement":{"false_positive":records.iter().filter(|r|r["verifier_false_positive"]==true).count(),"false_negative":records.iter().filter(|r|r["verifier_false_negative"]==true).count(),"evaluated_runs":records.iter().filter(|r|r["verifier_false_positive"].is_boolean()&&r["verifier_false_negative"].is_boolean()).count()},"hypotheses":{"H1":h1,"H2":"exploratory_stratum_specific_boundary_model; no universal claim","H3":"use C-B and D-C comparisons plus separately measured verification tax; missing attribution is not zero","H4":"secondary exploratory: inspect complexity strata and error traces; tool-selection details are unavailable in summary-only data"},"limitations":["Protocol and margin are not frozen: all inference is exploratory.","Bootstrap resamples task instances, retaining all paired repetitions within each cluster; repeated runs are not independent dataset instances.","Equal task weighting is used for paired effects; arm summaries include every supplied run, including failures and incomplete runs.","Missing usage suppresses exact token comparisons and efficiency. Reported known totals are lower bounds, not imputed zeros.","Non-inferiority is separate from token reduction; no-worse-observed clusters use a conservative zero-event risk guard.","Related semantic variants at multiple sizes may remain correlated across task clusters; interpret task-instance bootstrap uncertainty conditionally.","Input bytes are an artifact-size proxy, not measured final-output bytes.","Summary-only data cannot distinguish every tool-selection, argument, parsing or permission error. Consult host traces; do not infer absent errors from unclassified counts.","Synthetic cohorts are fixture validation only and cannot support scientific conclusions."]}),
    )
}

pub fn analyse(records: &[Value], config: &Config) -> Result<Value> {
    let mut result = analyse_inner(records, config)?;
    result["limitations"].as_array_mut().unwrap().push(json!("The size model includes failed and incomplete paired runs. It estimates token cost, not correctness-adjusted efficiency; per-task success rates accompany its points."));
    if result["protocol_violation_runs"] != 0 {
        for comparison in result["comparisons"].as_array_mut().unwrap() {
            for flag in [
                "inference_eligible",
                "non_inferiority_supported_exploratory",
                "token_reduction_supported_exploratory",
                "overall_support_exploratory",
            ] {
                comparison[flag] = json!(false);
            }
        }
    }
    let synthetic = result["synthetic"] == true;
    let mut stratum_comparisons = BTreeMap::new();
    for field in ["family", "size_parameter", "complexity", "expected_outcome"] {
        let mut groups = BTreeMap::<String, Vec<Value>>::new();
        for record in records {
            let label = record["task"][field]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| record["task"][field].to_string());
            groups.entry(label).or_default().push(record.clone());
        }
        let results = groups
            .into_iter()
            .map(|(label, rows)| {
                let n = rows
                    .iter()
                    .map(|r| r["task_id"].as_str().unwrap())
                    .collect::<BTreeSet<_>>()
                    .len();
                let full = result["matrix_complete"] == true
                    && result["protocol_violation_runs"] == 0
                    && rows.len() == n * 4 * config.expected_repetitions as usize;
                (
                    label,
                    [("A", "D"), ("A", "B"), ("B", "C"), ("C", "D"), ("B", "D")]
                        .into_iter()
                        .map(|(a, b)| comparison(&rows, a, b, config, full))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        stratum_comparisons.insert(field, results);
    }
    result["stratum_comparisons"] = json!(stratum_comparisons);
    let positive = records
        .iter()
        .filter(|r| r["metrics"]["last_verifier_ok"].is_boolean() && r["oracle_pass"] == true)
        .count();
    let negative = records
        .iter()
        .filter(|r| r["metrics"]["last_verifier_ok"].is_boolean() && r["oracle_pass"] == false)
        .count();
    result["verifier_disagreement"]["oracle_positive_runs"] = json!(positive);
    result["verifier_disagreement"]["oracle_negative_runs"] = json!(negative);
    result["verifier_disagreement"]["tool_arm_runs_without_evaluable_final_pair"] = json!(
        records
            .iter()
            .filter(|r| r["arm"] == "C" || r["arm"] == "D")
            .count()
            - positive
            - negative
    );
    result["verifier_disagreement"]["false_positive_rate"] = json!((negative > 0).then(|| {
        result["verifier_disagreement"]["false_positive"]
            .as_f64()
            .unwrap()
            / negative as f64
    }));
    result["verifier_disagreement"]["false_negative_rate"] = json!((positive > 0).then(|| {
        result["verifier_disagreement"]["false_negative"]
            .as_f64()
            .unwrap()
            / positive as f64
    }));
    result["hypotheses"]["H2"] = json!({"status":if synthetic {"not_evaluated_synthetic"}else{"exploratory_size_interactions_only"},"boundary_status":result["boundary"]["status"],"warning":"Size trends and crossovers are stratum-specific; no universal advantage or out-of-range crossover is asserted."});
    let raw = &result["comparisons"][2];
    let compressed = &result["comparisons"][3];
    let verification_rows: Vec<_> = records
        .iter()
        .filter(|r| r["arm"] == "C" || r["arm"] == "D")
        .collect();
    result["hypotheses"]["H3"] = json!({"status":if synthetic {"not_evaluated_synthetic"}else{"exploratory_only"},"compression_reduces_tokens":compressed["token_reduction_supported_exploratory"],"raw_verification_increases_tokens":raw["inference_eligible"]==true&&raw["delta_tokens"]["ci95"][0].as_f64().is_some_and(|v|v>0.0),"raw_verification_delta":raw["delta_tokens"],"compression_delta":compressed["delta_tokens"],"verification_input_attribution_complete":!verification_rows.is_empty()&&verification_rows.iter().all(|r|number(r,"verification_input_tokens").is_some()&&tokens(r).is_some()),"warning":"Execution CPU and latency are not LLM tokens. Attribution, runtime, resources and observed retries are separate quantities; compression is not assumed beneficial."});
    result["hypotheses"]["H4"] = json!({"status":if synthetic {"not_evaluated_synthetic"}else{"secondary_exploratory_only"},"invocation_measured_runs":records.iter().filter(|r|r["arm"]!="A"&&number(r,"tool_invocations").is_some()).count(),"semantic_complexity_strata":result["strata"]["complexity"],"warning":"Task semantic complexity is not measured CLI invocation complexity. Collect host traces to obtain invocation count/argument volume; no causal claim follows from these observational strata."});
    result["invocation_analysis"] = crate::invocation::analyse(
        records,
        result["comparisons"][0]["size_points"].as_array().unwrap(),
        config,
    );
    result["hypotheses"]["H4"]["invocation_relationships"] = result["invocation_analysis"].clone();
    let classified = records
        .iter()
        .filter(|r| r["failure_events"].is_array())
        .count();
    let mut events: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for record in records {
        if let Some(failures) = record["failure_events"].as_array() {
            for failure in failures {
                let category = failure["category"]
                    .as_str()
                    .ok_or("invalid failure event category")?;
                events.entry(category.into()).or_default().push(json!({"run_id":record["run_id"],"task_id":record["task_id"],"arm":record["arm"],"trace":record["source_trace"],"event":failure}));
            }
        }
    }
    for category in [
        "tool_selection_failure",
        "argument_construction_failure",
        "syntax_or_parsing_failure",
        "permission_or_safety_rejection",
        "incorrect_transformation_semantics",
        "resource_limit_failure",
        "model_context_or_output_limit_failure",
    ] {
        events.entry(category.into()).or_default();
    }
    let mut taxonomy: BTreeMap<_,_>=events.into_iter().map(|(category,events)|(category,json!({"unit":"encountered_events_not_mutually_exclusive_final_run_failures","events":if classified==records.len(){Some(events.len())}else{None},"known_events":events.len(),"examples":events.into_iter().take(3).collect::<Vec<_>>()}))).collect();
    for (category, field) in [
        ("verifier_false_positive", "false_positive"),
        ("verifier_false_negative", "false_negative"),
    ] {
        taxonomy.insert(category.into(),json!({"unit":"evaluable_final_verifier_oracle_runs","events":result["verifier_disagreement"][field],"evaluated_runs":result["verifier_disagreement"]["evaluated_runs"]}));
    }
    result["failure_taxonomy"] = json!(taxonomy);
    result["trace_classified_runs"] = json!(classified);
    let mut negative = Vec::new();
    for comparison in result["comparisons"].as_array().unwrap() {
        let label = format!(
            "{} versus {}",
            comparison["treatment"].as_str().unwrap(),
            comparison["reference"].as_str().unwrap()
        );
        if comparison["delta_tokens"]["estimate"]
            .as_f64()
            .is_some_and(|v| v > 0.0)
        {
            negative.push(format!("{label}: token consumption increased at the point estimate. Inspect its clustered interval before claiming an increase."));
        }
        if comparison["delta_success"]["estimate"]
            .as_f64()
            .is_some_and(|v| v < 0.0)
        {
            negative.push(format!("{label}: success decreased at the point estimate. Token savings do not compensate for correctness loss."));
        }
    }
    if result["comparisons"][0]["non_inferiority_supported_exploratory"] != true {
        negative.push("Success non-inferiority is not established; lower token consumption alone cannot establish H1.".into());
    }
    if !records.iter().all(|r| tokens(r).is_some()) {
        negative.push("Incomplete usage prevents exact token-efficiency and paired-token conclusions for affected comparisons.".into());
    }
    for arm in ARMS {
        if result["arms"][arm]["runs"] != 0 && result["arms"][arm]["successes"] == 0 {
            negative.push(format!(
                "Arm {arm}: zero successful runs; token efficiency is undefined."
            ));
        }
    }
    result["negative_findings_or_limits"] = json!(negative);
    result["environments"] = json!(
        records
            .iter()
            .filter(|r| !r["environment"].is_null())
            .map(|r| r["environment"].to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|v| serde_json::from_str::<Value>(&v).unwrap())
            .collect::<Vec<_>>()
    );
    result["environment_missing_runs"] = json!(
        records
            .iter()
            .filter(|r| r["environment"].is_null())
            .count()
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(repetitions: u64) -> Config {
        serde_json::from_value(fixtures::config(repetitions)).unwrap()
    }
    #[test]
    fn efficiency_includes_failures_and_clusters_not_repetitions() {
        let report = analyse(&fixtures::records(3), &config(3)).unwrap();
        assert_eq!(report["arms"]["A"]["total_tokens"], 1200.0);
        assert_eq!(report["arms"]["C"]["successes"], 4);
        assert_eq!(report["arms"]["C"]["token_efficiency"], 360.0);
        assert_eq!(report["comparisons"][0]["delta_tokens"]["estimate"], -100.0);
        assert_eq!(
            report["comparisons"][0]["paired_relative_savings"]["estimate"],
            0.5
        );
        assert_eq!(
            report["comparisons"][0]["delta_tokens"]["ci95"],
            json!([-150.0, -50.0])
        );
        let repeated = analyse(&fixtures::records(6), &config(6)).unwrap();
        assert_eq!(
            report["comparisons"][0]["delta_tokens"],
            repeated["comparisons"][0]["delta_tokens"]
        );
        assert_eq!(report["hypotheses"]["H1"], "not_evaluated_synthetic");
        assert_eq!(
            report["comparisons"][0]["non_inferiority_supported_exploratory"],
            false
        );
    }
    #[test]
    fn zero_success_unknown_usage_and_missing_cells_are_explicit() {
        let mut rows = fixtures::records(3);
        for row in &mut rows {
            if row["arm"] == "D" {
                row["success"] = json!(false);
                row["oracle_pass"] = json!(false);
                row["metrics"]["last_verifier_ok"] = json!(false);
            }
        }
        let report = analyse(&rows, &config(3)).unwrap();
        assert!(report["arms"]["D"]["token_efficiency"].is_null());
        assert_eq!(
            report["arms"]["D"]["token_efficiency_reason"],
            "zero_successful_runs"
        );
        rows[0]["metrics"]["usage_complete"] = json!(false);
        let report = analyse(&rows, &config(3)).unwrap();
        assert!(report["comparisons"][0]["delta_tokens"]["estimate"].is_null());
        assert_eq!(report["arms"]["A"]["known_total_tokens"], 1200.0);
        assert!(report["arms"]["A"]["total_tokens"].is_null());
        rows.pop();
        let report = analyse(&rows, &config(3)).unwrap();
        assert_eq!(report["matrix_complete"], false);
        assert_eq!(report["missing_cells"].as_array().unwrap().len(), 1);
        assert_eq!(report["comparisons"][0]["inference_eligible"], false);
    }
    #[test]
    fn invalid_and_mixed_cohorts_are_rejected_instead_of_filtered() {
        let rows = fixtures::records(3);
        let mut bad = rows.clone();
        bad.push(rows[0].clone());
        assert!(
            analyse(&bad, &config(3))
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        let mut bad = rows.clone();
        bad[0]["synthetic"] = json!(false);
        assert!(
            analyse(&bad, &config(3))
                .unwrap_err()
                .to_string()
                .contains("mixed")
        );
        let mut bad = rows.clone();
        bad[0]["metrics"]["total_tokens"] = json!(101);
        assert!(analyse(&bad, &config(3)).is_err());
        let mut bad = rows.clone();
        bad[0]["decoding"]["temperature"] = json!(0.5);
        assert!(analyse(&bad, &config(3)).is_err());
        let mut bad = rows.clone();
        bad[0]["task"]["requirement"] = json!("changed");
        assert!(analyse(&bad, &config(3)).is_err());
        let mut forbidden = config(3);
        forbidden.allow_synthetic = false;
        assert!(analyse(&rows, &forbidden).is_err());
        let mut aliases = rows.clone();
        for row in &mut aliases {
            if row["task_id"] == "task-1" {
                row["input_hashes"] = rows[0]["input_hashes"].clone();
                row["task"]["input_hashes"] = rows[0]["input_hashes"].clone();
            }
        }
        assert!(
            analyse(&aliases, &config(3))
                .unwrap_err()
                .to_string()
                .contains("multiple task-instance")
        );
    }
    #[test]
    fn zero_baseline_and_one_cluster_do_not_invent_ratios_or_uncertainty() {
        let mut rows = fixtures::records(3);
        for row in &mut rows {
            if row["arm"] == "A" {
                for key in [
                    "input_tokens",
                    "output_tokens",
                    "total_tokens",
                    "cached_input_tokens",
                    "schema_input_tokens",
                    "feedback_input_tokens",
                    "context_peak",
                ] {
                    row["metrics"][key] = json!(0);
                }
            }
        }
        let report = analyse(&rows, &config(3)).unwrap();
        assert_eq!(report["comparisons"][0]["zero_reference_token_pairs"], 6);
        assert!(report["comparisons"][0]["paired_relative_savings"]["estimate"].is_null());
        let mut one = config(3);
        one.expected_tasks = 1;
        let rows: Vec<_> = fixtures::records(3)
            .into_iter()
            .filter(|r| r["task_id"] == "task-0")
            .collect();
        let report = analyse(&rows, &one).unwrap();
        assert!(report["comparisons"][0]["delta_tokens"]["ci95"].is_null());
    }
    #[test]
    fn higher_treatment_cost_is_a_valid_negative_result() {
        let mut rows = fixtures::records(3);
        for row in &mut rows {
            if row["arm"] == "D" {
                let total = if row["task_id"] == "task-0" { 200 } else { 600 };
                row["metrics"]["total_tokens"] = json!(total);
                row["metrics"]["input_tokens"] = json!(total - 20);
                row["metrics"]["context_peak"] = json!(total);
            }
        }
        let report = analyse(&rows, &config(3)).unwrap();
        assert_eq!(
            report["comparisons"][0]["paired_relative_savings"]["estimate"],
            -1.0
        );
        assert_eq!(
            report["comparisons"][0]["token_reduction_supported_exploratory"],
            false
        );
        assert!(
            report["negative_findings_or_limits"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v
                    .as_str()
                    .unwrap()
                    .starts_with("D versus A: token consumption increased"))
        );
    }
    #[test]
    fn verifier_error_rates_use_oracle_conditioned_denominators() {
        let mut rows = fixtures::records(3);
        rows[8]["metrics"]["last_verifier_ok"] = json!(true);
        rows[8]["verifier_false_positive"] = json!(true);
        rows[9]["metrics"]["last_verifier_ok"] = json!(false);
        rows[9]["verifier_false_negative"] = json!(true);
        let report = analyse(&rows, &config(3)).unwrap();
        let rates = &report["verifier_disagreement"];
        assert_eq!(rates["false_positive"], 1);
        assert_eq!(rates["false_negative"], 1);
        assert_eq!(rates["oracle_negative_runs"], 2);
        assert_eq!(rates["oracle_positive_runs"], 10);
        assert_eq!(rates["false_positive_rate"], 0.5);
        assert_eq!(rates["false_negative_rate"], 0.1);
    }
    #[test]
    fn perfect_ties_have_finite_unseen_task_risk_not_zero_uncertainty() {
        for count in [48, 60] {
            let prototypes = fixtures::records(1);
            let mut rows = Vec::new();
            for index in 0..count {
                for prototype in &prototypes[..4] {
                    let mut row = prototype.clone();
                    row["run_id"] = json!(format!(
                        "risk-fixture-{index}-{}",
                        row["arm"].as_str().unwrap()
                    ));
                    row["task_id"] = json!(format!("risk-{index}"));
                    row["task"]["id"] = row["task_id"].clone();
                    row["input_hashes"] = json!({"input.csv":format!("{index:064x}")});
                    row["task"]["input_hashes"] = row["input_hashes"].clone();
                    rows.push(row);
                }
            }
            let mut cfg = config(1);
            cfg.expected_tasks = count;
            let report = analyse(&rows, &cfg).unwrap();
            assert_eq!(
                report["comparisons"][0]["delta_success"]["ci95"],
                json!([0.0, 0.0])
            );
            assert_eq!(
                report["comparisons"][0]["non_inferiority_supported_exploratory"],
                count == 60
            );
            assert_eq!(report["hypotheses"]["H1"], "not_evaluated_synthetic");
        }
    }
}
