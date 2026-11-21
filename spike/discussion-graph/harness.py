#!/usr/bin/env python3
"""ADR 0001 (meetingram) spike S1: incremental discussion-graph extraction harness.

Throwaway. Mirrors the ADR r4 design: batching (6 utterances / 45 s), context budget,
op validation rules table, flush + consolidate, and gold-based metrics.

Usage:
  python3 -I harness.py run --meeting meetings/m1.json --backend claude --model sonnet --out results/x.json
  python3 -I harness.py eval --result results/x.json --gold gold/m1.gold.json
"""
import argparse, json, os, re, subprocess, sys, tempfile, time

LIMIT = 24000
KINDS = {"issue", "position", "argument", "decision", "action"}
EDGE_RULES = {
    "responds_to": ({"position"}, {"issue"}),
    "supports": ({"argument"}, {"position", "argument"}),
    "objects_to": ({"argument"}, {"position", "argument"}),
    "resolves": ({"decision"}, {"issue"}),
    "assigns": ({"action"}, {"decision", "issue"}),
}
ACTS = {"asserts", "asks", "challenges", "agrees", "retracts", "rephrases", None}
STATUS_KIND = {"decided": {"issue"}, "resolved": {"issue"}, "retracted": {"position", "argument"}, "open": KINDS}

SYSTEM = """あなたは会議の議論構造を IBIS 形式のグラフとして記録する書記です。
入力として、既存のグラフの一部（文脈）と、新しく確定した発話を受け取ります。
グラフを更新する操作の列を JSON で 1 つだけ返してください。説明文は書かないでください。

## ノードの種類 kind
- issue: 問い・論点（「〜をどうするか」）
- position: 問いに対する案・立場
- argument: 案や他の理由に対する賛成・反対の理由
- decision: 問いに対して決まったこと
- action: 誰かがやることになった宿題

## エッジの種類 kind（向きは src → dst）
- responds_to: position → issue
- supports / objects_to: argument → position または argument
- resolves: decision → issue
- assigns: action → decision または issue

## 操作
{"ops": [
 {"op":"add_node","tmp":"a","kind":"issue","text":"短く言い直した命題","confidence":0.0〜1.0,"grounds":[{"uid":"u12","act":"asks"}]},
 {"op":"add_edge","src":"a または n3","dst":"…","kind":"responds_to","confidence":0.7,"grounds":[{"uid":"u12"}]},
 {"op":"merge","from":"n9","into":"n5","grounds":[{"uid":"u13","act":"rephrases"}]},
 {"op":"set_status","nid":"n2","status":"decided|resolved|retracted|open","grounds":[{"uid":"u14"}]}
]}
- tmp はこの応答の中だけの仮の名前。src/dst/from/into/nid は tmp か既存の nid を指す。
- act は asserts / asks / challenges / agrees / retracts / rephrases のどれか（省略可）。
- grounds の uid は、入力に含まれる発話（新しい発話か参照用の発話）だけ。根拠の無い操作は出さない。

## 規則
- 相づち・言いよどみ・挨拶だけの発話からノードを作らない。
- 既存のノードと同じ内容の言い直しは、新しいノードを作らず、既存の nid に grounds を足す代わりに何もしないか、
  新旧が両方とも既存のノードなら merge する。
- 推測で decision や action を作らない。発話で明示されたものだけ。
- decided / resolved は issue だけ、retracted は position と argument だけ。
- 更新が無ければ {"ops": []} を返す。
"""

CONSOLIDATE_SYSTEM = SYSTEM + """
## 今回の仕事（会議の後の整理）
会議は終わりました。入力はグラフ全体と、各ノードの根拠の発話です。
次のことだけを操作で返してください: 同じ内容のノードの merge、抜けているエッジの add_edge、
状態の訂正の set_status。新しいノードは原則として作らない。grounds には各ノードの根拠の発話の uid を使う。
"""


# ---------------------------------------------------------------- graph state
class Graph:
    def __init__(self):
        self.utt = []            # list of dicts with uid,row_id,speaker_label,text,start_ms,extracted_rev
        self.nodes = {}          # nid -> dict(kind,text,status,merged_into,confidence,review,grounds:set)
        self.edges = {}          # eid -> dict(src,dst,kind,confidence,review,grounds:set)
        self.rev = 0
        self.n_seq = 0
        self.e_seq = 0
        self.touched = []        # list of sets of nids per rev
        self.rejected = {}       # rule -> count

    def reject(self, rule):
        self.rejected[rule] = self.rejected.get(rule, 0) + 1

    def live(self, nid):
        n = self.nodes.get(nid)
        return n is not None and n["status"] != "merged" and n["review"] != "rejected"

    def apply(self, ops, allowed_uids):
        """Validate sequentially against the state after previous ops (ADR §2). Returns applied count."""
        self.rev += 1
        tmp = {}
        dropped_tmp = set()
        applied = 0
        touched = set()

        def resolve(ref):
            if ref in tmp:
                return tmp[ref]
            if ref in dropped_tmp:
                return None
            return ref if ref in self.nodes else None

        for op in ops if isinstance(ops, list) else []:
            if not isinstance(op, dict):
                self.reject("not_object"); continue
            kind_op = op.get("op")
            grounds = op.get("grounds") or []
            g_ok = isinstance(grounds, list) and grounds and all(
                isinstance(g, dict) and g.get("uid") in allowed_uids and g.get("act") in ACTS for g in grounds)
            if not g_ok:
                if kind_op == "add_node" and op.get("tmp"):
                    dropped_tmp.add(op["tmp"])
                self.reject("grounds"); continue
            conf = op.get("confidence", 1.0)
            if not isinstance(conf, (int, float)) or not 0 <= conf <= 1:
                if kind_op == "add_node" and op.get("tmp"):
                    dropped_tmp.add(op["tmp"])
                self.reject("confidence"); continue
            gset = {g["uid"] for g in grounds}

            if kind_op == "add_node":
                t = op.get("tmp")
                if op.get("kind") not in KINDS or not (op.get("text") or "").strip() or not t or t in tmp:
                    if t: dropped_tmp.add(t)
                    self.reject("add_node"); continue
                self.n_seq += 1
                nid = f"n{self.n_seq}"
                self.nodes[nid] = dict(kind=op["kind"], text=op["text"].strip(), status="open", merged_into=None,
                                       confidence=float(conf), review="auto", grounds=set(gset), rev=self.rev)
                tmp[t] = nid; touched.add(nid); applied += 1
            elif kind_op == "add_edge":
                s, d, k = resolve(op.get("src")), resolve(op.get("dst")), op.get("kind")
                if s is None or d is None:
                    self.reject("edge_missing_endpoint"); continue
                if not (self.live(s) and self.live(d)):
                    self.reject("edge_dead_endpoint"); continue
                if k not in EDGE_RULES or self.nodes[s]["kind"] not in EDGE_RULES[k][0] or self.nodes[d]["kind"] not in EDGE_RULES[k][1]:
                    self.reject("edge_kind"); continue
                if s == d:
                    self.reject("edge_self_loop"); continue
                if any(e["src"] == s and e["dst"] == d and e["kind"] == k for e in self.edges.values()):
                    self.reject("edge_duplicate"); continue
                self.e_seq += 1
                self.edges[f"e{self.e_seq}"] = dict(src=s, dst=d, kind=k, confidence=float(conf), review="auto", grounds=set(gset))
                touched.update([s, d]); applied += 1
            elif kind_op == "merge":
                f, i = resolve(op.get("from")), resolve(op.get("into"))
                if f is None or i is None or f == i:
                    self.reject("merge_ref"); continue
                nf, ni = self.nodes[f], self.nodes[i]
                if nf["status"] == "merged" or ni["status"] == "merged" or ni["review"] == "rejected":
                    self.reject("merge_state"); continue
                if nf["kind"] != ni["kind"]:
                    self.reject("merge_kind"); continue
                if nf["review"] != "auto" or any(e["review"] != "auto" and f in (e["src"], e["dst"]) for e in self.edges.values()):
                    self.reject("merge_human"); continue
                nf["status"] = "merged"; nf["merged_into"] = i
                ni["grounds"] |= nf["grounds"] | gset
                for eid, e in list(self.edges.items()):
                    if f in (e["src"], e["dst"]):
                        s2 = i if e["src"] == f else e["src"]
                        d2 = i if e["dst"] == f else e["dst"]
                        dup = any(x is not e and x["src"] == s2 and x["dst"] == d2 and x["kind"] == e["kind"] for x in self.edges.values())
                        if s2 == d2 or dup:
                            del self.edges[eid]
                        else:
                            e["src"], e["dst"] = s2, d2
                for n in self.nodes.values():
                    if n["merged_into"] == f:
                        n["merged_into"] = i
                touched.update([f, i]); applied += 1
            elif kind_op == "set_status":
                nid = resolve(op.get("nid")); st = op.get("status")
                if nid is None or not self.live(nid) or self.nodes[nid]["review"] != "auto":
                    self.reject("status_ref"); continue
                if st not in STATUS_KIND or self.nodes[nid]["kind"] not in STATUS_KIND[st]:
                    self.reject("status_kind"); continue
                self.nodes[nid]["status"] = st; self.nodes[nid]["grounds"] |= gset
                touched.add(nid); applied += 1
            else:
                self.reject("unknown_op")
        self.touched.append(touched)
        return applied

    def node_line(self, nid):
        n = self.nodes[nid]
        return f'{nid} [{n["kind"]}/{n["status"]}] {n["text"]}'


# ---------------------------------------------------------------- prompt building
def utt_line(u, cap=None):
    t = u["text"]
    if cap is not None and len(t) > cap:
        t = t[:cap] + "…"
    return f'{u["uid"]} {u["speaker_label"]}: {t}'


def build_incremental(g, K=3):
    budget = LIMIT - len(SYSTEM)
    third = budget // 3
    # context: open issues newest first, then nodes touched in last K patches
    ctx_ids = [nid for nid in sorted(g.nodes, key=lambda x: -int(x[1:]))
               if g.live(nid) and g.nodes[nid]["kind"] == "issue" and g.nodes[nid]["status"] == "open"]
    for t in reversed(g.touched[-K:]):
        for nid in sorted(t, key=lambda x: -int(x[1:])):
            if g.live(nid) and nid not in ctx_ids:
                ctx_ids.append(nid)
    ctx, used = [], 0
    for nid in ctx_ids:
        line = g.node_line(nid)
        if used + len(line) + 1 > third:
            break
        ctx.append(line); used += len(line) + 1
    pending = [u for u in g.utt if u["extracted_rev"] is None]
    first_idx = g.utt.index(pending[0]) if pending else len(g.utt)
    refs = [] if (pending and pending[0].get("retranscribed")) else g.utt[max(0, first_idx - 3):first_idx]
    ref_lines = [utt_line(u, 200) for u in refs]
    remain = budget - used - sum(len(x) + 1 for x in ref_lines) - 200
    new_lines, taken = [], []
    for u in pending:
        line = utt_line(u, third)
        if len(line) + 1 > remain and taken:
            break
        new_lines.append(line); taken.append(u); remain -= len(line) + 1
    user = ("## 既存のグラフ（文脈）\n" + ("\n".join(ctx) or "（まだ無い）") +
            "\n\n## 参照用の直前の発話\n" + ("\n".join(ref_lines) or "（無し）") +
            "\n\n## 新しい発話\n" + "\n".join(new_lines))
    allowed = {u["uid"] for u in refs + taken}
    return SYSTEM, user, taken, allowed


def build_consolidate(g):
    live = [nid for nid in g.nodes if g.live(nid)]
    by_uid = {u["uid"]: u for u in g.utt}
    lines, allowed = [], set()
    for nid in live:
        n = g.nodes[nid]
        gl = sorted(n["grounds"], key=lambda x: int(x[1:]))[:3]
        allowed |= set(gl)
        lines.append(g.node_line(nid) + "\n    根拠: " + " / ".join(utt_line(by_uid[x], 200) for x in gl))
    elines = [f'{e["src"]} -{e["kind"]}-> {e["dst"]}' for e in g.edges.values() if g.live(e["src"]) and g.live(e["dst"])]
    user = "## ノード\n" + "\n".join(lines) + "\n\n## エッジ\n" + "\n".join(elines)
    if len(CONSOLIDATE_SYSTEM) + len(user) > LIMIT:
        user = user[: LIMIT - len(CONSOLIDATE_SYSTEM) - 50] + "\n（以下省略）"
    return CONSOLIDATE_SYSTEM, user, allowed


# ---------------------------------------------------------------- backends
CLAUDE_DISALLOWED = "Bash,Edit,Write,Read,WebFetch,WebSearch,Task,Glob,Grep,NotebookEdit"


def call_llm(backend, model, system, user, timeout=180):
    prompt = system + "\n\n" + user
    t0 = time.monotonic()
    with tempfile.TemporaryDirectory() as wd:
        if backend == "claude":
            cmd = ["claude", "--print", "--output-format", "json", "--no-session-persistence",
                   "--disallowedTools", CLAUDE_DISALLOWED]
            if model:
                cmd += ["--model", model]
            cmd += ["--", prompt]
        elif backend == "claude-lean":
            cmd = ["claude", "--print", "--output-format", "json", "--no-session-persistence",
                   "--tools", "", "--strict-mcp-config", "--setting-sources", "", "--system-prompt", system]
            if model:
                cmd += ["--model", model]
            cmd += ["--", user]
        elif backend == "codex":
            out = os.path.join(wd, "out.txt")
            cmd = ["codex", "exec", "-s", "read-only", "--skip-git-repo-check", "--ephemeral", "--json", "-C", wd, "-o", out]
            if model:
                cmd += ["-m", model]
            cmd += ["--", prompt]
        else:
            raise SystemExit("unknown backend")
        try:
            p = subprocess.run(cmd, cwd=wd, capture_output=True, text=True, timeout=timeout, stdin=subprocess.DEVNULL)
        except subprocess.TimeoutExpired:
            return dict(ok=False, err="timeout", secs=time.monotonic() - t0)
        secs = time.monotonic() - t0
        if p.returncode != 0:
            return dict(ok=False, err=f"exit{p.returncode}", secs=secs, stderr=p.stderr[-300:])
        usage = {}
        if backend.startswith("claude"):
            try:
                v = json.loads(p.stdout)
            except Exception:
                return dict(ok=False, err="cli_json", secs=secs)
            text = v.get("result", "")
            u = v.get("usage", {})
            usage = dict(input=u.get("input_tokens"), cache_create=u.get("cache_creation_input_tokens"),
                         cache_read=u.get("cache_read_input_tokens"), output=u.get("output_tokens"),
                         cost_usd=v.get("total_cost_usd"), api_ms=v.get("duration_api_ms"))
        else:
            try:
                text = open(out, encoding="utf-8").read()
            except OSError:
                return dict(ok=False, err="no_output", secs=secs)
            for line in p.stdout.splitlines():
                try:
                    ev = json.loads(line)
                except Exception:
                    continue
                if ev.get("type") == "turn.completed" and "usage" in ev:
                    u = ev["usage"]
                    usage = dict(input=u.get("input_tokens"), cache_read=u.get("cached_input_tokens"),
                                 output=u.get("output_tokens"))
        return dict(ok=True, text=text, secs=secs, usage=usage, prompt_chars=len(prompt))


def parse_ops(text):
    t = re.sub(r"```(?:json)?", "", text)
    start = t.find("{")
    while start != -1:
        depth, instr, esc = 0, False, False
        for i in range(start, len(t)):
            c = t[i]
            if instr:
                if esc: esc = False
                elif c == "\\": esc = True
                elif c == '"': instr = False
            elif c == '"': instr = True
            elif c == "{": depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    try:
                        v = json.loads(t[start:i + 1])
                        if isinstance(v, dict) and "ops" in v:
                            return v["ops"]
                    except Exception:
                        pass
                    break
        start = t.find("{", start + 1)
    return None


# ---------------------------------------------------------------- run
def run(args):
    meeting = json.load(open(args.meeting, encoding="utf-8"))
    g = Graph()
    calls = []
    last_extract_ms = None
    fails = 0

    def extract(source):
        nonlocal fails
        system, user, taken, allowed = build_incremental(g)
        if not taken:
            return False
        for attempt in range(2):
            r = call_llm(args.backend, args.model, system, user)
            rec = dict(source=source, attempt=attempt, n_utt=len(taken), prompt_chars=len(system) + len(user),
                       ok=r["ok"], secs=round(r["secs"], 2), usage=r.get("usage"), err=r.get("err"))
            ops = parse_ops(r["text"]) if r["ok"] else None
            rec["parsed"] = ops is not None
            if ops is not None:
                before = dict(g.rejected)
                applied = g.apply(ops, allowed)
                rec.update(n_ops=len(ops), applied=applied,
                           rejected={k: g.rejected.get(k, 0) - before.get(k, 0) for k in g.rejected if g.rejected.get(k, 0) != before.get(k, 0)})
                for u in taken:
                    u["extracted_rev"] = g.rev
                calls.append(rec); fails = 0
                print(json.dumps(rec, ensure_ascii=False), file=sys.stderr, flush=True)
                return True
            calls.append(rec)
            print(json.dumps(rec, ensure_ascii=False), file=sys.stderr, flush=True)
            if r["ok"]:
                user = user + "\n\n（前回の応答は JSON として読めませんでした。JSON だけを返してください）"
                continue
            break
        fails += 1
        return False

    # live phase (latency ignored for batching; recorded separately)
    for idx, u in enumerate(meeting["utterances"]):
        g.utt.append(dict(uid=f"u{idx + 1}", row_id=u["row_id"], speaker_label=u["speaker_label"],
                          text=u["text"], start_ms=u.get("start_ms"), extracted_rev=None))
        now = u.get("start_ms")
        if now is None:
            now = idx * 6000  # no timestamps: assume ~6 s per utterance
        if last_extract_ms is None:
            last_extract_ms = now
        pending = sum(1 for x in g.utt if x["extracted_rev"] is None)
        if fails < 3 and (pending >= args.batch or (now - last_extract_ms >= args.secs * 1000 and pending >= 1)):
            extract("live")
            last_extract_ms = now
    live_calls = len(calls)
    # flush
    flush_fails = 0
    while any(x["extracted_rev"] is None for x in g.utt) and flush_fails < 3:
        if not extract("flush"):
            flush_fails += 1
    # consolidate
    if not args.no_consolidate and flush_fails < 3:
        system, user, allowed = build_consolidate(g)
        r = call_llm(args.backend, args.model, system, user, timeout=300)
        ops = parse_ops(r["text"]) if r["ok"] else None
        rec = dict(source="consolidate", prompt_chars=len(system) + len(user), ok=r["ok"], secs=round(r["secs"], 2),
                   usage=r.get("usage"), parsed=ops is not None, err=r.get("err"))
        if ops is not None:
            before = dict(g.rejected)
            rec.update(n_ops=len(ops), applied=g.apply(ops, allowed),
                       rejected={k: g.rejected.get(k, 0) - before.get(k, 0) for k in g.rejected if g.rejected.get(k, 0) != before.get(k, 0)})
        calls.append(rec)
        print(json.dumps(rec, ensure_ascii=False), file=sys.stderr, flush=True)
    uid2row = {u["uid"]: u["row_id"] for u in g.utt}
    out = dict(meeting=args.meeting, backend=args.backend, model=args.model, batch=args.batch, secs=args.secs,
               live_calls=live_calls, calls=calls, rejected=g.rejected,
               nodes={k: dict(v, grounds=sorted(uid2row[x] for x in v["grounds"])) for k, v in g.nodes.items()},
               edges={k: dict(v, grounds=sorted(uid2row[x] for x in v["grounds"])) for k, v in g.edges.items()})
    json.dump(out, open(args.out, "w", encoding="utf-8"), ensure_ascii=False, indent=1)


# ---------------------------------------------------------------- eval
def evaluate(result, gold):
    pn = {k: v for k, v in result["nodes"].items() if v["status"] != "merged"}
    gn = {n["id"]: n for n in gold["nodes"]}

    def jac(a, b):
        a, b = set(a), set(b)
        return len(a & b) / len(a | b) if a | b else 0

    def match(strict):
        pairs = []
        for p, pv in pn.items():
            for gid, gv in gn.items():
                if strict and pv["kind"] != gv["kind"]:
                    continue
                j = jac(pv["grounds"], gv["grounds"])
                if j > 0:
                    pairs.append((j, p, gid))
        pairs.sort(reverse=True)
        m, used_p, used_g = {}, set(), set()
        for j, p, gid in pairs:
            if p not in used_p and gid not in used_g:
                m[p] = gid; used_p.add(p); used_g.add(gid)
        # duplicates: unmatched predicted nodes whose grounds overlap an already-matched gold node of same kind
        dup = sum(1 for p, pv in pn.items() if p not in m and any(
            gv["kind"] == pv["kind"] and jac(pv["grounds"], gv["grounds"]) > 0 and gid in used_g for gid, gv in gn.items()))
        return m, dup

    out = {}
    for strict in (True, False):
        m, dup = match(strict)
        tp = len(m)
        prec = tp / len(pn) if pn else 0
        rec = tp / len(gn) if gn else 0
        key = "kind_strict" if strict else "kind_loose"
        out[key] = dict(pred=len(pn), gold=len(gn), matched=tp, precision=round(prec, 3), recall=round(rec, 3),
                        f1=round(2 * prec * rec / (prec + rec), 3) if prec + rec else 0, duplicates=dup)
        if strict:
            ge = {(e["src"], e["dst"], e["kind"]) for e in gold["edges"]}
            pe = set()
            for e in result["edges"].values():
                s, d = m.get(e["src"]), m.get(e["dst"])
                pe.add((s, d, e["kind"]))
            tp_e = len(pe & ge)
            pe_n = len(result["edges"])
            ep = tp_e / pe_n if pe_n else 0
            er = tp_e / len(ge) if ge else 0
            out["edges"] = dict(pred=pe_n, gold=len(ge), matched=tp_e, precision=round(ep, 3), recall=round(er, 3),
                                f1=round(2 * ep * er / (ep + er), 3) if ep + er else 0)
            # end-of-meeting decisions/actions (last 4 rows)
            n_rows = max(max(n["grounds"]) for n in gold["nodes"])
            tail = [gid for gid, gv in gn.items() if gv["kind"] in ("decision", "action") and max(gv["grounds"]) > n_rows - 4]
            out["tail_decisions_found"] = f"{sum(1 for gid in tail if gid in m.values())}/{len(tail)}"
    calls = [c for c in result["calls"] if c["source"] in ("live", "flush")]
    secs = sorted(c["secs"] for c in calls if c["ok"])
    pct = lambda q: secs[min(len(secs) - 1, int(q * len(secs)))] if secs else None
    tok_in = sum((c.get("usage") or {}).get("input") or 0 for c in result["calls"])
    tok_cc = sum((c.get("usage") or {}).get("cache_create") or 0 for c in result["calls"])
    tok_cr = sum((c.get("usage") or {}).get("cache_read") or 0 for c in result["calls"])
    tok_out = sum((c.get("usage") or {}).get("output") or 0 for c in result["calls"])
    cost = sum((c.get("usage") or {}).get("cost_usd") or 0 for c in result["calls"])
    parsed = [c for c in result["calls"] if c["ok"]]
    n_ops = sum(c.get("n_ops", 0) for c in result["calls"])
    n_rej = sum(result["rejected"].values())
    out["calls"] = dict(total=len(result["calls"]), live=result["live_calls"], ok=len(parsed),
                        parse_rate=round(sum(1 for c in parsed if c["parsed"]) / len(parsed), 3) if parsed else 0,
                        p50_s=pct(0.5), p95_s=pct(0.95), max_prompt_chars=max(c["prompt_chars"] for c in result["calls"]))
    out["tokens"] = dict(input=tok_in, cache_create=tok_cc, cache_read=tok_cr, output=tok_out, cost_usd=round(cost, 4))
    out["ops"] = dict(total=n_ops, rejected=n_rej, reject_rate=round(n_rej / n_ops, 3) if n_ops else 0, by_rule=result["rejected"])
    return out


def main():
    ap = argparse.ArgumentParser()
    sp = ap.add_subparsers(dest="cmd", required=True)
    r = sp.add_parser("run")
    r.add_argument("--meeting", required=True); r.add_argument("--backend", required=True)
    r.add_argument("--model", default=""); r.add_argument("--out", required=True)
    r.add_argument("--batch", type=int, default=6); r.add_argument("--secs", type=int, default=45)
    r.add_argument("--no-consolidate", action="store_true")
    e = sp.add_parser("eval")
    e.add_argument("--result", required=True); e.add_argument("--gold", required=True)
    a = ap.parse_args()
    if a.cmd == "run":
        run(a)
    else:
        print(json.dumps(evaluate(json.load(open(a.result, encoding="utf-8")), json.load(open(a.gold, encoding="utf-8"))),
                         ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
