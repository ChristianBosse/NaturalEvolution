"""Quick check that an embedding model gives sensible similarities.

    python sanity.py e5-small
"""
import sys

from run_emb import MODELS, embed, start, wait_ready

name = sys.argv[1]
_, _, _, qp, dp = MODELS[name]
port, stop = start("gpu", name)
try:
    wait_ready(port, 120)
    q = embed(port, [qp + "snake bite first aid", qp + "morsure de serpent"])
    d = embed(port, [dp + "Snakebite. First aid: keep the person calm and immobilize the bitten limb.",
                     dp + "Car engines need oil changes every 10,000 km.",
                     dp + "Hypothermia is a body temperature below 35 degrees."])
    print((q @ d.T).round(3))
finally:
    stop()
