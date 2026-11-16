use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, error::Error, fs, path::Path};

/// Hash a complete tree without following links or accepting special files.
pub fn snapshot(root: &Path) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    fn visit(
        root: &Path,
        dir: &Path,
        entries: &mut BTreeMap<String, String>,
    ) -> Result<(), Box<dyn Error>> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let name = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("non-UTF-8 path")?
                .to_owned();
            if kind.is_dir() {
                entries.insert(format!("{name}/"), "directory".to_owned());
                visit(root, &path, entries)?;
            } else if kind.is_file() {
                entries.insert(name, format!("{:x}", Sha256::digest(fs::read(path)?)));
            } else {
                return Err("links and special files are forbidden".into());
            }
        }
        Ok(())
    }
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err("root must be a real directory".into());
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries)?;
    Ok(entries)
}

pub fn generate(destination: &Path, seed: u64) -> Result<(), Box<dyn Error>> {
    fs::create_dir(destination)?;
    let input = destination.join("input");
    fs::create_dir(&input)?;
    fs::write(input.join("source file.txt"), format!("seed={seed}\n"))?;
    fs::write(input.join("unaffected.txt"), b"preserve me\n")?;
    let manifest = serde_json::json!({
        "id": "filesystem-rename", "seed": seed,
        "requirement": "Rename source file.txt to renamed file.txt inside the input directory. Preserve every other path and byte.",
        "input_hashes": snapshot(&input)?,
        "invariants": ["source removed", "content preserved", "no other changes"]
    });
    fs::write(
        destination.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}

pub fn score(before: &Path, after: &Path) -> Result<bool, Box<dyn Error>> {
    let mut expected = snapshot(before)?;
    let source = expected.remove("source file.txt").ok_or("missing source")?;
    if expected.insert("renamed file.txt".into(), source).is_some() {
        return Err("destination collision".into());
    }
    Ok(expected == snapshot(after)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rename_checks_entire_tree() {
        let root = std::env::temp_dir().join(format!("fs-task-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        generate(&root.join("a"), 12).unwrap();
        generate(&root.join("b"), 12).unwrap();
        let a = root.join("a/input");
        let b = root.join("b/input");
        assert_eq!(snapshot(&a).unwrap(), snapshot(&b).unwrap());
        fs::rename(b.join("source file.txt"), b.join("renamed file.txt")).unwrap();
        assert!(score(&a, &b).unwrap());
        fs::write(b.join("unaffected.txt"), "changed").unwrap();
        assert!(!score(&a, &b).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
