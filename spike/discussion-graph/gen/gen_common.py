import json, math, random, sys

LABELS = {
    "S": ("self", "自分"),
    "R": ("remote", "相手"),
    "1": ("remote", "相手 (話者1)"),
    "2": ("remote", "相手 (話者2)"),
    "3": ("remote", "相手 (話者3)"),
}

OUT = "/home/cuzic/src/meetingram-core-spike/spike/discussion-graph"


def build(name, title, script, nodes, edges, timestamps=True, seed=1):
    rnd = random.Random(seed)
    utts = []
    grounds = {nid: [] for nid in nodes}
    t = 1200
    prev = None
    for i, line in enumerate(l for l in script.strip().splitlines() if l.strip()):
        parts = line.split("|")
        if len(parts) != 3:
            sys.exit(f"bad line: {line!r}")
        spk, text, tags = parts[0].strip(), parts[1].strip(), parts[2].split()
        rid = i + 1
        track, label = LABELS[spk]
        dur = int(len(text) / rnd.uniform(6.0, 8.0) * 1000) + rnd.randint(150, 400)
        dur = max(dur, 450)
        gap = rnd.randint(80, 400) if spk == prev else rnd.randint(200, 1100)
        start = t + gap
        end = start + dur
        t = end
        prev = spk
        utts.append({
            "row_id": rid, "track": track, "speaker_label": label,
            "start_ms": start if timestamps else None,
            "end_ms": end if timestamps else None,
            "text": text,
        })
        for tag in tags:
            if tag not in grounds:
                sys.exit(f"row {rid}: unknown tag {tag}")
            grounds[tag].append(rid)
    duration_min = math.ceil(t / 60000)
    gnodes = []
    for nid, (kind, text, status) in nodes.items():
        if not grounds[nid]:
            sys.exit(f"node {nid} has no grounds")
        gnodes.append({"id": nid, "kind": kind, "text": text, "status": status, "grounds": grounds[nid]})
    gedges = [{"src": s, "dst": d, "kind": k} for s, d, k in edges]
    with open(f"{OUT}/meetings/{name}.json", "w", encoding="utf-8") as f:
        json.dump({"title": title, "duration_min": duration_min, "utterances": utts}, f, ensure_ascii=False, indent=1)
        f.write("\n")
    with open(f"{OUT}/gold/{name}.gold.json", "w", encoding="utf-8") as f:
        json.dump({"nodes": gnodes, "edges": gedges}, f, ensure_ascii=False, indent=1)
        f.write("\n")
    print(name, len(utts), "utts", sum(len(u["text"]) for u in utts), "chars", duration_min, "min")
