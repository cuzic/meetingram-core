use discussion_graph::{Node, NodeKind, Review, Status, Utterance};

pub fn node(nid: &str, kind: NodeKind) -> Node {
    Node {
        nid: nid.into(),
        kind,
        text: format!("Synthetic {nid}"),
        status: Status::Open,
        merged_into: None,
        confidence: 1.0,
        review: Review::Auto,
        created_rev: 1,
        updated_rev: 1,
    }
}

pub fn utterance(row_id: i64, text: String) -> Utterance {
    Utterance {
        uid: format!("u{row_id}"),
        row_id,
        track: None,
        speaker_label: "A".into(),
        start_ms: None,
        end_ms: None,
        text,
    }
}
