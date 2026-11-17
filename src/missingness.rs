//! Descriptive diagnostics for incomplete provider token usage; never imputes missing calls.
use crate::analysis::{self, Result};
use serde_json::Value;
use std::{collections::BTreeMap, fmt::Write as FmtWrite, fs::OpenOptions, io::Write, path::Path};

#[derive(Default)]
struct GroupCount {
    records: usize,
    incomplete: usize,
}

#[derive(Default)]
struct ArmCount {
    records: usize,
    incomplete: usize,
    partial_tokens: u64,
    incomplete_with_nonzero_subtotal: usize,
}

#[derive(Default)]
struct CauseCount {
    runs: usize,
    partial_tokens: u64,
    runs_with_nonzero_subtotal: usize,
}

fn incomplete(record: &Value) -> bool {
    record["metrics"]["usage_complete"] != true
}

fn token_subtotal(record: &Value) -> Result<u64> {
    record["metrics"]["total_tokens"].as_u64().ok_or_else(|| {
        format!(
            "run {} has no nonnegative integer metrics.total_tokens",
            label(record, "/run_id")
        )
        .into()
    })
}

fn label(record: &Value, pointer: &str) -> String {
    record
        .pointer(pointer)
        .map(|value| match value {
            Value::String(value) => value.clone(),
            Value::Null => "unknown".to_owned(),
            value => value.to_string(),
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn markdown(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn grouped(records: &[Value], key: impl Fn(&Value) -> String) -> BTreeMap<String, GroupCount> {
    let mut groups = BTreeMap::<String, GroupCount>::new();
    for record in records {
        let group = groups.entry(key(record)).or_default();
        group.records += 1;
        group.incomplete += usize::from(incomplete(record));
    }
    groups
}

fn percentage(numerator: usize, denominator: usize) -> String {
    if denominator == 0 {
        "n/a".to_owned()
    } else {
        format!("{:.2}%", 100.0 * numerator as f64 / denominator as f64)
    }
}

fn cause(record: &Value) -> String {
    let categories = record["failure_events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| event["category"].as_str());
    let mut content_rejection = false;
    let mut transport_failure = false;
    for category in categories {
        content_rejection |= category == "provider_content_rejection";
        transport_failure |= category == "adapter_transport_failure";
    }
    if content_rejection {
        "provider_content_rejection".to_owned()
    } else if transport_failure {
        "adapter_transport_failure".to_owned()
    } else {
        "other_or_unclassified".to_owned()
    }
}

fn paired_usage_counts(
    records: &[Value],
) -> Result<Vec<(&'static str, &'static str, usize, usize)>> {
    let mut cells = BTreeMap::<(String, String), BTreeMap<String, bool>>::new();
    for record in records {
        let key = (label(record, "/task_id"), label(record, "/repetition"));
        let arm = label(record, "/arm");
        if cells
            .entry(key.clone())
            .or_default()
            .insert(arm.clone(), !incomplete(record))
            .is_some()
        {
            return Err(format!("duplicate arm {arm} for task/repetition {key:?}").into());
        }
    }

    let mut results = Vec::new();
    for (treatment, reference) in [("D", "A"), ("B", "A"), ("C", "B"), ("D", "C"), ("D", "B")] {
        let (mut matched, mut unknown) = (0, 0);
        for arms in cells.values() {
            if let (Some(treatment_complete), Some(reference_complete)) =
                (arms.get(treatment), arms.get(reference))
            {
                matched += 1;
                unknown += usize::from(!treatment_complete || !reference_complete);
            }
        }
        results.push((treatment, reference, matched, unknown));
    }
    Ok(results)
}

fn dimension_table(
    output: &mut String,
    title: &str,
    records: &[Value],
    key: impl Fn(&Value) -> String,
) -> Result<()> {
    writeln!(
        output,
        "### {title}\n\n| Group | Records | Usage incomplete | Rate |\n|---|---:|---:|---:|"
    )?;
    for (name, counts) in grouped(records, key) {
        writeln!(
            output,
            "| {} | {} | {} | {} |",
            markdown(&name),
            counts.records,
            counts.incomplete,
            percentage(counts.incomplete, counts.records)
        )?;
    }
    output.push('\n');
    Ok(())
}

fn render(records: &[Value]) -> Result<String> {
    if records.is_empty() {
        return Err("cannot analyze an empty run-record file".into());
    }
    for record in records {
        token_subtotal(record)?;
    }

    let mut incomplete_count = 0usize;
    let mut partial_sum = 0u64;
    let mut zero_subtotals = 0usize;
    let mut arms = BTreeMap::<String, ArmCount>::new();
    let mut causes = BTreeMap::<String, CauseCount>::new();
    let mut causes_by_arm = BTreeMap::<(String, String), usize>::new();
    let mut affected_tasks = BTreeMap::<String, usize>::new();
    let model = label(&records[0], "/model");

    for record in records {
        let arm = label(record, "/arm");
        let arm_count = arms.entry(arm.clone()).or_default();
        arm_count.records += 1;
        if !incomplete(record) {
            continue;
        }

        incomplete_count += 1;
        let subtotal = token_subtotal(record)?;
        partial_sum = partial_sum
            .checked_add(subtotal)
            .ok_or("known partial token subtotal overflow")?;
        arm_count.incomplete += 1;
        arm_count.partial_tokens = arm_count
            .partial_tokens
            .checked_add(subtotal)
            .ok_or("arm token subtotal overflow")?;
        if subtotal == 0 {
            zero_subtotals += 1;
        } else {
            arm_count.incomplete_with_nonzero_subtotal += 1;
        }

        let failure_cause = cause(record);
        let cause_count = causes.entry(failure_cause.clone()).or_default();
        cause_count.runs += 1;
        cause_count.partial_tokens = cause_count
            .partial_tokens
            .checked_add(subtotal)
            .ok_or("cause token subtotal overflow")?;
        cause_count.runs_with_nonzero_subtotal += usize::from(subtotal > 0);
        *causes_by_arm
            .entry((arm.clone(), failure_cause))
            .or_default() += 1;
        *affected_tasks.entry(label(record, "/task_id")).or_default() += 1;
    }
    let pair_coverage = paired_usage_counts(records)?;

    let mut output = String::new();
    writeln!(output, "# V3 token-usage missingness EDA\n")?;
    writeln!(
        output,
        "Exploratory descriptive diagnostic; not a preregistered hypothesis test. Model recorded in source: `{}`.\n",
        markdown(&model)
    )?;
    writeln!(
        output,
        "Records: **{}**  \nUsage incomplete: **{} ({})**  \nKnown partial token subtotal within incomplete records: **{}**  \nIncomplete records with a zero known subtotal: **{}**\n",
        records.len(),
        incomplete_count,
        percentage(incomplete_count, records.len()),
        partial_sum,
        zero_subtotals
    )?;
    output.push_str("A zero subtotal does not prove zero token usage; it means the record contains no known usage total for that run. `metrics.total_tokens` on incomplete records is only a known subtotal, not the complete run total.\n\n");

    output.push_str("## Missing usage by arm\n\n| Arm | Records | Usage incomplete | Rate | Known partial token subtotal | Incomplete with nonzero subtotal |\n|---|---:|---:|---:|---:|---:|\n");
    for (arm, counts) in &arms {
        writeln!(
            output,
            "| {} | {} | {} | {} | {} | {} |",
            markdown(arm),
            counts.records,
            counts.incomplete,
            percentage(counts.incomplete, counts.records),
            counts.partial_tokens,
            counts.incomplete_with_nonzero_subtotal
        )?;
    }
    output.push_str("\n## Incomplete usage by classified failure cause\n\n| Cause | Runs | Runs with nonzero known subtotal | Known partial token subtotal |\n|---|---:|---:|---:|\n");
    for (name, counts) in &causes {
        writeln!(
            output,
            "| `{}` | {} | {} | {} |",
            markdown(name),
            counts.runs,
            counts.runs_with_nonzero_subtotal,
            counts.partial_tokens
        )?;
    }
    output.push_str("\n### Failure cause by arm\n\n| Arm | Provider content rejection | Adapter transport failure | Other/unclassified |\n|---|---:|---:|---:|\n");
    for arm in arms.keys() {
        let count = |cause: &str| {
            causes_by_arm
                .get(&(arm.clone(), cause.to_owned()))
                .copied()
                .unwrap_or(0)
        };
        writeln!(
            output,
            "| {} | {} | {} | {} |",
            markdown(arm),
            count("provider_content_rejection"),
            count("adapter_transport_failure"),
            count("other_or_unclassified")
        )?;
    }
    output.push_str("\nCause labels come from run-record `failure_events`; unrecognized or absent categories are grouped as `other_or_unclassified`.\n\n## Impact on paired token comparisons\n\n| Contrast | Observed matched pairs | Pairs with incomplete usage | Pairs with exact token deltas |\n|---|---:|---:|---:|\n");
    for (treatment, reference, matched, unknown) in pair_coverage {
        writeln!(
            output,
            "| {treatment} vs {reference} | {matched} | {unknown} | {} |",
            matched - unknown
        )?;
    }
    output.push_str("\nA pair is matched by task ID and repetition; it is incomplete when either arm lacks complete usage. Only observed pairs with both arms present are counted. These counts describe which contrasts lack exact token deltas; they do not estimate missing values.\n\n## Where incomplete usage occurred\n\n");
    dimension_table(&mut output, "Task family", records, |record| {
        label(record, "/task/family")
    })?;
    dimension_table(&mut output, "Generator size parameter", records, |record| {
        label(record, "/task/size_parameter")
    })?;
    output.push_str("Size parameters are family-specific generator settings, not a common physical artifact-size scale.\n\n");
    dimension_table(&mut output, "Expected outcome", records, |record| {
        label(record, "/task/expected_outcome")
    })?;
    dimension_table(&mut output, "Complexity", records, |record| {
        label(record, "/task/complexity")
    })?;

    let affected_count = affected_tasks.len();
    let max_per_task = affected_tasks.values().copied().max().unwrap_or(0);
    writeln!(
        output,
        "Incomplete records span **{affected_count} task IDs**; the maximum observed incomplete repetitions for one task ID is **{max_per_task}**. These are repeated task-level observations, not independent task samples.\n"
    )?;
    output.push_str("## Interpretation limits\n\n- No failed-call token counts are estimated or imputed.\n- Partial subtotals do not replace missing run totals and cannot make an affected paired treatment comparison exact.\n- The main report remains authoritative for token comparisons; this diagnostic does not modify it. Known partial usage already contributes to the main report’s lower-bound token totals and must not be added again.\n- Rates are descriptive. The small outcome strata and repeated runs per task do not establish a missingness mechanism or causal arm effect.\n- Regenerate from the JSONL run records with `cargo run --locked -- usage-missingness RECORDS.jsonl OUTPUT.md`; the output path must not already exist.\n");
    Ok(output)
}

pub fn generate(records_path: &Path, output_path: &Path) -> Result<()> {
    let records = analysis::load(records_path)?;
    let report = render(&records)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)?;
    output.write_all(report.as_bytes())?;
    Ok(())
}
