use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::Path};

pub fn generate(destination: &Path, seed: u64, rows: usize) -> Result<(), Box<dyn Error>> {
    if rows == 0 || rows > 100_000 {
        return Err("rows must be between 1 and 100000".into());
    }
    fs::create_dir(destination)?;
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(["id", "group", "value"])?;
    for id in 0..rows {
        let value = if id % 7 == 0 {
            String::new()
        } else {
            ((id as u64 + seed % 101) % 101).to_string()
        };
        writer.write_record([
            id.to_string(),
            if id % 2 == 0 { "A" } else { "B" }.to_owned(),
            value,
        ])?;
    }
    let bytes = writer.into_inner()?;
    let manifest = serde_json::json!({
        "id": "aggregation-filtered-count", "seed": seed, "rows": rows,
        "input_sha256": format!("{:x}", Sha256::digest(&bytes)),
        "requirement": "Count rows where group is A and non-null integer value is greater than 50. Empty value means null. Write a JSON object with exactly one key count and a nonnegative integer value to output.json.",
        "invariants": ["input unchanged", "exact integer result", "null values excluded"]
    });
    fs::write(destination.join("input.csv"), bytes)?;
    fs::write(
        destination.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}

pub fn score(input: &Path, output: &Path) -> Result<bool, Box<dyn Error>> {
    let mut reader = csv::Reader::from_path(input)?;
    if reader.headers()?.iter().collect::<Vec<_>>() != ["id", "group", "value"] {
        return Err("invalid schema".into());
    }
    let mut count = 0u64;
    for record in reader.records() {
        let record = record?;
        if !record[2].is_empty() {
            let value: i64 = record[2].parse()?;
            if &record[1] == "A" && value > 50 {
                count += 1;
            }
        }
    }
    let actual: serde_json::Value = serde_json::from_slice(&fs::read(output)?)?;
    Ok(actual == serde_json::json!({"count": count}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_count_and_semantic_json() {
        let root = std::env::temp_dir().join(format!("aggregation-task-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        generate(&root.join("a"), 9, 10).unwrap();
        generate(&root.join("b"), 9, 10).unwrap();
        assert_eq!(
            fs::read(root.join("a/input.csv")).unwrap(),
            fs::read(root.join("b/input.csv")).unwrap()
        );
        let input = root.join("hand.csv");
        let output = root.join("output.json");
        fs::write(
            &input,
            "id,group,value\n0,A,51\n1,A,\n2,B,99\n3,A,50\n4,A,100\n",
        )
        .unwrap();
        fs::write(&output, "{ \"count\" : 2 }\n").unwrap();
        assert!(score(&input, &output).unwrap());
        fs::write(&output, "{\"count\":3}").unwrap();
        assert!(!score(&input, &output).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
