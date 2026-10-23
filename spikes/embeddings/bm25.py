"""Plain BM25 keyword search over the passages, like the Tantivy spike:
lowercase words, English stop words dropped, a crude English stemmer."""
import math
import re
from collections import Counter

STOP = set("""a an and are as at be but by can do does for from has have how i if in into is it its
of on or should so that the their them then there these they this to was what when where which who
why will with you your my me we our after before about not no""".split())


def stem(w):
    for suf in ("ing", "ed", "es", "s", "ly"):
        if len(w) > len(suf) + 3 and w.endswith(suf):
            return w[: -len(suf)]
    return w


def tokens(text):
    return [stem(w) for w in re.findall(r"\w+", text.lower()) if w not in STOP]


class BM25:
    def __init__(self, docs, k1=1.2, b=0.75):
        self.k1, self.b = k1, b
        self.tf = [Counter(tokens(d)) for d in docs]
        self.len = [sum(t.values()) for t in self.tf]
        self.avg = sum(self.len) / len(self.len)
        df = Counter(w for t in self.tf for w in t)
        n = len(docs)
        self.idf = {w: math.log(1 + (n - f + 0.5) / (f + 0.5)) for w, f in df.items()}

    def search(self, query, k=50):
        q = tokens(query)
        scores = []
        for i, tf in enumerate(self.tf):
            s = 0.0
            for w in q:
                f = tf.get(w)
                if f:
                    s += self.idf[w] * f * (self.k1 + 1) / (f + self.k1 * (1 - self.b + self.b * self.len[i] / self.avg))
            if s:
                scores.append((s, i))
        scores.sort(reverse=True)
        return [i for _, i in scores[:k]]
