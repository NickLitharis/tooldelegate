use std::{collections::HashMap, error::Error, path::Path};

type Rows = Vec<(Vec<String>, i64)>;

fn read(path: &Path) -> Result<(Vec<String>, Rows), Box<dyn Error>> {
    let mut reader = csv::ReaderBuilder::new().flexible(false).from_path(path)?;
    let header = reader
        .headers()?
        .iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if header != ["id", "value"] {
        return Err("invalid schema".into());
    }
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record?;
        let value = record[1].parse::<i64>()?;
        rows.push((record.iter().map(str::to_owned).collect(), value));
    }
    Ok((header, rows))
}

/// Score semantic CSV content, including stable ordering of identical keys.
pub fn numeric_sort(input: &Path, output: &Path) -> Result<bool, Box<dyn Error>> {
    let (input_header, input_rows) = read(input)?;
    let (output_header, output_rows) = read(output)?;
    if input_header != output_header || input_rows.len() != output_rows.len() {
        return Ok(false);
    }
    let mut original: HashMap<i64, Vec<Vec<String>>> = HashMap::new();
    let mut result: HashMap<i64, Vec<Vec<String>>> = HashMap::new();
    for (row, value) in input_rows {
        original.entry(value).or_default().push(row);
    }
    let mut previous = None;
    for (row, value) in output_rows {
        if previous.is_some_and(|last| value < last) {
            return Ok(false);
        }
        previous = Some(value);
        result.entry(value).or_default().push(row);
    }
    Ok(original == result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn rejects_corruption_and_unstable_ties() {
        let root = std::env::temp_dir().join(format!(
            "oracle-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let input = root.join("input.csv");
        let output = root.join("output.csv");
        fs::write(&input, "id,value\na,2\nb,1\nc,2\n").unwrap();
        fs::write(&output, "id,value\nb,1\na,2\nc,2\n").unwrap();
        assert!(numeric_sort(&input, &output).unwrap());
        for bad in [
            "id,value\nb,1\nc,2\na,2\n",
            "id,value\na,2\nb,1\nc,2\n",
            "id,value\nb,1\na,2\n",
            "id,value\nb,1\na,2\nx,2\n",
        ] {
            fs::write(&output, bad).unwrap();
            assert!(!numeric_sort(&input, &output).unwrap());
        }
        fs::write(&output, "id,value\na,nope\n").unwrap();
        assert!(numeric_sort(&input, &output).is_err());
        fs::write(&output, "value,id\n1,b\n2,a\n2,c\n").unwrap();
        assert!(numeric_sort(&input, &output).is_err());
        fs::write(&output, "id,value\nb,1,extra\na,2\nc,2\n").unwrap();
        assert!(numeric_sort(&input, &output).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
