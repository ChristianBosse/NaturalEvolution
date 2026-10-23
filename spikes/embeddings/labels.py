"""Turn the 'rel' rules in questions.json into relevant passage IDs.

    python labels.py            # print a short summary per question (to review the rules)
    python labels.py QID        # print the matching passages of one question

A rule is [title, regex] or [title, regex, head_regex]. The optional third part
restricts the match to passages whose "Title - Section" head matches.
"""
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load():
    ps = [json.loads(l) for l in open(HERE / "data" / "passages.jsonl", encoding="utf-8")]
    qs = json.load(open(HERE / "questions.json", encoding="utf-8"))["questions"]
    by_id = {q["id"]: q for q in qs}
    for q in qs:
        base = by_id[q.get("same", q["id"])]
        q["rel"], q["en"], q["topic"] = base["rel"], base["q"], base["topic"]
        q["relevant"] = sorted(relevant(ps, base["rel"]))
    return ps, qs


def relevant(ps, rules):
    out = set()
    for rule in rules:
        title, rx = rule[0], re.compile(rule[1], re.I)
        hx = re.compile(rule[2], re.I) if len(rule) > 2 else None
        for p in ps:
            if p["title"] == title and rx.search(p["text"]) and (not hx or hx.search(p["head"])):
                out.add(p["id"])
    return out


if __name__ == "__main__":
    ps, qs = load()
    pid = {p["id"]: p for p in ps}
    if len(sys.argv) > 2 and sys.argv[1] == "heads":  # section heads of some articles
        for t in sys.argv[2:]:
            heads = [p["head"].split(" - ", 1)[-1][:28] for p in ps if p["title"] == t]
            print(t, "|", "; ".join(dict.fromkeys(heads)))
    elif len(sys.argv) > 1:
        q = next(q for q in qs if q["id"] == sys.argv[1])
        for i in q["relevant"]:
            print(i, pid[i]["head"], "|", pid[i]["text"][:110])
    else:
        for q in qs:
            if q["lang"] == "en":
                heads = sorted({pid[i]["head"].split(" - ", 1)[-1][:22] for i in q["relevant"]})
                print(f'{q["id"]:13} {len(q["relevant"]):3}  {"; ".join(heads)[:150]}')
