//! The text exposition format 0.0.4 read as a Prometheus server reads it,
//! strictly, for the tests: every line a comment, a blank, or a sample; a
//! metric name and a label name as the format's grammar allows them; a
//! label value's escapes only the three there are; a value a float; one
//! `# TYPE` per family, before its first sample and of a type there is;
//! one `# HELP` per family; a family's samples together; no two samples
//! with one name and one set of labels; the text ending in a line feed.

// Test code, all of it: lib.rs declares this module under `#[cfg(test)]`,
// and the style gates find where production code ends by the line below.
#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};

/// One sample as read: its name, its labels in order, its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub value: f64,
}

/// What a scrape reads: each family's type and help, and the samples.
#[derive(Debug, Default)]
pub struct Read {
    pub types: BTreeMap<String, String>,
    pub help: BTreeMap<String, String>,
    pub samples: Vec<Sample>,
}

fn is_name(text: &str, colons: bool) -> bool {
    let mut characters = text.chars();
    let first_ok = characters
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || (colons && c == ':'));
    first_ok && characters.all(|c| c.is_ascii_alphanumeric() || c == '_' || (colons && c == ':'))
}

/// Read `text`, or say which line breaks which rule.
pub fn read(text: &str) -> Result<Read, String> {
    if !text.is_empty() && !text.ends_with('\n') {
        return Err("the text does not end in a line feed".into());
    }
    let mut read = Read::default();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut current: Option<String> = None;
    let mut seen: BTreeSet<(String, Vec<(String, String)>)> = BTreeSet::new();
    for (number, line) in text.lines().enumerate() {
        let at = |why: &str| format!("line {}: {why}: {line}", number + 1);
        if line.trim().is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            let mut words = comment.trim_start().splitn(3, ' ');
            let (kind, name, rest) = (words.next(), words.next(), words.next());
            match (kind, name) {
                (Some("TYPE"), Some(name)) => {
                    let kind = rest.unwrap_or_default();
                    if !["counter", "gauge", "histogram", "summary", "untyped"].contains(&kind) {
                        return Err(at("a type there is not"));
                    }
                    if read.types.insert(name.into(), kind.into()).is_some() {
                        return Err(at("a second TYPE"));
                    }
                    if read.samples.iter().any(|sample| sample.name == name) {
                        return Err(at("a TYPE after its samples"));
                    }
                }
                (Some("HELP"), Some(name))
                    if read
                        .help
                        .insert(name.into(), rest.unwrap_or_default().into())
                        .is_some() =>
                {
                    return Err(at("a second HELP"));
                }
                _ => {}
            }
            continue;
        }
        let sample = sample(line).map_err(|why| at(&why))?;
        if current.as_deref() != Some(sample.name.as_str()) {
            if done.contains(&sample.name) {
                return Err(at("a family's samples are not together"));
            }
            if let Some(previous) = current.replace(sample.name.clone()) {
                done.insert(previous);
            }
        }
        if !seen.insert((sample.name.clone(), sample.labels.clone())) {
            return Err(at("a sample repeated"));
        }
        read.samples.push(sample);
    }
    Ok(read)
}

fn sample(line: &str) -> Result<Sample, String> {
    let (name, rest) = match line.find(['{', ' ']) {
        Some(at) => line.split_at(at),
        None => return Err("a sample with no value".into()),
    };
    if !is_name(name, true) {
        return Err(format!("'{name}' is not a metric name"));
    }
    let (labels, rest) = if let Some(inside) = rest.strip_prefix('{') {
        labels(inside)?
    } else {
        (Vec::new(), rest)
    };
    let mut parts = rest.split_whitespace();
    let value = parts.next().ok_or("a sample with no value")?;
    let value = match value {
        "+Inf" => f64::INFINITY,
        "-Inf" => f64::NEG_INFINITY,
        "NaN" => f64::NAN,
        number => number
            .parse()
            .map_err(|_| format!("'{number}' is not a value"))?,
    };
    if let Some(timestamp) = parts.next() {
        timestamp
            .parse::<i64>()
            .map_err(|_| format!("'{timestamp}' is not a timestamp"))?;
    }
    if parts.next().is_some() {
        return Err("more after the timestamp".into());
    }
    Ok(Sample {
        name: name.into(),
        labels,
        value,
    })
}

/// A sample's labels, by name and value, in order.
type Labels = Vec<(String, String)>;

/// The labels up to the closing brace, and what follows it.
fn labels(mut text: &str) -> Result<(Labels, &str), String> {
    let mut labels = Vec::new();
    loop {
        if let Some(rest) = text.strip_prefix('}') {
            return Ok((labels, rest));
        }
        let (name, rest) = text.split_once("=\"").ok_or("a label with no value")?;
        if !is_name(name, false) || name.starts_with("__") {
            return Err(format!("'{name}' is not a label name"));
        }
        let mut value = String::new();
        let mut characters = rest.char_indices();
        let end = loop {
            match characters.next() {
                Some((_, '\\')) => match characters.next() {
                    Some((_, '\\')) => value.push('\\'),
                    Some((_, '"')) => value.push('"'),
                    Some((_, 'n')) => value.push('\n'),
                    _ => return Err("an escape there is not".into()),
                },
                Some((at, '"')) => break at,
                Some((_, character)) => value.push(character),
                None => return Err("a label value that never ends".into()),
            }
        };
        if labels.iter().any(|(held, _)| held == name) {
            return Err(format!("the label '{name}' twice"));
        }
        labels.push((name.into(), value));
        text = &rest[end + 1..];
        text = text.strip_prefix(',').unwrap_or(text);
    }
}

#[test]
fn the_reader_refuses_what_the_format_does() {
    assert!(read("a 1\n").is_ok());
    assert!(read("a 1").is_err(), "no line feed at the end");
    assert!(read("1a 1\n").is_err(), "a name opening with a digit");
    assert!(read("a{__x=\"1\"} 1\n").is_err(), "a reserved label name");
    assert!(read("a{x=\"\\t\"} 1\n").is_err(), "an escape there is not");
    assert!(read("a 1\nb 1\na{x=\"1\"} 1\n").is_err(), "a family apart");
    assert!(read("a 1\na 2\n").is_err(), "a sample repeated");
    assert!(
        read("a 1\n# TYPE a gauge\n").is_err(),
        "a type after its samples"
    );
    assert!(read("# TYPE a meter\n").is_err(), "a type there is not");
    assert!(read("a one\n").is_err(), "a value that is not one");
}
