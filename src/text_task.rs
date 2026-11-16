use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::Path};

/// Create a deterministic exact-replacement input; the expected output is not stored.
pub fn generate(destination: &Path, seed: u64) -> Result<(), Box<dyn Error>> {
    fs::create_dir(destination)?;
    let input = format!("seed={seed}\nred redder red\nunchanged: café\n");
    let manifest = serde_json::json!({
        "id": "text-exact-replacement",
        "seed": seed,
        "input": "input.txt",
        "input_sha256": format!("{:x}", Sha256::digest(input.as_bytes())),
        "requirement": "Replace every exact case-sensitive occurrence of red with blue in input.txt, including occurrences inside longer words. Write the result to output.txt. Preserve all other bytes.",
        "invariants": ["all exact matches replaced", "unrelated bytes unchanged", "UTF-8 and line endings preserved"]
    });
    fs::write(destination.join("input.txt"), input)?;
    fs::write(
        destination.join("manifest.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    Ok(())
}

/// Compare bytes against the independently computed replacement.
pub fn score(input: &Path, output: &Path) -> Result<bool, Box<dyn Error>> {
    let input = fs::read(input)?;
    let output = fs::read(output)?;
    let mut expected = Vec::with_capacity(input.len());
    let mut offset = 0;
    while offset < input.len() {
        if input[offset..].starts_with(b"red") {
            expected.extend_from_slice(b"blue");
            offset += 3;
        } else {
            expected.push(input[offset]);
            offset += 1;
        }
    }
    Ok(output == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn deterministic_and_rejects_changed_bytes() {
        let root = std::env::temp_dir().join(format!(
            "text-task-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        generate(&a, 42).unwrap();
        generate(&b, 42).unwrap();
        let input = fs::read(a.join("input.txt")).unwrap();
        assert_eq!(input, fs::read(b.join("input.txt")).unwrap());
        assert_eq!(
            fs::read(a.join("manifest.json")).unwrap(),
            fs::read(b.join("manifest.json")).unwrap()
        );
        let output = a.join("output.txt");
        fs::write(&output, "seed=42\nblue blueder blue\nunchanged: café\n").unwrap();
        assert!(score(&a.join("input.txt"), &output).unwrap());
        fs::write(&output, "seed=42\nblue redder blue\nunchanged: café\n").unwrap();
        assert!(!score(&a.join("input.txt"), &output).unwrap());
        fs::write(&output, "seed=42\nblue blueder blue\r\nunchanged: café\n").unwrap();
        assert!(!score(&a.join("input.txt"), &output).unwrap());
        assert!(generate(&a, 42).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
