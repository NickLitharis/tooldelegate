//! Exploratory log-token-ratio regression with task-instance bootstrap uncertainty.
use crate::analysis::{Config, Random, quantile};
use nalgebra::{DMatrix, DVector};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

struct Point {
    family: String,
    complex: bool,
    failure: bool,
    x: f64,
    y: f64,
}

fn features(point: &Point, x: f64, families: &[String], complex: bool, failure: bool) -> Vec<f64> {
    let mut row = vec![1.0, x];
    for family in &families[1..] {
        let present = f64::from(u8::from(point.family == *family));
        row.extend([present, present * x]);
    }
    if complex {
        let present = f64::from(u8::from(point.complex));
        row.extend([present, present * x]);
    }
    if failure {
        let present = f64::from(u8::from(point.failure));
        row.extend([present, present * x]);
    }
    row
}

fn solve(rows: &[Vec<f64>], ys: &[f64]) -> Option<Vec<f64>> {
    let columns = rows.first()?.len();
    if rows.len() < columns + 2 {
        return None;
    }
    let matrix = DMatrix::from_row_slice(
        rows.len(),
        columns,
        &rows.iter().flatten().copied().collect::<Vec<_>>(),
    );
    let svd = matrix.svd(true, true);
    let epsilon = svd.singular_values[0] * 1e-10;
    if svd.rank(epsilon) != columns {
        return None;
    }
    let result = svd.solve(&DVector::from_column_slice(ys), epsilon).ok()?;
    result
        .iter()
        .all(|v| v.is_finite())
        .then(|| result.iter().copied().collect())
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn interval(values: &mut [f64], minimum: usize) -> Value {
    if values.len() < minimum {
        return Value::Null;
    }
    values.sort_by(f64::total_cmp);
    json!([quantile(values, 0.025), quantile(values, 0.975)])
}

pub fn fit(points: &[Value], config: &Config) -> Value {
    let data: Vec<_> = points
        .iter()
        .filter_map(|p| {
            let a = p["reference_mean_tokens"].as_f64()?;
            let d = p["treatment_mean_tokens"].as_f64()?;
            let bytes = p["input_bytes"].as_f64()?;
            if a <= 0.0 || d <= 0.0 || bytes <= 0.0 {
                return None;
            }
            Some(Point {
                family: p["family"].as_str()?.into(),
                complex: p["complexity"] == "multi-step",
                failure: p["expected_outcome"] != "success",
                x: bytes.log10(),
                y: (d / a).ln(),
            })
        })
        .collect();
    if data.len() < 3 {
        return json!({"status":"insufficient_task_instances","eligible_points":data.len(),"excluded_nonpositive_or_missing":points.len()-data.len()});
    }
    let families: Vec<_> = data
        .iter()
        .map(|p| p.family.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let complex = data.iter().any(|p| p.complex) && data.iter().any(|p| !p.complex);
    let failure = data.iter().any(|p| p.failure) && data.iter().any(|p| !p.failure);
    let center = data.iter().map(|p| p.x).sum::<f64>() / data.len() as f64;
    let rows: Vec<_> = data
        .iter()
        .map(|p| features(p, p.x - center, &families, complex, failure))
        .collect();
    let ys: Vec<_> = data.iter().map(|p| p.y).collect();
    let Some(coefficients) = solve(&rows, &ys) else {
        return json!({"status":"rank_deficient_or_insufficient_degrees_of_freedom","eligible_points":data.len(),"features":rows[0].len()});
    };
    let mut names = vec![
        "intercept".to_owned(),
        "log10_input_bytes_centered".to_owned(),
    ];
    for family in &families[1..] {
        names.extend([
            format!("family:{family}"),
            format!("family:{family}:log10_bytes"),
        ]);
    }
    if complex {
        names.extend(["multi_step".into(), "multi_step:log10_bytes".into()]);
    }
    if failure {
        names.extend([
            "expected_failure".into(),
            "expected_failure:log10_bytes".into(),
        ]);
    }
    let mut random = Random::new(config.bootstrap_seed);
    let mut bootstraps = Vec::new();
    for _ in 0..config.bootstrap_samples {
        let indices: Vec<_> = (0..data.len()).map(|_| random.index(data.len())).collect();
        if let Some(result) = solve(
            &indices.iter().map(|i| rows[*i].clone()).collect::<Vec<_>>(),
            &indices.iter().map(|i| ys[*i]).collect::<Vec<_>>(),
        ) {
            bootstraps.push(result);
        }
    }
    let required = (config.bootstrap_samples * 4).div_ceil(5);
    let mean_y = ys.iter().sum::<f64>() / ys.len() as f64;
    let residual_sum = rows
        .iter()
        .zip(&ys)
        .map(|(row, y)| (dot(row, &coefficients) - y).powi(2))
        .sum::<f64>();
    let total_sum = ys.iter().map(|y| (y - mean_y).powi(2)).sum::<f64>();
    let coefficient_rows: Vec<_>=names.iter().enumerate().map(|(index,name)|json!({"name":name,"estimate":coefficients[index],"ci95":interval(&mut bootstraps.iter().map(|b|b[index]).collect::<Vec<_>>(),required)})).collect();
    let mut strata: BTreeMap<(&str, bool, bool), Vec<&Point>> = BTreeMap::new();
    for point in &data {
        strata
            .entry((&point.family, point.complex, point.failure))
            .or_default()
            .push(point);
    }
    let groups: Vec<_>=strata.into_iter().map(|((family,multi,expected_failure),points)| {
        let prototype=points[0];
        let zero=features(prototype,0.0,&families,complex,failure);
        let one=features(prototype,1.0,&families,complex,failure);
        let slope_row: Vec<_>=one.iter().zip(&zero).map(|(a,b)|a-b).collect();
        let intercept=dot(&zero,&coefficients);let slope=dot(&slope_row,&coefficients);
        let min=points.iter().map(|p|p.x).reduce(f64::min).unwrap();let max=points.iter().map(|p|p.x).reduce(f64::max).unwrap();
        let distinct=points.iter().map(|p|p.x.to_bits()).collect::<BTreeSet<_>>().len();
        let root=if slope.abs()>1e-12 {Some(center-intercept/slope)} else {None};
        let in_range=root.is_some_and(|r|r.is_finite()&&r>=min&&r<=max)&&distinct>=3;
        let mut slopes: Vec<_>=bootstraps.iter().map(|b|dot(&slope_row,b)).collect();
        let slope_interval=interval(&mut slopes,required);
        let mut roots: Vec<_>=bootstraps.iter().filter_map(|b| {
            let s=dot(&slope_row,b);if s.abs()<1e-12{return None;}
            let root=center-dot(&zero,b)/s;
            root.is_finite().then_some(root)
        }).collect();
        let logs=interval(&mut roots,required);
        let stable_slope=slope_interval[0].as_f64().zip(slope_interval[1].as_f64()).is_some_and(|(a,b)|(a<0.0&&b<0.0)||(a>0.0&&b>0.0));
        let root_interval=if in_range&&stable_slope&&logs[0].as_f64().is_some_and(|x|x>=min)&&logs[1].as_f64().is_some_and(|x|x<=max) {json!([10f64.powf(logs[0].as_f64().unwrap()),10f64.powf(logs[1].as_f64().unwrap())])}else{Value::Null};
        json!({"family":family,"complexity":if multi {"multi-step"}else{"simple"},"expected_failure":expected_failure,"task_instances":points.len(),"observed_input_bytes":[10f64.powf(min),10f64.powf(max)],"slope_log_ratio_per_log10_byte":slope,"slope_ci95":slope_interval,"decreasing_token_ratio_supported_exploratory":data.len()>=config.minimum_clusters&&slope_interval[1].as_f64().is_some_and(|v|v< -1e-10),"crossover_input_bytes":if in_range {root.map(|r|10f64.powf(r))}else{None},"crossover_ci95":root_interval,"bootstrap_finite_roots":roots.len(),"crossover_status":if !in_range {"not_supported_in_measured_range"}else if root_interval.is_null() {"exploratory_point_only_uncertainty_unresolved"}else{"exploratory_in_range_estimate"}})
    }).collect();
    json!({"status":"exploratory_fit","formula":"ln(mean tokens D / mean tokens A) ~ centered log10(input bytes) * (family + complexity + expected-failure)","size_proxy":"input_bytes_not_final_artifact_bytes","baseline_family":families[0],"log10_center":center,"eligible_points":data.len(),"excluded_nonpositive_or_missing":points.len()-data.len(),"coefficients":coefficient_rows,"r_squared":if total_sum>1e-12 {Some(1.0-residual_sum/total_sum)}else{None},"residual_degrees_of_freedom":data.len()-coefficients.len(),"residual_standard_error":(residual_sum/(data.len()-coefficients.len()) as f64).sqrt(),"bootstrap_valid":bootstraps.len(),"bootstrap_requested":config.bootstrap_samples,"strata":groups,"warning":"No crossover is extrapolated outside its measured stratum. Root intervals are not conditioned on being in range: they are suppressed if unstable or extending outside it. At least 80% identifiable fits are required. Slope support excludes numerical effects below 1e-10."})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        serde_json::from_value(crate::analysis::fixtures::config(3)).unwrap()
    }
    #[test]
    fn known_crossover_is_recovered_but_never_extrapolated() {
        let points: Vec<_>=(0..12).map(|i|{
            let x=i as f64*0.4;
            json!({"task_id":format!("toy-{i}"),"family":"sorting","complexity":"simple","expected_outcome":"success","input_bytes":10f64.powf(x),"reference_mean_tokens":1000.0,"treatment_mean_tokens":1000.0*10f64.powf(1.0-0.5*x)})
        }).collect();
        let fitted = fit(&points, &config());
        assert_eq!(fitted["status"], "exploratory_fit");
        assert!(
            (fitted["strata"][0]["crossover_input_bytes"]
                .as_f64()
                .unwrap()
                - 100.0)
                .abs()
                < 1e-7
        );
        let mut outside = points.clone();
        for point in &mut outside {
            point["treatment_mean_tokens"] =
                json!(point["treatment_mean_tokens"].as_f64().unwrap() * 1e-10);
        }
        let fitted = fit(&outside, &config());
        assert!(fitted["strata"][0]["crossover_input_bytes"].is_null());
        assert_eq!(
            fitted["strata"][0]["crossover_status"],
            "not_supported_in_measured_range"
        );
        let mut singular = points;
        for point in &mut singular {
            point["input_bytes"] = json!(100);
        }
        assert_eq!(
            fit(&singular, &config())["status"],
            "rank_deficient_or_insufficient_degrees_of_freedom"
        );
    }
    #[test]
    fn family_complexity_and_outcome_interactions_are_identifiable() {
        let mut points = Vec::new();
        for (f, family) in ["aggregation", "filesystem", "sorting", "text"]
            .iter()
            .enumerate()
        {
            for multi in [false, true] {
                for failure in [false, true] {
                    for x in [1.0, 2.0, 3.0] {
                        let f = f as f64;
                        let m = f64::from(u8::from(multi));
                        let e = f64::from(u8::from(failure));
                        let y = 0.4 - 0.2 * x + f * 0.1 + f * 0.02 * x + m * 0.2 - m * 0.05 * x
                            + e * 0.3
                            + e * 0.01 * x;
                        points.push(json!({"family":family,"complexity":if multi {"multi-step"}else{"simple"},"expected_outcome":if failure {"INVALID"}else{"success"},"input_bytes":10f64.powf(x),"reference_mean_tokens":1000.0,"treatment_mean_tokens":1000.0*y.exp()}));
                    }
                }
            }
        }
        let fitted = fit(&points, &config());
        assert_eq!(fitted["status"], "exploratory_fit");
        assert_eq!(fitted["coefficients"].as_array().unwrap().len(), 12);
        assert_eq!(fitted["strata"].as_array().unwrap().len(), 16);
        for group in fitted["strata"].as_array().unwrap() {
            let f = ["aggregation", "filesystem", "sorting", "text"]
                .iter()
                .position(|family| group["family"] == *family)
                .unwrap() as f64;
            let m = if group["complexity"] == "multi-step" {
                1.0
            } else {
                0.0
            };
            let e = if group["expected_failure"] == true {
                1.0
            } else {
                0.0
            };
            assert!(
                (group["slope_log_ratio_per_log10_byte"].as_f64().unwrap()
                    - (-0.2 + f * 0.02 - m * 0.05 + e * 0.01))
                    .abs()
                    < 1e-9
            );
        }
    }
}
