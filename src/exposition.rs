//! A snapshot in Prometheus's text exposition format, version 0.0.4: what
//! a Prometheus server reads when it scrapes `/metrics`.
//!
//! One metric family per figure observe names (`observe::FIGURES`) that
//! has points, its name the figure's segments joined by underscores —
//! `xmip_health`, `xmip_health_severity`, `xmip_bytes` — with its `# HELP`
//! and `# TYPE`, then one sample per scope, labelled `scope` and, for a
//! mood, `mood`. No timestamps: a scrape is of now, which is what
//! Prometheus asks an exporter to expose.
//!
//! Every figure is a `gauge`. A mood, a severity and what awaits another
//! try are levels; a window's count starts again from nothing each window,
//! and a Prometheus `counter` only ever rises, so a count over a window is
//! a gauge too — read as it stands, not through `rate()`. No name carries
//! `_total`, which the conventions keep for counters.

use std::fmt::Write as _;

use observe::{FIGURES, Figure, Health, Point, Reading, Snapshot};

/// The media type a scrape is answered with.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// `snapshot` in the exposition format, appended to `out`.
pub fn write(out: &mut String, snapshot: &Snapshot) {
    let reading = Reading::of(snapshot);
    let labels = Labels::of(&reading);
    for figure in &FIGURES {
        let points = reading.points(figure);
        if points.len() == 0 {
            continue;
        }
        let name = name(figure);
        out.push_str("# HELP ");
        out.push_str(&name);
        out.push(' ');
        escape(out, figure.description, false);
        out.push_str("\n# TYPE ");
        out.push_str(&name);
        out.push_str(" gauge\n");
        for point in points {
            out.push_str(&name);
            out.push('{');
            out.push_str(labels.scope(point.at));
            if let Some(mood) = point.mood {
                out.push_str(labels.mood(mood));
            }
            // Writing to a String does not fail.
            let _ = writeln!(out, "}} {}", point.value);
        }
    }
}

/// A figure's metric name: its segments joined by underscores, anything
/// the format does not allow in a name made one.
fn name(figure: &Figure) -> String {
    figure
        .name('_')
        .chars()
        .enumerate()
        .map(|(at, character)| match character {
            'a'..='z' | 'A'..='Z' | '_' | ':' => character,
            '0'..='9' if at > 0 => character,
            _ => '_',
        })
        .collect()
}

/// `text` as the format escapes it: a backslash and a line feed always, a
/// double quote in a label's value.
fn escape(out: &mut String, text: &str, quoted: bool) {
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' if quoted => out.push_str("\\\""),
            _ => out.push(character),
        }
    }
}

/// Each scope's label and each mood's written once for a scrape, rather
/// than once for every sample that carries it.
struct Labels {
    /// Where each scope's label ends in `written`, in the order of
    /// [`Reading::scopes`].
    ends: Vec<usize>,
    written: String,
    /// Each mood's label, a comma before it, by rank.
    moods: Vec<String>,
}

impl Labels {
    fn of(reading: &Reading<'_>) -> Self {
        let scopes = reading.scopes();
        let mut written = String::with_capacity(scopes.len() * 56);
        let mut ends = Vec::with_capacity(scopes.len());
        for scope in scopes {
            written.push_str(Point::SCOPE);
            written.push_str("=\"");
            escape(&mut written, scope, true);
            written.push('"');
            ends.push(written.len());
        }
        let moods = Health::ALL
            .iter()
            .map(|mood| format!(",{}=\"{}\"", Point::MOOD, mood.word()))
            .collect();
        Self {
            ends,
            written,
            moods,
        }
    }

    fn mood(&self, mood: Health) -> &str {
        &self.moods[usize::from(mood.rank())]
    }

    /// The label of the scope at `at` in [`Reading::scopes`].
    fn scope(&self, at: usize) -> &str {
        let start = if at == 0 { 0 } else { self.ends[at - 1] };
        &self.written[start..self.ends[at]]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::reader::read;
    use observe::{Count, Counted, Health, HealthRecord};

    fn scope(index: usize) -> String {
        format!("xmip:///c1/node/n1/receive/location-{index:04}")
    }

    /// `scopes` Receive Locations, each with a mood and a count of every
    /// kind.
    pub(crate) fn snapshot(scopes: usize) -> Snapshot {
        let mut snapshot = Snapshot::new();
        for index in 0..scopes {
            snapshot.record_health(HealthRecord {
                scope: scope(index),
                health: if index % 7 == 0 {
                    Health::Stressed
                } else {
                    Health::Fine
                },
                severity: u8::try_from(index % 100).expect("under 100"),
                evidence: String::new(),
                observed_unix_nanos: 1_000,
            });
            for counted in Counted::ALL {
                snapshot.record_count(Count {
                    scope: scope(index),
                    counted,
                    value: index as u64,
                    window_start_unix_nanos: 500,
                    window_end_unix_nanos: 1_000,
                    observed_unix_nanos: 1_000,
                });
            }
        }
        snapshot
    }

    fn text(snapshot: &Snapshot) -> String {
        let mut out = String::new();
        write(&mut out, snapshot);
        out
    }

    #[test]
    fn every_figure_is_a_family_the_format_reads() {
        let read = read(&text(&snapshot(3))).expect("the format's rules");
        let families: Vec<&str> = read.types.keys().map(String::as_str).collect();
        assert_eq!(
            families,
            [
                "xmip_bytes",
                "xmip_failed",
                "xmip_health",
                "xmip_health_severity",
                "xmip_journeys",
                "xmip_messages",
                "xmip_retrying",
                "xmip_streams"
            ]
        );
        assert!(read.types.values().all(|kind| kind == "gauge"));
        assert_eq!(read.help.len(), 8, "every family says what it is");
        assert_eq!(read.samples.len(), 24);

        let stressed = &read.samples[0];
        assert_eq!(stressed.name, "xmip_health");
        assert_eq!(
            stressed.labels,
            [
                ("scope".to_string(), scope(0)),
                ("mood".to_string(), "stressed".to_string())
            ]
        );
        assert!(
            (stressed.value - 3.0).abs() < f64::EPSILON,
            "stressed ranks 3"
        );
        let bytes: Vec<f64> = read
            .samples
            .iter()
            .filter(|sample| sample.name == "xmip_bytes")
            .map(|sample| sample.value)
            .collect();
        assert_eq!(bytes, [0.0, 1.0, 2.0]);
    }

    #[test]
    fn a_scope_that_needs_escaping_reads_back_as_itself() {
        let mut snapshot = Snapshot::new();
        let awkward = "xmip:///c1/node/n1/receive/a\"b\\c\nd";
        snapshot.record_health(HealthRecord {
            scope: awkward.to_string(),
            health: Health::Done,
            severity: 90,
            evidence: String::new(),
            observed_unix_nanos: 1,
        });
        let read = read(&text(&snapshot)).expect("the format's rules");
        assert_eq!(read.samples[0].labels[0].1, awkward);
        for word in Health::ALL.map(Health::word) {
            let mut escaped = String::new();
            escape(&mut escaped, word, true);
            assert_eq!(escaped, word, "a mood's word needs no escaping");
        }
    }

    #[test]
    fn an_empty_snapshot_is_an_empty_scrape() {
        assert_eq!(text(&Snapshot::new()), "");
        assert!(read("").is_ok());
    }
}
