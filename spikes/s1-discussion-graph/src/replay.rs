use crate::graph::Graph;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::Path,
};

const SYSTEM: &str = include_str!("prompt.txt");

#[derive(Deserialize)]
pub struct Row {
    pub row_id: u64,
    pub speaker_label: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub fn load(path: &Path) -> Result<Vec<Row>> {
    let rows: Vec<Row> = fs::read_to_string(path)?
        .lines()
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line).with_context(|| format!("行 {}", index + 1))
        })
        .collect::<Result<_>>()?;
    ensure!(
        rows.iter().all(|row| row.start_ms <= row.end_ms),
        "発話の終了時刻が開始時刻より前です"
    );
    ensure!(
        rows.windows(2)
            .all(|pair| pair[0].row_id < pair[1].row_id && pair[0].start_ms <= pair[1].start_ms),
        "発話 ID または時刻が順序どおりではありません"
    );
    Ok(rows)
}

pub fn windows(rows: &[Row], count: usize, ms: u64) -> Vec<usize> {
    let mut boundaries = Vec::new();
    let mut start = 0;
    let mut deadline = rows
        .first()
        .map_or(0, |row| row.start_ms.saturating_add(ms));
    for (index, row) in rows.iter().enumerate() {
        // If a timer expires before this row is finalized, flush previous rows.
        if index > start && row.end_ms > deadline {
            boundaries.push(index);
            start = index;
            deadline = deadline.saturating_add(ms);
            if row.end_ms > deadline {
                deadline = row.start_ms.saturating_add(ms);
            }
        }
        if index + 1 - start >= count || row.end_ms >= deadline {
            boundaries.push(index + 1);
            start = index + 1;
            deadline = row.end_ms.saturating_add(ms);
        }
    }
    if start < rows.len() {
        boundaries.push(rows.len());
    }
    boundaries
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn utterance(row: &Row, index: usize, cap: usize) -> String {
    json!({"uid": format!("u{}", index + 1), "speaker_label": row.speaker_label, "text": row.text.chars().take(cap).collect::<String>()}).to_string()
}

pub struct Input {
    pub prompt: String,
    pub consumed: usize,
    pub grounds: HashMap<String, u64>,
}

pub fn input(
    rows: &[Row],
    pending: std::ops::Range<usize>,
    graph: &Graph,
    recent: &[BTreeSet<usize>],
    limit: usize,
) -> Input {
    let start = pending.start;
    let mut context = Vec::new();
    let mut budget = limit / 3;
    let mut seen = BTreeSet::new();
    let open = graph
        .nodes
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, node)| node.kind == "issue" && node.status == "open")
        .map(|(index, _)| index);
    let mut priority = open.chain(
        recent
            .iter()
            .rev()
            .flat_map(|patch| patch.iter().rev().copied()),
    );
    let mut reference: Vec<_> = (start.saturating_sub(3)..start)
        .map(|index| utterance(&rows[index], index, 200))
        .collect();
    let mut fresh = Vec::new();
    let first = utterance(&rows[start], start, limit / 3);
    let render = |context: &[String], reference: &[String], fresh: &[String]| {
        format!(
            "{SYSTEM}\n{{\"context\":[{}],\"reference\":[{}],\"new\":[{}]}}",
            context.join(","),
            reference.join(","),
            fresh.join(",")
        )
    };
    let first_only = std::slice::from_ref(&first);
    while chars(&render(&[], &reference, first_only)) > limit && !reference.is_empty() {
        reference.remove(0);
    }
    for index in &mut priority {
        let node = &graph.nodes[index];
        if !node.active() || !seen.insert(index) {
            continue;
        }
        let line =
            json!({"nid": node.id, "kind": node.kind, "text": node.text, "status": node.status})
                .to_string();
        let cost = chars(&line) + usize::from(!context.is_empty());
        if cost > budget {
            break;
        }
        context.push(line);
        if chars(&render(&context, &reference, first_only)) > limit {
            context.pop();
            break;
        }
        budget -= cost;
    }
    for index in pending {
        fresh.push(utterance(&rows[index], index, limit / 3));
        if chars(&render(&context, &reference, &fresh)) > limit {
            fresh.pop();
            break;
        }
    }
    let consumed = fresh.len();
    let grounds = (start - reference.len()..start + consumed)
        .map(|index| (format!("u{}", index + 1), rows[index].row_id))
        .collect();
    let prompt = if consumed == 0 {
        render(&context, &reference, first_only)
    } else {
        render(&context, &reference, &fresh)
    };
    Input {
        prompt,
        consumed,
        grounds,
    }
}

#[derive(Deserialize)]
struct Response {
    ops: Vec<Value>,
}

pub fn parse(text: &str) -> Result<Vec<Value>, &'static str> {
    let start = text.find('{').ok_or("object_missing")?;
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
    let value = stream
        .next()
        .ok_or("object_missing")?
        .map_err(|_| "invalid_json")?;
    serde_json::from_value::<Response>(value)
        .map(|response| response.ops)
        .map_err(|_| "invalid_schema")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prose_and_fences() {
        assert_eq!(
            parse("前置き\n```json\n{\"ops\":[{\"op\":\"merge\"}]}\n```\n後書き")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(parse("前置き {\"ops\": [}"), Err("invalid_json"));
        assert_eq!(parse("{}"), Err("invalid_schema"));
    }

    #[test]
    fn windows_flush_on_first_trigger_and_at_end() {
        let rows: Vec<_> = [0, 1, 2, 3, 14, 15, 16]
            .into_iter()
            .enumerate()
            .map(|(index, t)| Row {
                row_id: index as u64 + 1,
                speaker_label: "自分".into(),
                start_ms: t,
                end_ms: t,
                text: "発話".into(),
            })
            .collect();
        assert_eq!(windows(&rows, 3, 10), [3, 4, 7]);
        assert_eq!(windows(&rows, 8, 10), [4, 7]);
    }
}
