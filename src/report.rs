use crate::analysis::{self, ARMS, Config, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, fs, path::Path};

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn markdown_label(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\|`*_{}[]()#!~".contains(c) {
            out.push('\\');
        }
        out.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
    escape(&out)
}
fn display(value: &Value) -> String {
    value
        .as_f64()
        .map(format_number)
        .unwrap_or_else(|| "n/a".into())
}
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else if value.abs() > 0.0 && value.abs() < 0.001 {
        format!("{value:.3e}")
    } else {
        format!("{value:.4}")
    }
}
fn json_block(value: &Value) -> String {
    format!(
        "```json\n{}\n```\n",
        serde_json::to_string_pretty(value).unwrap()
    )
}

fn svg_start(width: usize, height: usize, title: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" role=\"img\" viewBox=\"0 0 {width} {height}\"><title>{}</title><rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><style>text{{font-family:system-ui,sans-serif;fill:#17324d;font-size:12px}}.title{{font-weight:650;font-size:16px}}</style>",
        escape(title)
    )
}

fn bars(
    title: &str,
    unit: &str,
    values: &[(String, Option<f64>)],
    fixed_max: Option<f64>,
) -> String {
    let mut svg = svg_start(640, 320, title);
    let max =
        fixed_max.unwrap_or_else(|| values.iter().filter_map(|(_, v)| *v).fold(1.0, f64::max));
    writeln!(
        svg,
        "<text class=\"title\" x=\"16\" y=\"25\">{}</text><text x=\"16\" y=\"46\">{}</text>",
        escape(title),
        escape(unit)
    )
    .unwrap();
    for (index, (label, value)) in values.iter().enumerate() {
        let x = 92.0 + index as f64 * 135.0;
        if let Some(value) = value {
            let height = 210.0 * (value / max);
            writeln!(svg,"<rect x=\"{x}\" y=\"{}\" width=\"70\" height=\"{height}\" fill=\"#237891\"/><text x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",270.0-height,x+35.0,260.0-height,format_number(*value)).unwrap();
        } else {
            writeln!(
                svg,
                "<text x=\"{}\" y=\"250\" text-anchor=\"middle\">unknown</text>",
                x + 35.0
            )
            .unwrap();
        }
        writeln!(
            svg,
            "<text x=\"{}\" y=\"293\" text-anchor=\"middle\">{}</text>",
            x + 35.0,
            escape(label)
        )
        .unwrap();
    }
    svg.push_str("<path d=\"M65 60V270H620\" fill=\"none\" stroke=\"#9aaeb8\"/></svg>");
    svg
}

fn scatter(title: &str, x_label: &str, y_label: &str, points: &[(f64, f64, String)]) -> String {
    let mut svg = svg_start(640, 340, title);
    writeln!(svg,"<text class=\"title\" x=\"16\" y=\"25\">{}</text><text x=\"16\" y=\"47\">{}</text><text x=\"320\" y=\"326\" text-anchor=\"middle\">{}</text>",escape(title),escape(y_label),escape(x_label)).unwrap();
    if points.is_empty() {
        svg.push_str("<text x=\"80\" y=\"160\">No fully measured, defined points</text></svg>");
        return svg;
    }
    let mut xmin = points.iter().map(|p| p.0).reduce(f64::min).unwrap();
    let mut xmax = points.iter().map(|p| p.0).reduce(f64::max).unwrap();
    let mut ymin = points.iter().map(|p| p.1).fold(0.0, f64::min);
    let mut ymax = points.iter().map(|p| p.1).fold(0.0, f64::max);
    if xmin == xmax {
        xmin -= 0.5;
        xmax += 0.5;
    }
    if ymin == ymax {
        ymin -= 0.5;
        ymax += 0.5;
    }
    let xy = |x: f64, y: f64| {
        (
            80.0 + (x - xmin) / (xmax - xmin) * 520.0,
            275.0 - (y - ymin) / (ymax - ymin) * 210.0,
        )
    };
    let (_, zero) = xy(xmin, 0.0);
    writeln!(svg,"<path d=\"M80 60V275H610M80 {zero}H610\" fill=\"none\" stroke=\"#9aaeb8\"/><text x=\"80\" y=\"297\">{}</text><text x=\"600\" y=\"297\" text-anchor=\"end\">{}</text><text x=\"10\" y=\"75\">{}</text><text x=\"10\" y=\"275\">{}</text>",format_number(xmin),format_number(xmax),format_number(ymax),format_number(ymin)).unwrap();
    for (x, y, label) in points {
        let (px, py) = xy(*x, *y);
        let color = if *y < 0.0 { "#b84d39" } else { "#237891" };
        writeln!(svg,"<circle cx=\"{px}\" cy=\"{py}\" r=\"5\" fill=\"{color}\" opacity=\"0.8\"><title>{}: {}, {}</title></circle>",escape(label),format_number(*x),format_number(*y)).unwrap();
    }
    svg.push_str("</svg>");
    svg
}

fn charts(summary: &Value) -> Vec<(&'static str, String)> {
    let values = |key: &str| {
        ARMS.iter()
            .map(|arm| (arm.to_string(), summary["arms"][arm][key].as_f64()))
            .collect::<Vec<_>>()
    };
    let metric = |key: &str, field: &str| {
        ARMS.iter()
            .map(|arm| {
                (
                    arm.to_string(),
                    summary["arms"][arm]["metrics"][key][field].as_f64(),
                )
            })
            .collect::<Vec<_>>()
    };
    let points = summary["comparisons"][0]["size_points"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|point| {
            Some((
                point["input_bytes"].as_f64()?.log10(),
                point["relative_savings"].as_f64()? * 100.0,
                format!(
                    "{} / {}",
                    point["task_id"].as_str()?,
                    point["family"].as_str()?
                ),
            ))
        })
        .collect::<Vec<_>>();
    let efficiency = ARMS
        .iter()
        .filter_map(|arm| {
            Some((
                summary["arms"][arm]["success_rate"].as_f64()? * 100.0,
                summary["arms"][arm]["token_efficiency"].as_f64()?,
                arm.to_string(),
            ))
        })
        .collect::<Vec<_>>();
    let mut overhead = svg_start(
        640,
        960,
        "Verification overhead: distinct units, not interchangeable costs",
    );
    for (index, (key, title, unit)) in [
        (
            "verification_input_tokens",
            "Verification feedback in context",
            "Mean attributed input tokens / run",
        ),
        (
            "verifier_cpu_seconds",
            "Verification CPU",
            "Mean measured CPU seconds / run",
        ),
        (
            "verifier_elapsed_seconds",
            "Verification elapsed time",
            "Mean elapsed seconds / run",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let values: Vec<_> = ARMS
            .iter()
            .map(|arm| {
                let total = summary["arms"][arm]["metrics"][key]["sum"].as_f64();
                let n = summary["arms"][arm]["runs"].as_f64().unwrap_or(0.0);
                (arm.to_string(), total.filter(|_| n > 0.0).map(|v| v / n))
            })
            .collect();
        let nested = bars(title, unit, &values, None).replacen(
            "<svg ",
            &format!(
                "<svg x=\"0\" y=\"{}\" width=\"640\" height=\"320\" ",
                index * 320
            ),
            1,
        );
        overhead.push_str(&nested);
    }
    overhead.push_str("</svg>");
    vec![
        (
            "tokens.svg",
            bars(
                "Model tokens by arm",
                "Mean total model tokens / run",
                &values("mean_tokens"),
                None,
            ),
        ),
        (
            "success.svg",
            bars(
                "Task success by arm",
                "Completed hidden-oracle passes (%)",
                &values("success_rate")
                    .into_iter()
                    .map(|(arm, v)| (arm, v.map(|n| n * 100.0)))
                    .collect::<Vec<_>>(),
                Some(100.0),
            ),
        ),
        (
            "savings-size.svg",
            scatter(
                "D versus A: token savings and input size",
                "log10(input bytes), an artifact-size proxy",
                "Task-level relative savings (%)",
                &points,
            ),
        ),
        ("verification-tax.svg", overhead),
        (
            "context.svg",
            bars(
                "Context peak by arm",
                "Mean per-run peak input + output tokens",
                &metric("context_peak", "mean"),
                None,
            ),
        ),
        (
            "efficiency-correctness.svg",
            scatter(
                "Token efficiency versus correctness",
                "Completed hidden-oracle success (%)",
                "Total tokens / successful runs",
                &efficiency,
            ),
        ),
        (
            "latency.svg",
            bars(
                "End-to-end latency by arm",
                "Available mean elapsed seconds / run; see missing counts",
                &metric("end_to_end_seconds", "mean"),
                None,
            ),
        ),
    ]
}

fn markdown(summary: &Value) -> String {
    let banner = if summary["synthetic"] == true {
        "SYNTHETIC FIXTURE REPORT — NOT RESEARCH MEASUREMENTS"
    } else {
        "EXPLORATORY OBSERVATIONAL REPORT — NOT CONFIRMATORY"
    };
    let mut text = format!("# Verified CLI Delegation Benchmark\n\n**{banner}**\n\n");
    text.push_str("## 1. Research question and hypotheses\n\nWhen does CLI delegation with compact verification feedback reduce model tokens without reducing correctness? H1 tests efficiency and correctness separately. H2 tests size dependence and in-range crossover. H3 isolates raw verification and compression effects. H4 examines invocation-complexity limitations independently. No treatment advantage is assumed.\n\n");
    text.push_str("## 2. Experimental methodology\n\nAll supplied runs, including failures and incomplete runs, remain in arm summaries. Paired effects weight each task instance equally, retaining paired repetitions within task clusters. Protocol, cohort, and confirmatory status are defined by preregistration; this report does not infer them from the presence of data.\n\n");
    text.push_str(&json_block(&json!({"model":summary["model"],"decoding":summary["decoding"],"analysis_config":summary["config"],"matrix_complete":summary["matrix_complete"],"records":summary["records"],"expected_records":summary["expected_records"],"missing_cells":summary["missing_cells"],"unidentified_missing_tasks":summary["unidentified_missing_tasks"],"protocol_violation_runs":summary["protocol_violation_runs"]})));
    text.push_str("\n## 3. Dataset description\n\nInputs and requirements are validated against run manifests. Input byte count is an artifact-size proxy, not final-output size. Expected error tasks are retained; the boundary model controls for expected failure when identifiable. Detailed family, size, complexity and outcome strata are in summary.json.\n\n| Family / arm | Runs | Successes | Exact model tokens |\n|---|---:|---:|---:|\n");
    if let Some(groups) = summary["strata"]["family"].as_object() {
        for (name, group) in groups {
            writeln!(
                text,
                "| {} | {} | {} | {} |",
                markdown_label(name),
                display(&group["runs"]),
                display(&group["successes"]),
                display(&group["total_tokens"])
            )
            .unwrap();
        }
    }
    text.push_str("\n## 4. Implementation architecture\n\nRust dataset generators, immutable originals, fresh Linux bubblewrap workspaces, single-process tool execution, independent feedback verification, bounded certificates, model adapters, host-only JSONL telemetry and a separate hidden oracle. Provider and protocol validation status is documented in the applicable protocol.\n\n## 5. Token consumption by arm\n\nUnknown usage is not zero; known totals are lower bounds whenever usage is incomplete.\n\n| Arm | Runs | Exact tokens | Known tokens | Unknown-usage runs |\n|---|---:|---:|---:|---:|\n");
    for arm in ARMS {
        let a = &summary["arms"][arm];
        writeln!(
            text,
            "| {arm} | {} | {} | {} | {} |",
            display(&a["runs"]),
            display(&a["total_tokens"]),
            display(&a["known_total_tokens"]),
            display(&a["usage_incomplete_runs"])
        )
        .unwrap();
    }
    text.push_str("\n![Tokens by arm](tokens.svg)\n\n## 6. Success rates by arm\n\nSuccess requires completion and a hidden-oracle pass. Unknown oracle outcomes and resource-limited incomplete runs do not count as successes. Raw oracle pass and completeness remain distinct in raw records.\n\n| Arm | Successes / runs | Success rate | Incomplete runs | Unknown oracle |\n|---|---:|---:|---:|---:|\n");
    for arm in ARMS {
        let a = &summary["arms"][arm];
        writeln!(
            text,
            "| {arm} | {} / {} | {} | {} | {} |",
            display(&a["successes"]),
            display(&a["runs"]),
            display(&a["success_rate"]),
            display(&a["incomplete_runs"]),
            display(&a["oracle_unknown_runs"])
        )
        .unwrap();
    }
    text.push_str("\n![Success by arm](success.svg)\n\n## 7. Token efficiency and verification overhead\n\nEfficiency is aggregate tokens across all runs divided by successful runs, never an average of per-run ratios. Zero successes or missing usage makes it undefined. Verification input tokens, CPU, wall time, memory and observed retries are separate costs; CPU is never converted to LLM tokens. See arm metrics in summary.json for measured/missing counts and attributed-input coverage.\n\n| Arm | Tokens / success | Verifier input tokens | Verifier CPU seconds | Verifier elapsed seconds | Estimated monetary cost |\n|---|---:|---:|---:|---:|---:|\n");
    for arm in ARMS {
        let a = &summary["arms"][arm];
        let m = &a["metrics"];
        writeln!(
            text,
            "| {arm} | {} | {} | {} | {} | {} |",
            display(&a["token_efficiency"]),
            display(&m["verification_input_tokens"]["sum"]),
            display(&m["verifier_cpu_seconds"]["sum"]),
            display(&m["verifier_elapsed_seconds"]["sum"]),
            display(&m["monetary_cost"]["sum"])
        )
        .unwrap();
    }
    text.push_str("\nRecorded pricing assumptions (null means unavailable):\n\n");
    text.push_str(&json_block(&summary["run_config"]["pricing"]));
    text.push_str("\n![Verification overhead](verification-tax.svg)\n\n![Efficiency versus correctness](efficiency-correctness.svg)\n\n## 8. Token savings against artifact size\n\nPaired relative savings are 1 − treatment/reference; positive values mean fewer treatment tokens. The chart uses ratios of task-level mean tokens, whereas the paired relative-effect estimate averages paired ratios within each task. Zero reference tokens produce an undefined ratio. Negative savings are retained. The exploratory SVD fit uses log token ratio with size × family, complexity and expected-failure interactions when identifiable. No crossover is extrapolated beyond its measured stratum.\n\n![Savings against input size](savings-size.svg)\n\n");
    text.push_str(&json_block(&summary["boundary"]));
    text.push_str("\n## 9. Context-usage comparison\n\nPeak context is the largest reported input-plus-output call for each run. Complete artifacts are not injected into later model calls; arm A retains only chunk metadata. Input reads and raw feedback remain part of actual context. Missing usage prevents exact context-peak conclusions. Tool I/O volume, calls, attempts and retries are reported separately in summary.json.\n\n![Context peaks](context.svg)\n\n## 10. Failure taxonomy and representative traces\n\nTerminations count runs. Taxonomy counts encountered events, not mutually exclusive final failures; retries may recover. Verifier-reported semantic/format errors are marked as such, not treated as oracle truth. Unknown classifications remain unknown, including unclassified execution failures. Trace collection enriches summary-only data; absent trace coverage is not evidence that no errors occurred.\n\n");
    text.push_str(&json_block(&json!({"terminations":summary["terminations"],"trace_classified_runs":summary["trace_classified_runs"],"failure_taxonomy":summary["failure_taxonomy"],"failed_run_examples":summary["failure_examples"],"verifier_disagreement":summary["verifier_disagreement"]})));
    text.push_str("\n## 11. Statistical uncertainty\n\nAll intervals are exploratory. Percentile bootstrap resamples task instances with replacement and preserves all matched repetitions in each selected cluster; repeated runs are not independent samples. Paired differences are treatment minus reference. Non-inferiority uses a one-sided 95% lower bound and is separate from a two-sided 95% token interval. Too few clusters, missing matrix cells, protocol violations or unknown usage prevent the corresponding support claim. Secondary comparisons are not multiplicity-adjusted and are not confirmatory.\n\nA degenerate bootstrap is not proof of zero failure risk. For all-nonnegative or constant observed task success differences, the success lower bound is also capped by m·0.05^(1/n) − (1−0.05^(1/n)), where m is the minimum observed cluster difference and n is the task count. This conservative unseen-task risk guard shares the task-sampling assumptions; it is not a guarantee outside this dataset. In particular, 48 perfect ties do not establish a five-point margin under this guard.\n\n| Treatment − reference | Task clusters | Mean token difference | Token CI95 | Mean success difference | NI lower bound |\n|---|---:|---:|---|---:|---:|\n");
    for c in summary["comparisons"].as_array().unwrap() {
        writeln!(
            text,
            "| {} − {} | {} | {} | {} | {} | {} |",
            c["treatment"].as_str().unwrap(),
            c["reference"].as_str().unwrap(),
            display(&c["task_clusters"]),
            display(&c["delta_tokens"]["estimate"]),
            c["delta_tokens"]["ci95"],
            display(&c["delta_success"]["estimate"]),
            display(&c["non_inferiority_lower_bound"])
        )
        .unwrap();
    }
    text.push_str("\n## 12. Threats to validity\n\n");
    for item in summary["limitations"].as_array().unwrap() {
        writeln!(text, "- {}", item.as_str().unwrap()).unwrap();
    }
    text.push_str("\n## 13. Boundary conditions and negative findings\n\nThese observations do not discard negative results. Point estimates are not declarations of statistical significance. Synthetic observations validate report logic only.\n\n");
    for finding in summary["negative_findings_or_limits"].as_array().unwrap() {
        writeln!(text, "- {}", finding.as_str().unwrap()).unwrap();
    }
    text.push_str("\n![Latency](latency.svg)\n\n## 14. Conclusions and reproducibility\n\nToken reduction alone is insufficient to establish an overall improvement. Hypotheses are evaluated independently; the report can show no advantage or no established non-inferiority. Synthetic fixtures support none of the scientific hypotheses.\n\n");
    text.push_str(&json_block(&summary["hypotheses"]));
    text.push_str("\nRegenerate from the repository root, preserving Cargo.lock:\n\n```sh\ncargo run --locked -- analyze REPORT_DIRECTORY/records.jsonl NEW_OUTPUT REPORT_DIRECTORY/config.json\n```\n\nThe report bundles source records, normalized analysis settings, summary.json and seven SVGs. Source hashes are in summary.json. Keep original run directories for full prompts, provider payloads, tool output and environment evidence. Consult the preregistration for cohort and confirmatory status; the generated report does not infer these from its record count.\n");
    text
}

pub fn generate(records_path: &Path, destination: &Path, config_path: &Path) -> Result<()> {
    let config: Config = serde_json::from_slice(&fs::read(config_path)?)?;
    let records = analysis::load(records_path)?;
    let mut summary = analysis::analyse(&records, &config)?;
    summary["source_records_sha256"] =
        json!(format!("{:x}", Sha256::digest(fs::read(records_path)?)));
    summary["analysis_config_sha256"] = json!(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&config)?)
    ));
    summary["analysis_cargo_lock_sha256"] = json!(format!(
        "{:x}",
        Sha256::digest(include_bytes!("../Cargo.lock"))
    ));
    fs::create_dir(destination)?;
    fs::copy(records_path, destination.join("records.jsonl"))?;
    fs::write(
        destination.join("config.json"),
        serde_json::to_vec_pretty(&config)?,
    )?;
    fs::write(
        destination.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    let markdown = markdown(&summary);
    fs::write(destination.join("report.md"), &markdown)?;
    let banner = if summary["synthetic"] == true {
        "SYNTHETIC FIXTURES — NOT RESEARCH RESULTS"
    } else {
        "EXPLORATORY — NOT CONFIRMATORY"
    };
    let mut html = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Verified CLI benchmark analysis</title><style>body{{max-width:1100px;margin:3rem auto;padding:0 1rem;background:#f4f7f8;color:#17324d;font:16px/1.6 system-ui}}h1{{line-height:1.2}}.notice{{padding:1rem;border-left:5px solid #b84d39;background:#fff}}.charts{{display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:1rem}}figure{{margin:0;background:white;padding:1rem}}img{{width:100%;height:auto}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#fff;padding:1rem;font-size:14px}}a{{color:#17647b}}</style><h1>Verified CLI Delegation Benchmark</h1><p class=\"notice\"><strong>{banner}</strong>. Missing values are unknown, not zero. No confirmatory conclusions are made.</p><p><a href=\"report.md\">Full Markdown report</a> · <a href=\"summary.json\">Machine-readable analysis</a></p><div class=\"charts\">"
    );
    for (name, svg) in charts(&summary) {
        let caption = match name {
            "tokens.svg" => "Mean model tokens per run",
            "success.svg" => "Completed hidden-oracle success rate",
            "savings-size.svg" => "Task-level D/A savings versus log input size",
            "verification-tax.svg" => {
                "Verification input tokens, CPU and elapsed time in separate units"
            }
            "context.svg" => "Mean per-run peak context",
            "efficiency-correctness.svg" => "Aggregate tokens per success versus correctness",
            _ => "Available mean end-to-end latency; missing measurements remain reported",
        };
        fs::write(destination.join(name), svg)?;
        write!(
            html,
            "<figure><img src=\"{name}\" alt=\"{}\"><figcaption>{}</figcaption></figure>",
            escape(caption),
            escape(caption)
        )
        .unwrap();
    }
    html.push_str("</div><h2>Complete report</h2><pre>");
    html.push_str(&escape(&markdown));
    html.push_str("</pre></html>");
    fs::write(destination.join("report.html"), html)?;
    Ok(())
}
