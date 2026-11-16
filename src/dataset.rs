//! Dataset generation only: no reference answers are written into task directories.
use crate::filesystem_task::snapshot;
use serde_json::{Value, json};
use std::{error::Error, fs, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn write(root: &Path, name: &str, bytes: impl AsRef<[u8]>) -> Result<()> {
    let path = root.join(name);
    fs::create_dir_all(path.parent().ok_or("missing parent")?)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn sorting(root: &Path, variant: usize, size: usize, seed: u64) -> Result<Value> {
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer.write_record(["id", "value", "secondary", "payload"])?;
    let mut state = seed;
    for id in 0..size {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let value = if id % 11 == 0 {
            String::new()
        } else {
            match variant {
                0 => ((state % 101) as i64 - 50).to_string(),
                1 | 2 => ["alpha", "Alpha", "beta", "BETA", "éclair", "zebra"]
                    [(state % 6) as usize]
                    .into(),
                3 => [
                    "2024-01-01T00:00:00Z",
                    "2023-12-31T19:00:00-05:00",
                    "2024-01-02T03:00:00+03:00",
                    "2024/01/01 12:00:00",
                ][(state % 4) as usize]
                    .into(),
                _ => unreachable!(),
            }
        };
        writer.write_record([
            id.to_string(),
            value,
            (state % 7).to_string(),
            format!("row {id}, quoted \"payload\"\nseed={seed}"),
        ])?;
    }
    write(root, "input.csv", writer.into_inner()?)?;
    let policies = [
        "Sort value as signed base-10 integer ascending, empty values last; stable ties.",
        "Sort value lexicographically by Unicode scalar sequence, case-sensitive ascending, empty values last; stable ties.",
        "Sort value after ASCII-only lowercase mapping ascending, then secondary as integer descending. Empty values last, with secondary descending also applied among empty values; preserve input order when both keys compare equal. Non-ASCII characters are unchanged.",
        "Sort value by UTC instant ascending. Accept RFC3339 timestamps with explicit offset and YYYY/MM/DD HH:MM:SS interpreted as UTC. Empty values last; stable ties. Preserve original timestamp spelling.",
    ];
    Ok(
        json!({"requirement": format!("Read input.csv using CSV-aware parsing. {} Write output.csv with the same header and exact field values, preserving every row. Do not change input.csv.", policies[variant]),
        "policy": policies[variant], "expected_outcome": "success", "complexity": if variant >= 2 { "multi-step" } else { "simple" },
        "invariants": ["ascending ordering under specified policy", "row multiset preserved", "header and schema preserved", "stable equal keys", "input unchanged"]}),
    )
}

fn filesystem(root: &Path, variant: usize, size: usize, seed: u64) -> Result<Value> {
    let content = format!("seed={seed}; café\n").repeat(size);
    write(root, "work/source file [α].txt", &content)?;
    write(root, "work/unchanged.txt", b"preserve exactly\r\n")?;
    let (requirement, outcome, complexity) = match variant {
        0 => (
            "Rename work/source file [α].txt to work/renamed file [α].txt without overwriting any file.",
            "success",
            "simple",
        ),
        1 => (
            "Create directories work/nested and work/nested/deeper, then move work/source file [α].txt to work/nested/deeper/moved file [α].txt without overwriting any file.",
            "success",
            "multi-step",
        ),
        2 => {
            for i in 0..5 {
                write(
                    root,
                    &format!("work/batch/old-{i}.txt"),
                    format!("{seed}:{i}\n{content}"),
                )?;
            }
            (
                "In work/batch only, rename the five files old-0.txt through old-4.txt to new-0.txt through new-4.txt, replacing the leading old- with new-. Do not overwrite destinations. Leave work/source file [α].txt unchanged.",
                "success",
                "multi-step",
            )
        }
        3 => {
            write(root, "work/occupied.txt", b"destination must survive\n")?;
            (
                "Attempt a non-overwriting move of work/source file [α].txt to work/occupied.txt. The destination already exists: report a collision and leave the entire input tree unchanged.",
                "collision_no_change",
                "simple",
            )
        }
        _ => unreachable!(),
    };
    Ok(
        json!({"requirement": format!("{requirement} Preserve all unaffected paths, directories and file bytes. No changes outside work/ are permitted."), "expected_outcome": outcome, "complexity": complexity,
        "invariants": ["correct final paths", "content preserved", "source removed only on successful move", "no overwrites", "no unauthorized tree changes"]}),
    )
}

fn text(root: &Path, variant: usize, size: usize, seed: u64) -> Result<Value> {
    let line =
        format!("seed={seed}: red RED Red redder bred red-red café_red éred redé (red),\r\n");
    write(root, "scope/a.txt", line.repeat(size))?;
    write(root, "scope/b.txt", "red\nredder\nRED\n".repeat(size))?;
    write(root, "scope/nested/c.txt", b"red (red)\n")?;
    write(root, "scope/ignored.bin", b"red\n")?;
    write(root, "outside/untouched.txt", b"red RED\r\n")?;
    let policies = [
        "In scope/a.txt only, replace every exact case-sensitive substring red with blue, including matches in longer words.",
        "In scope/a.txt only, replace case-sensitive whole-word red with blue. A word character is a Unicode alphanumeric character or underscore; a match must have no word character immediately before or after it.",
        "In scope/a.txt and scope/b.txt, replace every ASCII-case-insensitive substring red with lowercase blue, including matches in longer words. Do not apply Unicode case folding.",
        "In every .txt file recursively under scope/ only, replace case-sensitive whole-word red with blue. Word characters are Unicode alphanumeric characters or underscore. Never change files under outside/.",
    ];
    Ok(
        json!({"requirement": format!("{} Edit files in place. Preserve every unrelated byte, UTF-8 encoding and each existing line ending. Create no files.", policies[variant]), "expected_outcome": "success", "complexity": if variant >= 2 { "multi-step" } else { "simple" },
        "invariants": ["all and only required matches replaced", "replacement counts correct", "unrelated bytes preserved", "outside scope identical", "line endings preserved"]}),
    )
}

fn aggregation(root: &Path, variant: usize, size: usize, seed: u64) -> Result<Value> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(["id", "group", "value", "active"])?;
    for id in 0..size {
        let value = if id % 7 == 0 {
            String::new()
        } else {
            ((id as u64 % 101 + seed % 101) % 101).to_string()
        };
        writer.write_record([
            id.to_string(),
            ["A", "B", "C"][id % 3].into(),
            value,
            (id % 2 == 0).to_string(),
        ])?;
    }
    write(root, "input.csv", writer.into_inner()?)?;
    let policies = [
        "Count rows with group A, active true and value greater than 50. Output {\"count\": INTEGER}.",
        "Group all rows by group. For each group compute row count (including null values) and sum of non-null value. Output {\"groups\": {GROUP: {\"count\": INTEGER, \"sum\": INTEGER}}}.",
        "Filter to active true before grouping by group. Compute sum and arithmetic mean of non-null value per group. Output {\"groups\": {GROUP: {\"sum\": INTEGER, \"average\": NUMBER_OR_NULL}}}. Use null for the average when no non-null values exist.",
        "Among rows with active true and group not C, count distinct non-null value integers and count null value rows. Output {\"distinct\": INTEGER, \"null_count\": INTEGER}.",
    ];
    Ok(
        json!({"requirement": format!("Read input.csv using CSV-aware parsing; empty value means null. Values are base-10 integers; active is the literal true or false. {} Write only the specified JSON structure to output.json. Do not change input.csv. JSON key order and whitespace do not matter. Integer results are exact; means have absolute tolerance 0.000000001. Include all groups present after filtering.", policies[variant]), "expected_outcome": "success", "complexity": if variant == 0 { "simple" } else { "multi-step" },
        "invariants": ["semantic JSON equality", "null semantics respected", "filter precedes aggregation", "input unchanged", "no extra keys"]}),
    )
}

pub fn generate(destination: &Path, seed: u64) -> Result<()> {
    fs::create_dir(destination)?;
    let mut tasks = Vec::new();
    for family in ["sorting", "filesystem", "text", "aggregation"] {
        for variant in 0..4 {
            for (size_index, size) in [10, 200, 500].into_iter().enumerate() {
                let id = format!("{family}-v{variant}-s{size_index}");
                let task = destination.join(&id);
                let input = task.join("input");
                fs::create_dir(&task)?;
                fs::create_dir(&input)?;
                let mut manifest = match family {
                    "sorting" => sorting(&input, variant, size, seed)?,
                    "filesystem" => filesystem(&input, variant, size, seed)?,
                    "text" => text(&input, variant, size, seed)?,
                    "aggregation" => aggregation(&input, variant, size, seed)?,
                    _ => unreachable!(),
                };
                if variant == 3 && size_index == 0 && family != "filesystem" {
                    let code = match family {
                        "sorting" => {
                            let path = input.join("input.csv");
                            let content = fs::read_to_string(&path)?;
                            fs::write(path, content.replacen("2024", "BAD!", 1))?;
                            "INVALID_TIMESTAMP"
                        }
                        "text" => {
                            write(&input, "scope/nested/invalid.txt", [0xff, 0xfe])?;
                            "INVALID_UTF8"
                        }
                        "aggregation" => {
                            let path = input.join("input.csv");
                            let content = fs::read_to_string(&path)?;
                            fs::write(path, content.replacen("1,B,", "1,B,not-an-integer", 1))?;
                            "INVALID_NUMBER"
                        }
                        _ => unreachable!(),
                    };
                    manifest["expected_outcome"] = json!(code);
                    manifest["requirement"] = json!(format!(
                        "{} This instance contains invalid input. Validate all target data before making changes. On invalid input leave the entire input tree unchanged, create no output.csv or output.json, and write only result.json containing {{\"error\":\"{code}\"}}.",
                        manifest["requirement"].as_str().unwrap()
                    ));
                }
                if family == "filesystem" && variant == 3 {
                    manifest["requirement"] = json!(format!(
                        "{} Report the collision by writing result.json containing {{\"error\":\"COLLISION\"}} at the input tree root. This is the only permitted added file.",
                        manifest["requirement"].as_str().unwrap()
                    ));
                }
                manifest["id"] = json!(id);
                manifest["family"] = json!(family);
                manifest["variant"] = json!(variant);
                manifest["seed"] = json!(seed);
                manifest["size_parameter"] = json!(size);
                manifest["size_unit"] = json!(match family {
                    "sorting" | "aggregation" => "rows",
                    _ => "repeated_content_blocks",
                });
                manifest["generator_version"] = json!("suite-v3");
                manifest["input_hashes"] = json!(snapshot(&input)?);
                manifest["input_bytes"] = json!(input_bytes(&input)?);
                manifest["limits"] = json!({"status":"dataset_defaults_not_protocol_frozen", "wall_seconds":60, "memory_bytes":536870912, "artifact_bytes":134217728});
                fs::write(
                    task.join("manifest.json"),
                    serde_json::to_vec_pretty(&manifest)?,
                )?;
                tasks.push(id);
            }
        }
    }
    fs::write(
        destination.join("dataset.json"),
        serde_json::to_vec_pretty(
            &json!({"version":"suite-v3", "status":"draft", "seed":seed, "tasks":tasks}),
        )?,
    )?;
    Ok(())
}

fn input_bytes(root: &Path) -> Result<u64> {
    let mut bytes = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        bytes += if entry.file_type()?.is_dir() {
            input_bytes(&entry.path())?
        } else {
            entry.metadata()?.len()
        };
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_suite_is_reproducible_and_manifested() {
        let root = std::env::temp_dir().join(format!(
            "dataset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        generate(&a, 2026).unwrap();
        generate(&b, 2026).unwrap();
        assert_eq!(snapshot(&a).unwrap(), snapshot(&b).unwrap());
        let index: Value =
            serde_json::from_slice(&fs::read(a.join("dataset.json")).unwrap()).unwrap();
        let tasks = index["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 48);
        for family in ["sorting", "filesystem", "text", "aggregation"] {
            assert_eq!(
                tasks
                    .iter()
                    .filter(|id| id.as_str().unwrap().starts_with(family))
                    .count(),
                12
            );
        }
        for id in tasks {
            let task = a.join(id.as_str().unwrap());
            let manifest: Value =
                serde_json::from_slice(&fs::read(task.join("manifest.json")).unwrap()).unwrap();
            assert_eq!(
                manifest["input_hashes"],
                json!(snapshot(&task.join("input")).unwrap())
            );
            assert!(manifest["input_bytes"].as_u64().unwrap() > 0);
            assert!(
                manifest["input_bytes"].as_u64().unwrap()
                    < manifest["limits"]["artifact_bytes"].as_u64().unwrap()
            );
            assert!(manifest["requirement"].as_str().unwrap().len() > 40);
            if manifest["family"] == "sorting" || manifest["family"] == "aggregation" {
                let rows = csv::Reader::from_path(task.join("input/input.csv"))
                    .unwrap()
                    .records()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(
                    rows.len() as u64,
                    manifest["size_parameter"].as_u64().unwrap()
                );
            }
            let output = root.join("reference-output");
            crate::suite::reference(&task, &output).unwrap();
            assert!(
                crate::suite::score(&task, &output).unwrap(),
                "reference rejected: {id}"
            );
            let verification = crate::verifier::verify(&task, &output);
            assert_eq!(
                verification.code,
                crate::certificates::Code::Pass,
                "feedback verifier rejected {id}: {}",
                verification.raw()
            );
            assert!(
                input_bytes(&output).unwrap()
                    < manifest["limits"]["artifact_bytes"].as_u64().unwrap()
            );
            fs::write(output.join("unauthorized.txt"), b"extra").unwrap();
            assert!(
                !crate::suite::score(&task, &output).unwrap(),
                "extra file accepted: {id}"
            );
            assert_ne!(
                crate::verifier::verify(&task, &output).code,
                crate::certificates::Code::Pass,
                "verifier accepted extra file: {id}"
            );
            fs::remove_file(output.join("unauthorized.txt")).unwrap();
            let corrupt = if manifest["expected_outcome"] != "success" {
                "result.json"
            } else {
                match manifest["family"].as_str().unwrap() {
                    "sorting" => "output.csv",
                    "aggregation" => "output.json",
                    "text" => "scope/a.txt",
                    "filesystem" => "work/unchanged.txt",
                    _ => unreachable!(),
                }
            };
            fs::write(output.join(corrupt), b"corrupt").unwrap();
            assert!(
                !crate::suite::score(&task, &output).unwrap_or(false),
                "corruption accepted: {id}"
            );
            assert_ne!(
                crate::verifier::verify(&task, &output).code,
                crate::certificates::Code::Pass,
                "verifier accepted corruption: {id}"
            );
            fs::remove_dir_all(output).unwrap();
        }
        let changed = root.join("changed-seed");
        fs::create_dir(&changed).unwrap();
        sorting(&changed, 0, 10, 2027).unwrap();
        assert_ne!(
            fs::read(changed.join("input.csv")).unwrap(),
            fs::read(a.join("sorting-v0-s0/input/input.csv")).unwrap()
        );
        assert!(generate(&a, 1).is_err());
        fs::remove_dir_all(root).unwrap();
        println!(
            "Validated 48 tasks: 12 per family; two identical generations; oracle and independent feedback verifier agree on 48 accepted reference trees, 48 rejected unauthorized files and 48 rejected corrupted artifacts; all original and final trees below artifact limits."
        );
    }
}
