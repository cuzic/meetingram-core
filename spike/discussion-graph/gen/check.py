import json, sys, collections
D = "/home/cuzic/src/meetingram-core-spike/spike/discussion-graph"
RULES = {
    "responds_to": ({"position"}, {"issue"}),
    "supports": ({"argument"}, {"position", "argument"}),
    "objects_to": ({"argument"}, {"position", "argument"}),
    "resolves": ({"decision"}, {"issue"}),
    "assigns": ({"action"}, {"decision", "issue"}),
}
ok = True
def err(m):
    global ok; ok = False; print("  ERROR:", m)
for m in ["m1", "m2", "m3"]:
    t = json.load(open(f"{D}/meetings/{m}.json", encoding="utf-8"))
    g = json.load(open(f"{D}/gold/{m}.gold.json", encoding="utf-8"))
    us = t["utterances"]
    ids = [u["row_id"] for u in us]
    print(f"== {m}: {t['title']} | {len(us)} utts, {sum(len(u['text']) for u in us)} chars, duration_min={t['duration_min']}, longest={max(len(u['text']) for u in us)}")
    if ids != list(range(1, len(us) + 1)): err("row_ids not contiguous")
    if not 150 <= len(us) <= 220: err("utterance count out of range")
    for u in us:
        if u["track"] == "self" and u["speaker_label"] != "自分": err(f"row {u['row_id']} label")
        if u["track"] == "remote" and not u["speaker_label"].startswith("相手"): err(f"row {u['row_id']} label")
    ts = [(u["start_ms"], u["end_ms"]) for u in us]
    if m == "m3":
        if any(a is not None or b is not None for a, b in ts): err("m3 must have null times")
    else:
        prev = -1
        for a, b in ts:
            if not (a < b and a >= prev): err("times not increasing"); break
            prev = b
    print("  speakers:", dict(collections.Counter(u["speaker_label"] for u in us)))
    nodes = {n["id"]: n for n in g["nodes"]}
    rows = set(ids)
    for n in g["nodes"]:
        if not n["grounds"]: err(f"{n['id']} no grounds")
        for r in n["grounds"]:
            if r not in rows: err(f"{n['id']} ground {r} missing")
        if n["status"] in ("decided", "resolved") and n["kind"] != "issue": err(f"{n['id']} status")
        if n["status"] == "retracted" and n["kind"] not in ("position", "argument"): err(f"{n['id']} status")
    for e in g["edges"]:
        if e["src"] not in nodes or e["dst"] not in nodes: err(f"dangling edge {e}"); continue
        s, d = RULES[e["kind"]]
        if nodes[e["src"]]["kind"] not in s or nodes[e["dst"]]["kind"] not in d: err(f"bad edge {e}")
    linked = {e["src"] for e in g["edges"]} | {e["dst"] for e in g["edges"]}
    iso = [n for n in nodes if n not in linked]
    if iso: print("  isolated nodes:", iso)
    print("  nodes:", len(nodes), dict(collections.Counter(n["kind"] for n in g["nodes"])))
    print("  status:", dict(collections.Counter(f"{n['kind']}:{n['status']}" for n in g["nodes"] if n["status"] != "open")))
    print("  edges:", len(g["edges"]), dict(collections.Counter(e["kind"] for e in g["edges"])))
    print("  last 4:", [(u["row_id"], u["speaker_label"], u["text"][:40]) for u in us[-4:]] if m == "m1" else "")
print("OK" if ok else "FAILED")
