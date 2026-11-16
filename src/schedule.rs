use serde_json::{Value, json};
use std::{collections::BTreeSet, error::Error, fs, io::Write, path::Path};

pub fn generate(
    dataset: &Path,
    destination: &Path,
    seed: u64,
    repetitions: u64,
) -> Result<(), Box<dyn Error>> {
    if !(3..=100).contains(&repetitions) {
        return Err("main schedule requires 3 through 100 repetitions".into());
    }
    let manifest: Value = serde_json::from_slice(&fs::read(dataset.join("dataset.json"))?)?;
    let tasks = manifest["tasks"].as_array().ok_or("missing tasks")?;
    if tasks.len() != 48 {
        return Err("main schedule requires 48 tasks".into());
    }
    let mut unique = BTreeSet::new();
    let mut order = Vec::new();
    for task in tasks {
        let id = task.as_str().ok_or("invalid task id")?;
        if id.is_empty()
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err("unsafe task id".into());
        }
        if !unique.insert(id) {
            return Err("duplicate task id".into());
        }
        for repetition in 0..repetitions {
            for arm in ["A", "B", "C", "D"] {
                order.push(json!({"task_id":id,"arm":arm,"repetition":repetition}));
            }
        }
    }
    // SplitMix64 + rejection sampling, separate from dataset generation's PRNG.
    shuffle(&mut order, seed);
    let result = json!({"status":"draft_not_authorized_for_execution","algorithm":"splitmix64-fisher-yates-v1","randomization_seed":seed,"dataset":manifest,"repetitions":repetitions,"runs":order});
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    file.write_all(&serde_json::to_vec_pretty(&result)?)?;
    Ok(())
}

/// Fisher-Yates over a SplitMix64 stream. Deterministic for a given seed and length.
pub(crate) fn shuffle(order: &mut [Value], seed: u64) {
    let mut state = seed;
    for index in (1..order.len()).rev() {
        let bound = index as u64 + 1;
        let threshold = bound.wrapping_neg() % bound;
        let random = loop {
            state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut value = state;
            value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
            value ^= value >> 31;
            if value >= threshold {
                break value;
            }
        };
        order.swap(index, (random % bound) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paired_matrix_is_randomized_and_reproducible() {
        let root = std::env::temp_dir().join(format!("schedule-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("dataset.json"),
            json!({"tasks":(0..48).map(|i|format!("task-{i}")).collect::<Vec<_>>(),"seed":2026})
                .to_string(),
        )
        .unwrap();
        generate(&root, &root.join("a.json"), 42, 3).unwrap();
        generate(&root, &root.join("b.json"), 42, 3).unwrap();
        generate(&root, &root.join("c.json"), 43, 3).unwrap();
        let a = fs::read(root.join("a.json")).unwrap();
        assert_eq!(a, fs::read(root.join("b.json")).unwrap());
        assert_ne!(a, fs::read(root.join("c.json")).unwrap());
        let parsed: Value = serde_json::from_slice(&a).unwrap();
        let runs = parsed["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 576);
        assert_eq!(
            runs.iter()
                .map(Value::to_string)
                .collect::<BTreeSet<_>>()
                .len(),
            576
        );
        assert!(generate(&root, &root.join("a.json"), 42, 3).is_err());
        assert!(generate(&root, &root.join("bad.json"), 42, 2).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
