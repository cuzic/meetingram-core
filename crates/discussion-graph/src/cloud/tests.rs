use super::*;
use crate::{Ground, Review};

#[derive(Default)]
struct Builder {
    graph: Graph,
    next_row: i64,
}

impl Builder {
    fn node(&mut self, nid: &str, kind: NodeKind, text: &str, rev: u64) -> &mut Self {
        let status = if kind == NodeKind::Decision {
            Status::Decided
        } else {
            Status::Open
        };
        self.graph.nodes.insert(
            nid.to_owned(),
            Node {
                nid: nid.to_owned(),
                kind,
                text: text.to_owned(),
                status,
                merged_into: None,
                confidence: 0.9,
                review: Review::Auto,
                created_rev: rev,
                updated_rev: rev,
            },
        );
        self
    }

    fn edge(&mut self, src: &str, dst: &str, kind: EdgeKind) -> &mut Self {
        let eid = format!("{src}>{dst}:{kind}");
        self.graph.edges.insert(
            eid.clone(),
            Edge {
                eid,
                src: src.to_owned(),
                dst: dst.to_owned(),
                kind,
                confidence: 0.9,
                review: Review::Auto,
                created_rev: 1,
            },
        );
        self
    }

    fn set_status(&mut self, nid: &str, status: Status) -> &mut Self {
        self.graph.nodes.get_mut(nid).unwrap().status = status;
        self
    }

    fn reject_edge(&mut self, src: &str, dst: &str, kind: EdgeKind) -> &mut Self {
        let edge = self
            .graph
            .edges
            .get_mut(&format!("{src}>{dst}:{kind}"))
            .unwrap();
        edge.review = Review::Rejected;
        self
    }

    fn say(&mut self, uid: &str, text: &str) -> &mut Self {
        self.next_row += 1;
        self.graph.utterances.insert(
            uid.to_owned(),
            Utterance {
                uid: uid.to_owned(),
                row_id: self.next_row,
                track: None,
                speaker_label: "Synthetic speaker".to_owned(),
                start_ms: None,
                end_ms: None,
                text: text.to_owned(),
                is_retranscribed: false,
            },
        );
        self
    }

    fn ground(&mut self, target: &str, uid: &str) -> &mut Self {
        self.graph.grounds.push(Ground {
            target: target.to_owned(),
            uid: uid.to_owned(),
            act: None,
        });
        self
    }

    fn issue_with(&mut self, issue: &str, positions: &[&str], rev: u64) -> &mut Self {
        self.node(issue, NodeKind::Issue, "合成の問い", rev);
        for position in positions {
            self.node(position, NodeKind::Position, "合成の案", rev)
                .edge(position, issue, EdgeKind::RespondsTo);
        }
        self
    }

    fn reason(&mut self, nid: &str, on: &str, kind: EdgeKind) -> &mut Self {
        self.node(nid, NodeKind::Argument, "合成の理由", 2)
            .edge(nid, on, kind)
    }
}

#[test]
fn single_position_issue_is_not_a_candidate_even_with_an_objection() {
    let mut b = Builder::default();
    b.issue_with("i1", &["p1"], 1)
        .reason("a1", "p1", EdgeKind::ObjectsTo);
    assert!(conflict_candidates(&b.graph).is_empty());
}

#[test]
fn two_live_positions_make_a_candidate_with_a_sorted_key() {
    let mut b = Builder::default();
    b.issue_with("i1", &["p9", "p2"], 1);
    let found = conflict_candidates(&b.graph);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].default_pair, ["p2", "p9"]);
    assert_eq!(found[0].subject(), "i1:p2:p9");
    assert_eq!(conflict_subject("i1", "p9", "p2"), "i1:p2:p9");
}

#[test]
fn rejected_edges_and_dead_nodes_are_not_counted() {
    for case in 0..6 {
        let mut b = Builder::default();
        b.issue_with("i1", &["p1", "p2", "p3"], 1);
        // Remove p3 and p2 so only p1 and one candidate position remain.
        match case {
            0 => {
                b.reject_edge("p2", "i1", EdgeKind::RespondsTo);
            }
            1 => {
                b.set_status("p2", Status::Retracted);
            }
            2 => {
                b.set_status("p2", Status::Merged);
            }
            3 => {
                b.graph.nodes.get_mut("p2").unwrap().review = Review::Rejected;
            }
            4 => {
                b.set_status("p2", Status::Retracted);
            }
            _ => {
                b.reject_edge("p2", "i1", EdgeKind::RespondsTo);
            }
        }
        b.reject_edge("p3", "i1", EdgeKind::RespondsTo);
        assert!(conflict_candidates(&b.graph).is_empty(), "case {case}");
    }
    let mut b = Builder::default();
    b.issue_with("i1", &["p1", "p2"], 1)
        .set_status("i1", Status::Merged);
    assert!(conflict_candidates(&b.graph).is_empty());
    let mut b = Builder::default();
    b.issue_with("i1", &["p1", "p2"], 1);
    b.graph.nodes.get_mut("i1").unwrap().review = Review::Rejected;
    assert!(conflict_candidates(&b.graph).is_empty());
}

#[test]
fn default_pair_ignores_rejected_and_retracted_reasons() {
    let mut b = Builder::default();
    b.issue_with("i1", &["p1", "p2", "p3"], 1)
        .reason("a1", "p3", EdgeKind::Supports)
        .reason("a2", "p3", EdgeKind::ObjectsTo)
        .reason("a3", "p2", EdgeKind::Supports)
        // Not counted: rejected edge and retracted argument.
        .reason("a4", "p1", EdgeKind::Supports)
        .reason("a5", "p1", EdgeKind::Supports)
        .reject_edge("a4", "p1", EdgeKind::Supports)
        .set_status("a5", Status::Retracted);
    let found = conflict_candidates(&b.graph);
    assert_eq!(found[0].default_pair, ["p2", "p3"]);
    assert_eq!(found[0].reasons, 3);
    assert_eq!(found[0].positions, ["p1", "p2", "p3"]);
}

#[test]
fn default_pair_ties_prefer_older_positions() {
    let mut b = Builder::default();
    b.issue_with("i1", &["p1", "p2", "p3"], 1);
    b.graph.nodes.get_mut("p1").unwrap().created_rev = 5;
    b.graph.nodes.get_mut("p2").unwrap().created_rev = 4;
    b.graph.nodes.get_mut("p3").unwrap().created_rev = 3;
    assert_eq!(conflict_candidates(&b.graph)[0].default_pair, ["p2", "p3"]);
}

#[test]
fn candidates_are_ordered_by_decision_objection_reasons_and_age() {
    let mut b = Builder::default();
    b.issue_with("old", &["o1", "o2"], 1)
        .issue_with("decided", &["d1", "d2"], 1)
        .node("dec", NodeKind::Decision, "合成の決定", 3)
        .edge("dec", "decided", EdgeKind::Resolves)
        .issue_with("objected", &["b1", "b2"], 9)
        .reason("x1", "b1", EdgeKind::ObjectsTo)
        .issue_with("busy", &["c1", "c2"], 8)
        .reason("y1", "c1", EdgeKind::Supports)
        .reason("y2", "c2", EdgeKind::Supports)
        .reason("y3", "c2", EdgeKind::Supports)
        .issue_with("young", &["e1", "e2"], 10);
    let order: Vec<_> = conflict_candidates(&b.graph)
        .into_iter()
        .map(|candidate| candidate.issue)
        .collect();
    assert_eq!(order, ["objected", "busy", "old", "young", "decided"]);
}

#[test]
fn a_retracted_decision_does_not_make_an_issue_decided() {
    let mut b = Builder::default();
    b.issue_with("i1", &["p1", "p2"], 1)
        .node("dec", NodeKind::Decision, "合成の決定", 3)
        .edge("dec", "i1", EdgeKind::Resolves)
        .set_status("dec", Status::Retracted);
    assert!(!conflict_candidates(&b.graph)[0].decided);
}

fn rich_graph() -> Builder {
    let mut b = Builder::default();
    let long = "あ".repeat(300);
    b.issue_with("i1", &["p2", "p1"], 3)
        .say("u1", &format!("案一の発言{long}"))
        .say("u2", &format!("案一の別の発言{long}"))
        .say("u3", &format!("案二の発言{long}"))
        .say("u4", &format!("理由の発言{long}"))
        .say("u5", &format!("二段目の発言{long}"))
        .say("u6", &format!("決定の発言{long}"))
        .say("u7", "どの題材にも関係しない発言")
        .ground("i1", "u1")
        .ground("p1", "u1")
        .ground("p1", "u2")
        .ground("p2", "u3")
        .ground("a1", "u4")
        .ground("a1", "u2")
        .ground("a2", "u5")
        .ground("dec", "u6")
        .ground("act", "u6")
        .reason("a1", "p1", EdgeKind::Supports)
        .reason("a2", "a1", EdgeKind::ObjectsTo)
        .node("a3", NodeKind::Argument, "三段目の理由", 2)
        .edge("a3", "a2", EdgeKind::Supports)
        .node("dec", NodeKind::Decision, "合成の決定", 4)
        .edge("dec", "i1", EdgeKind::Resolves)
        .node("act", NodeKind::Action, "合成の宿題", 5)
        .edge("act", "dec", EdgeKind::Assigns);
    for (index, rev) in [7, 3, 4, 5, 6, 8, 9].into_iter().enumerate() {
        let nid = format!("g{index}");
        b.node(
            &nid,
            NodeKind::Issue,
            &format!("{index}番目の別の議題{}", "い".repeat(80)),
            rev,
        );
    }
    // A sub-issue is not an agenda item.
    b.node("sub", NodeKind::Issue, "子の問い", 1)
        .edge("sub", "g1", EdgeKind::SubIssueOf);
    b
}

#[test]
fn input_has_the_subject_and_only_related_utterances() {
    let b = rich_graph();
    let prompt = build_conflict_cloud_default(&b.graph, "i1", ["p2", "p1"]).unwrap();
    assert_eq!(prompt.system, CLOUD_SYSTEM);
    assert_eq!(prompt.subject, "i1:p1:p2");
    assert_eq!(prompt.stage, CloudInputStage::Full);
    // D is the smaller nid, whatever order the caller passed.
    let user = &prompt.user;
    let d_line = user.lines().find(|l| l.contains(r#""slot":"d""#)).unwrap();
    assert!(d_line.contains(r#""nid":"p1""#));
    let d2_line = user.lines().find(|l| l.contains(r#""slot":"d2""#)).unwrap();
    assert!(d2_line.contains(r#""nid":"p2""#));
    assert_eq!(prompt.anchors[0].nid, "p1");
    assert_eq!(prompt.anchors[0].grounds, ["u1", "u2"]);
    assert_eq!(prompt.anchors[1].nid, "p2");
    for uid in ["u1", "u2", "u3", "u4", "u5", "u6"] {
        assert!(prompt.allowed_uids.contains(&uid.to_owned()), "{uid}");
    }
    assert!(!prompt.allowed_uids.contains(&"u7".to_owned()));
    assert!(!user.contains("どの題材にも関係しない"));
    // Decision, action and both reason levels are present; the third level is not.
    for nid in ["dec", "act", "a1", "a2"] {
        assert!(prompt.allowed_nids.contains(&nid.to_owned()), "{nid}");
    }
    assert!(!prompt.allowed_nids.contains(&"a3".to_owned()));
    // Utterances are cut at 200 characters.
    let line = user
        .lines()
        .find(|l| l.contains(r#""uid":"u1""#) && l.contains("speaker_label"))
        .unwrap();
    assert!(line.contains(&"あ".repeat(100)) && !line.contains(&"あ".repeat(200)));
}

#[test]
fn other_agenda_issues_are_the_oldest_five_cut_to_sixty_characters() {
    let b = rich_graph();
    let prompt = build_conflict_cloud_default(&b.graph, "i1", ["p1", "p2"]).unwrap();
    let section: Vec<_> = prompt
        .user
        .lines()
        .skip_while(|l| *l != "other_issues")
        .skip(1)
        .take_while(|l| *l != "positions")
        .collect();
    let nids: Vec<_> = section
        .iter()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["nid"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    // g1 (rev 3), g2 (4), g3 (5), g4 (6), g0 (7). The sub-issue and the subject are excluded.
    assert_eq!(nids, ["g1", "g2", "g3", "g4", "g0"]);
    let text = serde_json::from_str::<Value>(section[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(text.chars().count(), 60);
}

#[test]
fn invalid_subjects_are_rejected() {
    let b = rich_graph();
    for (issue, pair) in [
        ("nope", ["p1", "p2"]),
        ("i1", ["p1", "p1"]),
        ("i1", ["p1", "a1"]),
        ("i1", ["p1", "nope"]),
        ("g1", ["p1", "p2"]),
    ] {
        assert_eq!(
            build_conflict_cloud_default(&b.graph, issue, pair).unwrap_err(),
            CloudInputError::InvalidSubject,
            "{issue} {pair:?}"
        );
    }
    let mut b = rich_graph();
    b.set_status("p2", Status::Retracted);
    assert_eq!(
        build_conflict_cloud_default(&b.graph, "i1", ["p1", "p2"]).unwrap_err(),
        CloudInputError::InvalidSubject
    );
}

#[test]
fn input_is_reduced_in_the_adr_order_and_then_refused() {
    let b = rich_graph();
    let full = build_conflict_cloud_default(&b.graph, "i1", ["p1", "p2"]).unwrap();
    let full_len = chars(&full.system) + chars(&full.user);
    let mut stages = Vec::new();
    let mut previous: Option<CloudPrompt> = None;
    for limit in (0..=full_len).rev() {
        let Ok(prompt) = build_conflict_cloud(&b.graph, "i1", ["p1", "p2"], limit) else {
            break;
        };
        assert!(chars(&prompt.system) + chars(&prompt.user) <= limit);
        if stages.last() != Some(&prompt.stage) {
            stages.push(prompt.stage);
            if let Some(before) = &previous {
                // Each step only removes input.
                assert!(prompt.user.len() < before.user.len());
            }
        }
        previous = Some(prompt);
    }
    assert_eq!(stages, STAGES);
    let last = previous.unwrap();
    assert_eq!(last.stage, CloudInputStage::NoSecondLevelReasons);
    // The smallest input still has evidence for every node and keeps node text.
    assert!(!last.allowed_nids.contains(&"a2".to_owned()));
    assert!(!last.allowed_uids.contains(&"u5".to_owned()));
    assert!(last.user.contains("合成の宿題") && last.user.contains("合成の理由"));
    assert!(!last.user.contains("0番目の別の議題"));
    assert!(last.allowed_uids.contains(&"u1".to_owned()));
    assert!(last.allowed_uids.contains(&"u3".to_owned()));
    for line in last.user.lines().filter(|l| l.contains(r#""uids":"#)) {
        assert!(!line.contains(r#""uids":[]"#), "{line}");
    }
    let tiny = chars(&last.system) + chars(&last.user) - 1;
    assert_eq!(
        build_conflict_cloud(&b.graph, "i1", ["p1", "p2"], tiny).unwrap_err(),
        CloudInputError::TooLarge
    );
}

#[test]
fn stage_steps_match_the_documented_order() {
    let b = rich_graph();
    let at = |stage| {
        let m = Material::collect(&b.graph, "i1", ["p1", "p2"]).unwrap();
        m.render(stage)
    };
    let (full, full_uids, _) = at(CloudInputStage::Full);
    let (one, one_uids, _) = at(CloudInputStage::OneUtterancePerNode);
    let (short, _, _) = at(CloudInputStage::ShortUtterances);
    let (no_others, _, no_others_nids) = at(CloudInputStage::NoOtherIssues);
    let (no_second, _, nids) = at(CloudInputStage::NoSecondLevelReasons);
    // u2 grounds p1 and a1; with one per node, a1 keeps u2 (its oldest) and p1 keeps u1.
    assert!(full_uids.contains("u4") && !one_uids.contains("u4"));
    assert!(one.contains(&"あ".repeat(150)) && !short.contains(&"あ".repeat(150)));
    assert!(short.contains("0番目の別の議題") && !no_others.contains("0番目の別の議題"));
    assert!(no_others_nids.contains("a2") && no_others.contains("二段目の発言"));
    assert!(!nids.contains("a2") && !no_second.contains("二段目の発言"));
    assert!(full.len() > one.len());
}

fn small_prompt() -> CloudPrompt {
    let mut b = Builder::default();
    b.issue_with("i1", &["p1", "p2"], 1)
        .say("u1", "合成の発言一")
        .say("u2", "合成の発言二")
        .say("u3", "合成の発言三")
        .ground("p1", "u1")
        .ground("p1", "u3")
        .ground("p2", "u2");
    b.graph.nodes.get_mut("p1").unwrap().text = "案一の本文".to_owned();
    b.graph.nodes.get_mut("p2").unwrap().text = "あ".repeat(130);
    build_conflict_cloud_default(&b.graph, "i1", ["p1", "p2"]).unwrap()
}

fn element(text: &str) -> Value {
    json!({"text": text, "basis": "inferred"})
}

fn response(extra: Value) -> String {
    let mut body = json!({"a": element("目的"), "b": element("要望一"), "c": element("要望二")});
    for (key, value) in extra.as_object().unwrap() {
        body[key] = value.clone();
    }
    body.to_string()
}

#[test]
fn a_clean_response_is_kept() {
    let prompt = small_prompt();
    let text = format!(
        "説明です。{}",
        response(json!({
            "b": {"text": "要望一", "basis": "stated", "grounds": ["u1"], "nids": ["p1"]},
            "d": {"text": "案一の言い換え"},
            "d2": {"text": "案二の言い換え"},
            "assumptions": [{"on": "bd", "text": "前提", "basis": "stated", "grounds": ["u2"],
                "injection": {"text": "打ち手", "basis": "inferred"}}],
        }))
    );
    let result = validate_cloud(&text, &prompt).unwrap();
    assert_eq!((result.dropped, result.downgraded), (0, 0));
    let d = &result.diagram;
    assert_eq!(d.b.basis, Basis::Stated);
    assert_eq!(d.b.grounds, ["u1"]);
    assert_eq!(d.b.nids, ["p1"]);
    assert_eq!(d.a.basis, Basis::Inferred);
    assert_eq!(d.d.text, "案一の言い換え");
    assert_eq!(d.assumptions[0].on, AssumptionOn::Bd);
    assert_eq!(d.assumptions[0].injection.as_ref().unwrap().text, "打ち手");
    let json = serde_json::to_string(d).unwrap();
    assert_eq!(serde_json::from_str::<CloudDiagram>(&json).unwrap(), *d);
}

#[test]
fn stated_with_only_nids_is_downgraded() {
    let prompt = small_prompt();
    let text = response(json!({
        "a": {"text": "目的", "basis": "stated", "nids": ["p1"]},
        "b": {"text": "要望", "basis": "stated", "grounds": ["u99"], "nids": ["n99", "p2"]},
        "assumptions": [{"on": "ab", "text": "前提", "basis": "stated", "nids": ["i1"]}],
    }));
    let result = validate_cloud(&text, &prompt).unwrap();
    let d = &result.diagram;
    assert_eq!(d.a.basis, Basis::Inferred);
    assert_eq!(d.a.nids, ["p1"]);
    assert_eq!(d.b.basis, Basis::Inferred);
    assert!(d.b.grounds.is_empty());
    assert_eq!(d.b.nids, ["p2"]);
    assert_eq!(d.assumptions[0].basis, Basis::Inferred);
    assert_eq!(result.downgraded, 3);
    // The unknown uid and the unknown nid.
    assert_eq!(result.dropped, 2);
}

#[test]
fn unknown_basis_becomes_inferred_without_a_downgrade() {
    let prompt = small_prompt();
    let text = response(json!({"a": {"text": "目的", "basis": "certain", "grounds": ["u1"]}}));
    let result = validate_cloud(&text, &prompt).unwrap();
    assert_eq!(result.diagram.a.basis, Basis::Inferred);
    assert_eq!(result.downgraded, 0);
}

#[test]
fn text_is_cut_at_116_utf16_units_without_splitting_a_pair() {
    assert_eq!(truncate_utf16(&"あ".repeat(200), 116).chars().count(), 116);
    let mixed = format!("{}😀{}", "あ".repeat(115), "い");
    // 115 units then a 2-unit emoji would reach 117, so the emoji is dropped whole.
    assert_eq!(truncate_utf16(&mixed, 116), "あ".repeat(115));
    let fits = format!("{}😀", "あ".repeat(114));
    assert_eq!(truncate_utf16(&fits, 116), fits);
    assert_eq!(truncate_utf16("短い", 116), "短い");

    let prompt = small_prompt();
    let text = response(json!({
        "a": element(&"あ".repeat(200)),
        "b": element(&mixed),
        "assumptions": [{"on": "dd", "text": mixed,
            "injection": {"text": "😀".repeat(100), "basis": "inferred"}}],
    }));
    let result = validate_cloud(&text, &prompt).unwrap();
    let d = &result.diagram;
    assert_eq!(d.a.text.encode_utf16().count(), 116);
    assert_eq!(d.b.text, "あ".repeat(115));
    assert_eq!(d.assumptions[0].text, "あ".repeat(115));
    assert_eq!(
        d.assumptions[0].injection.as_ref().unwrap().text,
        "😀".repeat(58)
    );
}

#[test]
fn assumption_rows_are_dropped_by_the_table_rules() {
    let prompt = small_prompt();
    let item = |on: &str, text: &str| json!({"on": on, "text": text});
    let text = response(json!({"assumptions": [
        item("xx", "不正な矢印"),
        item("ab", ""),
        item("ab", "一"), item("ab", "二"), item("ab", "三は落ちる"),
        item("ac", "四"), item("ac", "五"),
        item("bd", "六"), item("bd", "七"),
        item("cd", "八"), item("cd", "九"),
        item("dd", "十は上限で落ちる"),
    ]}));
    let result = validate_cloud(&text, &prompt).unwrap();
    let kept: Vec<_> = result
        .diagram
        .assumptions
        .iter()
        .map(|a| a.text.as_str())
        .collect();
    // The empty and malformed rows do not use up the quota.
    assert_eq!(kept, ["一", "二", "四", "五", "六", "七", "八", "九"]);
    assert_eq!(result.dropped, 4);
}

#[test]
fn an_empty_injection_is_dropped_alone() {
    let prompt = small_prompt();
    let text = response(json!({"assumptions": [
        {"on": "ab", "text": "前提", "injection": {"text": "  "}},
        {"on": "ac", "text": "前提二", "injection": null},
    ]}));
    let result = validate_cloud(&text, &prompt).unwrap();
    assert_eq!(result.diagram.assumptions.len(), 2);
    assert!(result.diagram.assumptions[0].injection.is_none());
    assert_eq!(result.dropped, 1);
}

#[test]
fn the_whole_response_fails_when_a_b_or_c_is_unusable() {
    let prompt = small_prompt();
    for text in ["", "説明だけです", "[1, 2]", r#"{"a": "#] {
        assert_eq!(
            validate_cloud(text, &prompt).unwrap_err(),
            CloudParseError::NotJson,
            "{text}"
        );
    }
    for key in ["a", "b", "c"] {
        let mut missing: Value = serde_json::from_str(&response(json!({}))).unwrap();
        missing.as_object_mut().unwrap().remove(key);
        let mut empty = missing.clone();
        empty[key] = json!({"text": "   "});
        for body in [missing, empty] {
            assert_eq!(
                validate_cloud(&body.to_string(), &prompt).unwrap_err(),
                CloudParseError::MissingElement
            );
        }
    }
}

#[test]
fn d_and_d2_are_filled_from_the_positions_and_always_stated() {
    let prompt = small_prompt();
    let result = validate_cloud(&response(json!({})), &prompt).unwrap();
    let d = &result.diagram;
    assert_eq!(d.d.text, "案一の本文");
    assert_eq!(
        (d.d.basis, d.d.grounds.clone(), d.d.nids.clone()),
        (
            Basis::Stated,
            vec!["u1".into(), "u3".into()],
            vec!["p1".into()]
        )
    );
    // The second position is longer than 116 units, so its text is cut.
    assert_eq!(d.d2.text, "あ".repeat(116));
    assert_eq!(d.d2.basis, Basis::Stated);
    assert_eq!(d.d2.grounds, ["u2"]);
    assert_eq!(result.dropped, 0);
}

#[test]
fn a_model_supplied_basis_and_grounds_for_d_are_overwritten() {
    let prompt = small_prompt();
    let text = response(json!({
        "d": {"text": "x".repeat(300), "basis": "inferred", "grounds": ["u2"], "nids": ["i1"]},
        "d2": {"text": ""},
    }));
    let result = validate_cloud(&text, &prompt).unwrap();
    assert_eq!(result.diagram.d.text, "x".repeat(116));
    assert_eq!(result.diagram.d.basis, Basis::Stated);
    assert_eq!(result.diagram.d.grounds, ["u1", "u3"]);
    assert_eq!(result.diagram.d.nids, ["p1"]);
    assert_eq!(result.diagram.d2.text, "あ".repeat(116));
    assert_eq!((result.dropped, result.downgraded), (0, 0));
}

#[test]
fn errors_and_prompts_do_not_carry_meeting_text() {
    assert!(!CLOUD_SYSTEM.contains("合成"));
    for error in [CloudInputError::InvalidSubject, CloudInputError::TooLarge] {
        assert!(!error.to_string().contains("合成"));
    }
    let prompt = small_prompt();
    let error = validate_cloud("合成の本文だけ", &prompt).unwrap_err();
    assert!(!format!("{error} {error:?}").contains("合成"));
}

#[test]
fn an_action_assigned_directly_to_the_issue_is_in_the_input_only_when_decided() {
    let mut b = rich_graph();
    b.node("act2", NodeKind::Action, "課題に直接付く宿題", 6)
        .edge("act2", "i1", EdgeKind::Assigns)
        .say("u8", "宿題の発言")
        .ground("act2", "u8");
    let prompt = build_conflict_cloud_default(&b.graph, "i1", ["p1", "p2"]).unwrap();
    assert!(prompt.user.contains("課題に直接付く宿題"));
    assert!(prompt.user.contains("合成の宿題"));
    assert!(prompt.allowed_nids.contains(&"act2".to_owned()));
    assert!(prompt.allowed_uids.contains(&"u8".to_owned()));

    // Not decided: no actions at all.
    b.set_status("dec", Status::Retracted);
    let prompt = build_conflict_cloud_default(&b.graph, "i1", ["p1", "p2"]).unwrap();
    assert!(!prompt.user.contains("課題に直接付く宿題"));
}

#[test]
fn a_reason_on_both_levels_is_listed_once_at_the_first_level() {
    let mut b = rich_graph();
    // a1 supports p1 (level 1). a2 attaches to a1 (level 2) and also directly
    // to p2, so it is level 1 and must not appear again at level 2.
    b.edge("a2", "p2", EdgeKind::Supports)
        // a4 attaches to two level-1 reasons and appears once at level 2.
        .reason("a4", "a1", EdgeKind::Supports)
        .edge("a4", "a2", EdgeKind::ObjectsTo);
    let material = Material::collect(&b.graph, "i1", ["p1", "p2"]).unwrap();
    let rows: Vec<_> = material
        .reasons
        .iter()
        .map(|r| (r.node.nid.as_str(), r.on, r.second_level))
        .collect();
    assert_eq!(
        rows,
        [
            ("a1", "p1", false),
            ("a2", "p2", false),
            ("a3", "a2", true),
            ("a4", "a1", true)
        ]
    );
    // Dropping the second level keeps the first-level rows.
    let (user, _, nids) = material.render(CloudInputStage::NoSecondLevelReasons);
    assert!(nids.contains("a1") && nids.contains("a2") && !nids.contains("a4"));
    assert!(user.contains(r#""nid":"a2""#));
}

#[test]
fn assumptions_dropped_by_the_quota_add_one_to_dropped_and_nothing_else() {
    let prompt = small_prompt();
    let stated = |text: &str| json!({"on": "ab", "text": text, "basis": "stated", "grounds": ["u99"], "nids": ["n99"]});
    let text = response(json!({"assumptions": [stated("一"), stated("二"), stated("三")]}));
    let result = validate_cloud(&text, &prompt).unwrap();
    assert_eq!(result.diagram.assumptions.len(), 2);
    assert_eq!(result.downgraded, 2);
    // Two kept assumptions lose one ground and one node ID each; the third counts once.
    assert_eq!(result.dropped, 2 * 2 + 1);
}

#[test]
fn the_system_prompt_states_the_arrows_and_the_stated_rule() {
    for arrow in [
        "ab は「a を満たすには b が要る」",
        "ac は「a を満たすには c が要る」",
        "bd は「b を満たすには d が要る」",
        "cd は「c を満たすには d2 が要る」",
    ] {
        assert!(CLOUD_SYSTEM.contains(arrow), "{arrow}");
    }
    assert!(CLOUD_SYSTEM.contains("grounds に入れて stated にする"));
}
