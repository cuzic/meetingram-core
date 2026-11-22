use std::{
    fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Result, ensure};
use discussion_graph::{Utterance, eval::GoldGraph};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Deserialize)]
pub(crate) struct Gold {
    #[serde(flatten)]
    pub graph: GoldGraph,
    #[serde(default)]
    pub duplicate_issue_pairs: Vec<[String; 2]>,
}

pub(crate) fn load(name: &str) -> Result<(Vec<Utterance>, Gold)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = if name == "long" {
        root.join("data")
    } else {
        root.join("../../crates/discussion-graph/testdata")
    };
    let lines = BufReader::new(fs::File::open(dir.join(format!("{name}.jsonl")))?).lines();
    let mut utterances = Vec::new();
    for line in lines {
        let line = line?;
        let mut utterance: Utterance = serde_json::from_value(line_with_uid(&line)?)
            .map_err(|_| anyhow::anyhow!("invalid utterance record in {name}"))?;
        utterance.uid = uid(utterance.row_id);
        utterances.push(utterance);
    }
    utterances.sort_by_key(|u| u.row_id);
    ensure!(
        utterances
            .windows(2)
            .all(|pair| pair[0].row_id < pair[1].row_id),
        "duplicate row ID in {name}"
    );
    Ok((
        utterances,
        read_json(&dir.join(format!("{name}.gold.json")))?,
    ))
}

fn line_with_uid(line: &str) -> Result<serde_json::Value> {
    let mut value: serde_json::Value =
        serde_json::from_str(line).map_err(|_| anyhow::anyhow!("invalid utterance JSON"))?;
    let fields = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("utterance must be an object"))?;
    fields.insert("uid".to_owned(), "".into());
    Ok(value)
}

pub(crate) fn uid(row_id: i64) -> String {
    format!("u{row_id}")
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(BufReader::new(fs::File::open(path)?))
        .map_err(|_| anyhow::anyhow!("invalid JSON in {}", path.display()))
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut writer = BufWriter::new(fs::File::create(path)?);
    serde_json::to_writer(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn trial_path(out: &Path, meeting: &str, style: &str, repeat: u16) -> PathBuf {
    out.join(meeting).join(format!("{style}-{repeat}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use discussion_graph::NodeKind;
    use std::collections::BTreeSet;

    #[test]
    fn long_gold_has_pairs_on_opposite_sides_of_the_halfway_mark() {
        let (utterances, gold) = load("long").unwrap();
        assert_eq!(utterances.len(), 600);
        assert_eq!(utterances.first().unwrap().start_ms, Some(0));
        assert_eq!(utterances.last().unwrap().end_ms, Some(3_600_000));
        assert!(gold.duplicate_issue_pairs.len() >= 3);
        let mut seen = BTreeSet::new();
        for pair in &gold.duplicate_issue_pairs {
            for (id, early) in pair.iter().zip([true, false]) {
                assert!(seen.insert(id));
                let node = gold.graph.nodes.iter().find(|node| &node.id == id).unwrap();
                assert_eq!(node.kind, NodeKind::Issue);
                assert!(!node.grounds.is_empty());
                assert!(node.grounds.iter().all(|row| {
                    let u = utterances.iter().find(|u| u.row_id == *row).unwrap();
                    (u.start_ms.unwrap() < 1_800_000) == early
                }));
            }
        }
    }
}
