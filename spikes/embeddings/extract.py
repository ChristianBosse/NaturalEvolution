"""Cut WikiMed articles into short passages for the retrieval test.

Runs inside python:3.12-slim with `pip install libzim` (see README).

    python extract.py check  ZIM TITLE...        # which titles exist (follows redirects)
    python extract.py build  ZIM articles.txt N  # write passages.jsonl with about N passages

articles.txt lists one article title per line: the articles the questions need,
plus related articles that make hard distractors. Random articles fill up to N.
"""
import html.parser
import json
import random
import re
import sys

from libzim.reader import Archive

SKIP_TAGS = {"sup", "table", "style", "script", "figure", "figcaption", "math"}
STOP_SECTIONS = re.compile(r"^(references|external links|see also|further reading|notes|bibliography|sources|citations|footnotes)$", re.I)
BLOCK = {"p", "li", "dd"}
HEAD = {"h1", "h2", "h3", "h4"}


class Text(html.parser.HTMLParser):
    """Collect (section, paragraph) blocks; skip references, tables and boxes."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.blocks, self.section, self.skip, self.buf, self.mode, self.stopped = [], "", 0, [], None, False

    def handle_starttag(self, tag, attrs):
        cls = dict(attrs).get("class") or ""
        if self.skip or tag in SKIP_TAGS or "navbox" in cls or "hatnote" in cls or "reflist" in cls or "mw-ref" in cls:
            if tag not in ("br", "img", "hr", "meta", "link", "input", "wbr"):
                self.skip += 1
            return
        if tag in HEAD or tag in BLOCK:
            self.flush()
            self.mode = "head" if tag in HEAD else "block"

    def handle_endtag(self, tag):
        if self.skip:
            self.skip -= 1
            return
        if tag in HEAD or tag in BLOCK:
            self.flush()

    def handle_data(self, data):
        if not self.skip and self.mode:
            self.buf.append(data)

    def flush(self):
        text = re.sub(r"\s+", " ", "".join(self.buf)).strip()
        text = re.sub(r"\[\d+\]|\[citation needed\]", "", text)
        if self.mode == "head" and text:
            self.section = text
            if STOP_SECTIONS.match(text):
                self.stopped = True
        elif self.mode == "block" and len(text) > 40 and not self.stopped:
            self.blocks.append((self.section, text))
        self.buf, self.mode = [], None


def article(arch, title):
    """Return (title, html) for a title or path, following redirects."""
    for path in (title.replace(" ", "_"), title):
        if arch.has_entry_by_path(path):
            e = arch.get_entry_by_path(path)
            while e.is_redirect:
                e = e.get_redirect_entry()
            return e.title, bytes(e.get_item().content).decode("utf-8", "replace")
    return None, None


def passages(title, page, max_words=220):
    """Group paragraphs of a section into passages of up to about max_words words."""
    p = Text()
    p.feed(page)
    p.flush()
    out, cur, cur_sec = [], [], None
    for sec, text in p.blocks:
        words = len(text.split())
        if cur and (sec != cur_sec or sum(len(t.split()) for t in cur) + words > max_words):
            out.append((cur_sec, " ".join(cur)))
            cur = []
        cur_sec = sec
        if words > max_words * 1.5:  # split long paragraphs at sentence ends
            sents, part = re.split(r"(?<=[.!?]) ", text), []
            for s in sents:
                part.append(s)
                if len(" ".join(part).split()) >= max_words:
                    out.append((sec, " ".join(part)))
                    part = []
            if part:
                cur = [" ".join(part)]
            continue
        cur.append(text)
    if cur:
        out.append((cur_sec, " ".join(cur)))
    return [(title + (" - " + s if s and s != title else ""), t) for s, t in out]


def main():
    mode, zim = sys.argv[1], sys.argv[2]
    arch = Archive(zim)
    if mode == "check":
        names = sys.argv[3:]
        if names[0].endswith(".txt"):
            names = [l.strip() for l in open(names[0], encoding="utf-8") if l.strip() and not l.startswith("#")]
        for t in names:
            real, _ = article(arch, t)
            if real != t:
                print(f"{t} -> {real}")
        return
    titles = [l.strip() for l in open(sys.argv[3], encoding="utf-8") if l.strip() and not l.startswith("#")]
    target = int(sys.argv[4])
    out, seen = [], set()

    def add(title, page, limit=None):
        ps = passages(title, page)[:limit] if limit else passages(title, page)
        for head, text in ps:
            out.append({"id": f"p{len(out):05d}", "title": title, "head": head, "text": text})

    for t in titles:
        real, page = article(arch, t)
        if not real:
            print("missing:", t, file=sys.stderr)
            continue
        if real in seen:
            continue
        seen.add(real)
        add(real, page)
    print(f"{len(out)} passages from {len(seen)} listed articles", file=sys.stderr)
    rng = random.Random(42)
    n = arch.all_entry_count
    while len(out) < target:
        e = arch._get_entry_by_id(rng.randrange(n))
        if e.is_redirect or e.title in seen or "/" in e.path or "." in e.path[-5:]:
            continue
        item = e.get_item()
        if item.mimetype != "text/html":
            continue
        seen.add(e.title)
        add(e.title, bytes(item.content).decode("utf-8", "replace"), limit=3)
    with open("passages.jsonl", "w", encoding="utf-8") as f:
        for p in out[:target]:
            f.write(json.dumps(p, ensure_ascii=False) + "\n")
    print(f"wrote {min(len(out), target)} passages, {len(seen)} articles", file=sys.stderr)


if __name__ == "__main__":
    main()
