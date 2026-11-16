//! Host-only reference and oracle. Never mount these interfaces into a model sandbox.
use crate::filesystem_task::snapshot;
use serde_json::{Value, json};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    error::Error,
    fs,
    path::Path,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Score in a bounded host-only worker. Its result is never model feedback.
pub fn score_isolated(task: &Path, after: &Path) -> Result<bool> {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let mut child = Command::new("/usr/bin/prlimit")
        .args([
            "--cpu=30:30",
            "--as=536870912:536870912",
            "--nofile=64:64",
            "--core=0:0",
            "--",
        ])
        .arg(std::env::current_exe()?)
        .arg("score-internal")
        .arg(task)
        .arg(after)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("oracle stdout missing")?;
    let reader = thread::spawn(move || crate::executor::drain(stdout, 65536));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(60) {
            child.kill()?;
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(5));
    };
    let (count, raw) = reader.join().map_err(|_| "oracle reader panicked")??;
    if !status.success() || count > 65536 {
        return Err("oracle worker failed or exhausted resources".into());
    }
    let value: Value = serde_json::from_slice(&raw)?;
    value["pass"]
        .as_bool()
        .ok_or_else(|| "oracle worker rejected artifact format".into())
}

pub fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let entries = snapshot(source)?;
    fs::create_dir(destination)?;
    for (name, _) in entries {
        if name.ends_with('/') {
            fs::create_dir_all(destination.join(name))?;
        } else {
            let path = destination.join(&name);
            fs::create_dir_all(path.parent().ok_or("missing parent")?)?;
            fs::copy(source.join(name), path)?;
        }
    }
    Ok(())
}

fn manifest(task: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(
        task.join("manifest.json"),
    )?)?)
}

fn records(path: &Path, header: &[&str]) -> Result<Vec<Vec<String>>> {
    let mut reader = csv::Reader::from_path(path)?;
    if reader.headers()?.iter().collect::<Vec<_>>() != header {
        return Err("invalid schema".into());
    }
    reader
        .records()
        .map(|row| Ok(row?.iter().map(str::to_owned).collect()))
        .collect()
}

fn timestamp(text: &str) -> Result<i64> {
    if text.contains('T') {
        Ok(chrono::DateTime::parse_from_rfc3339(text)?.timestamp())
    } else {
        Ok(
            chrono::NaiveDateTime::parse_from_str(text, "%Y/%m/%d %H:%M:%S")?
                .and_utc()
                .timestamp(),
        )
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Number(i64),
    Text(String),
}

fn key(row: &[String], variant: usize) -> Result<(bool, Key, i64)> {
    let null = row[1].is_empty();
    let primary = match variant {
        0 => Key::Number(if null { 0 } else { row[1].parse()? }),
        1 => Key::Text(row[1].clone()),
        2 => Key::Text(row[1].to_ascii_lowercase()),
        3 => Key::Number(if null { 0 } else { timestamp(&row[1])? }),
        _ => return Err("unknown variant".into()),
    };
    // Nulls also follow the secondary policy; their primary values are identical.
    Ok((
        null,
        primary,
        if variant == 2 {
            -row[2].parse::<i64>()?
        } else {
            0
        },
    ))
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn reference_text(text: &str, variant: usize) -> String {
    let mut result = String::new();
    let mut offset = 0;
    while offset < text.len() {
        let tail = &text[offset..];
        let matched = tail.as_bytes().get(..3).is_some_and(|s| {
            if variant == 2 {
                s.eq_ignore_ascii_case(b"red")
            } else {
                s == b"red"
            }
        });
        let boundary = !matched
            || !matches!(variant, 1 | 3)
            || (!text[..offset].chars().next_back().is_some_and(word)
                && !tail[3..].chars().next().is_some_and(word));
        if matched && boundary {
            result.push_str("blue");
            offset += 3;
        } else {
            let c = tail.chars().next().unwrap();
            result.push(c);
            offset += c.len_utf8();
        }
    }
    result
}

fn targets(tree: &Path, variant: usize) -> Result<Vec<String>> {
    Ok(snapshot(tree)?
        .into_keys()
        .filter(|name| match variant {
            0 | 1 => name == "scope/a.txt",
            2 => name == "scope/a.txt" || name == "scope/b.txt",
            3 => name.starts_with("scope/") && name.ends_with(".txt"),
            _ => false,
        })
        .collect())
}

fn failure(tree: &Path, family: &str, variant: usize) -> Result<Option<&'static str>> {
    Ok(match family {
        "sorting" => {
            let rows = records(
                &tree.join("input.csv"),
                &["id", "value", "secondary", "payload"],
            )?;
            if rows.iter().any(|r| key(r, variant).is_err()) {
                Some("INVALID_TIMESTAMP")
            } else {
                None
            }
        }
        "text" => {
            let mut invalid = false;
            for path in targets(tree, variant)? {
                if String::from_utf8(fs::read(tree.join(path))?).is_err() {
                    invalid = true;
                }
            }
            if invalid { Some("INVALID_UTF8") } else { None }
        }
        "aggregation" => {
            let rows = records(&tree.join("input.csv"), &["id", "group", "value", "active"])?;
            if rows
                .iter()
                .any(|r| !r[2].is_empty() && r[2].parse::<i64>().is_err())
            {
                Some("INVALID_NUMBER")
            } else {
                None
            }
        }
        "filesystem" if variant == 3 => Some("COLLISION"),
        _ => None,
    })
}

fn reference_query(rows: &[Vec<String>], variant: usize) -> Result<Value> {
    let mut groups: BTreeMap<String, (u64, i64, u64)> = BTreeMap::new();
    let (mut count, mut nulls) = (0, 0);
    let mut distinct = BTreeSet::new();
    for row in rows {
        let value = if row[2].is_empty() {
            None
        } else {
            Some(row[2].parse::<i64>()?)
        };
        let active = row[3] == "true";
        if row[1] == "A" && active && value.is_some_and(|v| v > 50) {
            count += 1;
        }
        if active && row[1] != "C" {
            if let Some(v) = value {
                distinct.insert(v);
            } else {
                nulls += 1;
            }
        }
        if variant != 2 || active {
            let group = groups.entry(row[1].clone()).or_default();
            group.0 += 1;
            if let Some(v) = value {
                group.1 += v;
                group.2 += 1;
            }
        }
    }
    Ok(match variant {
        0 => json!({"count":count}),
        3 => json!({"distinct":distinct.len(), "null_count":nulls}),
        _ => {
            json!({"groups":groups.into_iter().map(|(name,(count,sum,n))| (name, if variant == 1 { json!({"count":count,"sum":sum}) } else { json!({"sum":sum,"average":if n == 0 { None } else {Some(sum as f64/n as f64)}}) })).collect::<BTreeMap<_,_>>() })
        }
    })
}

pub fn reference(task: &Path, destination: &Path) -> Result<()> {
    let m = manifest(task)?;
    let family = m["family"].as_str().ok_or("missing family")?;
    let variant = m["variant"].as_u64().ok_or("missing variant")? as usize;
    let input = task.join("input");
    copy_tree(&input, destination)?;
    if let Some(code) = failure(&input, family, variant)? {
        fs::write(
            destination.join("result.json"),
            serde_json::to_vec(&json!({"error":code}))?,
        )?;
        return Ok(());
    }
    match family {
        "sorting" => {
            let rows = records(
                &input.join("input.csv"),
                &["id", "value", "secondary", "payload"],
            )?;
            let mut keyed = rows
                .into_iter()
                .map(|r| Ok((key(&r, variant)?, r)))
                .collect::<Result<Vec<_>>>()?;
            keyed.sort_by(|a, b| a.0.cmp(&b.0));
            let mut writer = csv::Writer::from_path(destination.join("output.csv"))?;
            writer.write_record(["id", "value", "secondary", "payload"])?;
            for (_, row) in keyed {
                writer.write_record(row)?;
            }
            writer.flush()?;
        }
        "text" => {
            for name in targets(&input, variant)? {
                fs::write(
                    destination.join(&name),
                    reference_text(&fs::read_to_string(input.join(&name))?, variant),
                )?;
            }
        }
        "filesystem" => match variant {
            0 => fs::rename(
                destination.join("work/source file [α].txt"),
                destination.join("work/renamed file [α].txt"),
            )?,
            1 => {
                fs::create_dir_all(destination.join("work/nested/deeper"))?;
                fs::rename(
                    destination.join("work/source file [α].txt"),
                    destination.join("work/nested/deeper/moved file [α].txt"),
                )?;
            }
            2 => {
                for i in 0..5 {
                    fs::rename(
                        destination.join(format!("work/batch/old-{i}.txt")),
                        destination.join(format!("work/batch/new-{i}.txt")),
                    )?;
                }
            }
            _ => return Err("unknown variant".into()),
        },
        "aggregation" => fs::write(
            destination.join("output.json"),
            serde_json::to_vec(&reference_query(
                &records(
                    &input.join("input.csv"),
                    &["id", "group", "value", "active"],
                )?,
                variant,
            )?)?,
        )?,
        _ => return Err("unknown family".into()),
    }
    Ok(())
}

/// Independently check ordering and provenance, without sorting a reference artifact.
fn score_sort(input: &Path, output: &Path, variant: usize) -> Result<bool> {
    let original = records(input, &["id", "value", "secondary", "payload"])?;
    let actual = records(output, &["id", "value", "secondary", "payload"])?;
    if original.len() != actual.len() {
        return Ok(false);
    }
    let mut positions: HashMap<Vec<String>, VecDeque<usize>> = HashMap::new();
    for (index, row) in original.into_iter().enumerate() {
        positions.entry(row).or_default().push_back(index);
    }
    let mut previous: Option<((bool, Key, i64), usize)> = None;
    for row in actual {
        let Some(index) = positions.get_mut(&row).and_then(VecDeque::pop_front) else {
            return Ok(false);
        };
        let current = key(&row, variant)?;
        if let Some((ref last, last_index)) = previous {
            match last.cmp(&current) {
                Ordering::Greater => return Ok(false),
                Ordering::Equal if last_index > index => return Ok(false),
                _ => (),
            }
        }
        previous = Some((current, index));
    }
    Ok(true)
}

fn oracle_text(input: &str, variant: usize) -> String {
    let search = if variant == 2 {
        input.to_ascii_lowercase()
    } else {
        input.to_owned()
    };
    let mut out = String::new();
    let mut end = 0;
    for (start, _) in search.match_indices("red") {
        let after = start + 3;
        if matches!(variant, 1 | 3)
            && (input[..start].chars().next_back().is_some_and(word)
                || input[after..].chars().next().is_some_and(word))
        {
            continue;
        }
        out.push_str(&input[end..start]);
        out.push_str("blue");
        end = after;
    }
    out.push_str(&input[end..]);
    out
}

/// Different computation path: select rows per group, then reduce each column.
fn oracle_query(rows: &[Vec<String>], variant: usize) -> Result<Value> {
    let parsed = rows
        .iter()
        .map(|r| {
            Ok((
                r[1].as_str(),
                r[3] == "true",
                if r[2].is_empty() {
                    None
                } else {
                    Some(r[2].parse::<i64>()?)
                },
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    if variant == 0 {
        return Ok(
            json!({"count":parsed.iter().filter(|(g,a,v)| *g == "A" && *a && v.is_some_and(|x| x > 50)).count()}),
        );
    }
    if variant == 3 {
        let selected: Vec<_> = parsed.iter().filter(|(g, a, _)| *a && *g != "C").collect();
        return Ok(
            json!({"distinct":selected.iter().filter_map(|(_,_,v)| *v).collect::<BTreeSet<_>>().len(), "null_count":selected.iter().filter(|(_,_,v)| v.is_none()).count()}),
        );
    }
    let selected: Vec<_> = parsed
        .iter()
        .filter(|(_, a, _)| variant == 1 || *a)
        .collect();
    let mut groups = BTreeMap::new();
    for name in selected.iter().map(|(g, _, _)| *g).collect::<BTreeSet<_>>() {
        let group: Vec<_> = selected.iter().filter(|(g, _, _)| *g == name).collect();
        let values: Vec<_> = group.iter().filter_map(|(_, _, v)| *v).collect();
        let sum: i64 = values.iter().sum();
        groups.insert(name, if variant == 1 { json!({"count":group.len(),"sum":sum}) } else {json!({"sum":sum,"average":if values.is_empty() {None} else {Some(sum as f64 / values.len() as f64)}})});
    }
    Ok(json!({"groups":groups}))
}

fn equal(expected: &Value, actual: &Value, average: bool) -> bool {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            e.len() == a.len()
                && e.iter()
                    .all(|(k, v)| a.get(k).is_some_and(|x| equal(v, x, k == "average")))
        }
        (Value::Number(e), Value::Number(a)) if average => {
            (e.as_f64().unwrap() - a.as_f64().unwrap()).abs() <= 1e-9
        }
        _ => expected == actual,
    }
}

pub fn score(task: &Path, after: &Path) -> Result<bool> {
    let m = manifest(task)?;
    let input = task.join("input");
    let mut expected = snapshot(&input)?;
    if json!(expected) != m["input_hashes"] {
        return Err("original input hash mismatch".into());
    }
    let mut actual = snapshot(after)?;
    let family = m["family"].as_str().ok_or("missing family")?;
    let variant = m["variant"].as_u64().ok_or("missing variant")? as usize;
    let outcome = m["expected_outcome"].as_str().ok_or("missing outcome")?;
    if outcome != "success" {
        let code = if outcome == "collision_no_change" {
            "COLLISION"
        } else {
            outcome
        };
        let report: Value = serde_json::from_slice(&fs::read(after.join("result.json"))?)?;
        actual.remove("result.json");
        return Ok(expected == actual && report == json!({"error":code}));
    }
    let valid = match family {
        "sorting" => {
            actual.remove("output.csv");
            score_sort(&input.join("input.csv"), &after.join("output.csv"), variant)?
        }
        "aggregation" => {
            actual.remove("output.json");
            equal(
                &oracle_query(
                    &records(
                        &input.join("input.csv"),
                        &["id", "group", "value", "active"],
                    )?,
                    variant,
                )?,
                &serde_json::from_slice(&fs::read(after.join("output.json"))?)?,
                false,
            )
        }
        "text" => {
            for name in targets(&input, variant)? {
                let wanted = oracle_text(&fs::read_to_string(input.join(&name))?, variant);
                if fs::read(after.join(&name))? != wanted.as_bytes() {
                    return Ok(false);
                }
                expected.remove(&name);
                actual.remove(&name);
            }
            true
        }
        "filesystem" => {
            let renames = match variant {
                0 => vec![(
                    "work/source file [α].txt".into(),
                    "work/renamed file [α].txt".into(),
                )],
                1 => {
                    expected.insert("work/nested/".into(), "directory".into());
                    expected.insert("work/nested/deeper/".into(), "directory".into());
                    vec![(
                        "work/source file [α].txt".into(),
                        "work/nested/deeper/moved file [α].txt".into(),
                    )]
                }
                2 => (0..5)
                    .map(|i| {
                        (
                            format!("work/batch/old-{i}.txt"),
                            format!("work/batch/new-{i}.txt"),
                        )
                    })
                    .collect(),
                _ => return Err("unknown variant".into()),
            };
            for (from, to) in renames {
                let hash = expected.remove(&from).ok_or("missing source")?;
                if expected.insert(to, hash).is_some() {
                    return Err("collision".into());
                }
            }
            true
        }
        _ => return Err("unknown family".into()),
    };
    Ok(valid && expected == actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_normalization_and_null_policy() {
        assert_eq!(timestamp("2024-01-01T00:00:00Z").unwrap(), 1_704_067_200);
        assert_eq!(
            timestamp("2023-12-31T19:00:00-05:00").unwrap(),
            1_704_067_200
        );
        assert_eq!(timestamp("2024/01/01 12:00:00").unwrap(), 1_704_110_400);
        assert!(timestamp("2024-02-30T00:00:00Z").is_err());
        let row = |value: &str, secondary: &str| {
            vec!["0".into(), value.into(), secondary.into(), "payload".into()]
        };
        assert!(key(&row("9", "0"), 0).unwrap() < key(&row("", "0"), 0).unwrap());
        assert!(key(&row("Alpha", "2"), 2).unwrap() < key(&row("alpha", "1"), 2).unwrap());
    }

    #[test]
    fn text_matches_hand_calculated_results() {
        let input = "red RED Red redder bred red-red café_red éred redé (red),\r\n";
        let cases = [
            (
                0,
                "blue RED Red blueder bblue blue-blue café_blue éblue blueé (blue),\r\n",
            ),
            (
                1,
                "blue RED Red redder bred blue-blue café_red éred redé (blue),\r\n",
            ),
            (
                2,
                "blue blue blue blueder bblue blue-blue café_blue éblue blueé (blue),\r\n",
            ),
            (
                3,
                "blue RED Red redder bred blue-blue café_red éred redé (blue),\r\n",
            ),
        ];
        for (variant, expected) in cases {
            assert_eq!(reference_text(input, variant), expected);
            assert_eq!(oracle_text(input, variant), expected);
        }
        for input in ["", "é", "éé", "🦀red", "red🦀", "r", "re", "éredé"] {
            for variant in 0..4 {
                assert_eq!(reference_text(input, variant), oracle_text(input, variant));
            }
        }
    }

    #[test]
    fn aggregations_match_hand_calculated_results() {
        let rows = [
            ["0", "A", "60", "true"],
            ["1", "A", "", "true"],
            ["2", "A", "60", "false"],
            ["3", "B", "10", "true"],
            ["4", "B", "20", "true"],
            ["5", "C", "", "true"],
        ]
        .map(|r| r.into_iter().map(str::to_owned).collect::<Vec<_>>());
        let expected = [
            json!({"count":1}),
            json!({"groups":{"A":{"count":3,"sum":120},"B":{"count":2,"sum":30},"C":{"count":1,"sum":0}}}),
            json!({"groups":{"A":{"sum":60,"average":60.0},"B":{"sum":30,"average":15.0},"C":{"sum":0,"average":null}}}),
            json!({"distinct":3,"null_count":1}),
        ];
        for (variant, wanted) in expected.iter().enumerate() {
            assert_eq!(reference_query(&rows, variant).unwrap(), *wanted);
            assert_eq!(oracle_query(&rows, variant).unwrap(), *wanted);
        }
        assert!(equal(
            &json!({"average":1.0}),
            &json!({"average":1.0000000005}),
            false
        ));
        assert!(!equal(
            &json!({"average":1.0}),
            &json!({"average":1.000001}),
            false
        ));
        assert!(!equal(
            &json!({"count":1}),
            &json!({"count":1,"extra":0}),
            false
        ));
        assert!(!equal(&json!({"count":1}), &json!({"count":1.1}), false));
    }

    #[test]
    fn sort_oracle_rejects_unstable_ties_and_changed_rows() {
        let root = std::env::temp_dir().join(format!("suite-sort-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let input = root.join("input.csv");
        let output = root.join("output.csv");
        fs::write(
            &input,
            "id,value,secondary,payload\na,2,0,x\nb,1,0,y\nc,2,0,z\nd,,0,q\n",
        )
        .unwrap();
        fs::write(
            &output,
            "id,value,secondary,payload\nb,1,0,y\na,2,0,x\nc,2,0,z\nd,,0,q\n",
        )
        .unwrap();
        assert!(score_sort(&input, &output, 0).unwrap());
        for bad in [
            "b,1,0,y\nc,2,0,z\na,2,0,x\nd,,0,q\n",
            "b,1,0,y\na,2,0,x\nc,2,0,CHANGED\nd,,0,q\n",
            "d,,0,q\nb,1,0,y\na,2,0,x\nc,2,0,z\n",
            "b,1,0,y\na,2,0,x\na,2,0,x\nd,,0,q\n",
        ] {
            fs::write(&output, format!("id,value,secondary,payload\n{bad}")).unwrap();
            assert!(!score_sort(&input, &output, 0).unwrap());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
