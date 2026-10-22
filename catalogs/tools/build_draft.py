"""Build catalogs/nevo-catalog-draft.json from the Kiwix OPDS feed.

Not part of NEVO; a reproducible helper for the catalog draft (docs/spec/catalog.md).

    curl -L -o opds.xml "https://library.kiwix.org/catalog/v2/entries?count=-1"
    python -I build_draft.py opds.xml CACHE_DIR OUT.json [KITS_TABLE.md]

It reads the OPDS feed, picks the packs listed below, fetches each pack's
.sha256 and .meta4 file (small text files, never the ZIM itself) into
CACHE_DIR, and writes the draft catalog. Sizes are bytes; "GB" means 10^9.
"""
import json
import re
import sys
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

BUILT = "2026-10-22"
NS = {"a": "http://www.w3.org/2005/Atom"}
ML = {"m": "urn:ietf:params:xml:ns:metalink"}
LANG = {"eng": "en", "fra": "fr", "spa": "es"}
GB = 10**9

# ---------------------------------------------------------------- licenses
# How each license was checked on BUILT. "site" = the source site's own
# license notice was fetched and read; "known" = well documented, not refetched.
WIKIMEDIA = {
    "license": "CC-BY-SA-4.0",
    "license_note": "Text under CC BY-SA 4.0 (older revisions also GFDL). Images keep their own free licenses; nopic and mini files leave most of them out.",
    "license_check": "site: Wikimedia footer (checked on en.wikivoyage.org), CC BY-SA 4.0",
}
SOURCES = {
    "wikipedia": dict(WIKIMEDIA, attribution="Wikipedia contributors", homepage="https://www.wikipedia.org"),
    "wikibooks": dict(WIKIMEDIA, attribution="Wikibooks contributors", homepage="https://www.wikibooks.org"),
    "wikivoyage": dict(WIKIMEDIA, attribution="Wikivoyage contributors", homepage="https://www.wikivoyage.org"),
    "wikiversity": dict(WIKIMEDIA, attribution="Wikiversity contributors", homepage="https://www.wikiversity.org"),
    "wiktionary": dict(WIKIMEDIA, attribution="Wiktionary contributors", homepage="https://www.wiktionary.org"),
    "wikisource": dict(WIKIMEDIA, attribution="Wikisource contributors", homepage="https://wikisource.org",
                       license_note="Site contributions under CC BY-SA 4.0; most source texts are public domain."),
    "mdwiki": {"license": "CC-BY-SA-4.0", "attribution": "MDWiki / WikiProjectMed contributors",
               "homepage": "https://mdwiki.org", "license_check": "site: mdwiki.org footer, CC BY-SA 4.0"},
    "wikem": {"license": "CC-BY-SA-4.0", "attribution": "WikEM contributors", "homepage": "https://wikem.org",
              "license_check": "site: wikem.org/wiki/WikEM:Copyrights, CC BY-SA 4.0"},
    "appropedia": {"license": "CC-BY-SA-3.0 AND CC-BY-SA-4.0", "attribution": "Appropedia contributors",
                   "homepage": "https://www.appropedia.org",
                   "license_note": "Newer pages CC BY-SA 4.0, older pages CC BY-SA 3.0.",
                   "license_check": "site: appropedia.org footer names both"},
    "energypedia": {"license": "CC-BY-SA-4.0", "attribution": "energypedia contributors",
                    "homepage": "https://energypedia.info", "license_check": "site: energypedia.info footer, CC BY-SA 4.0"},
    "restarters": {"license": "CC-BY-SA-4.0", "attribution": "The Restart Project wiki contributors",
                   "homepage": "https://wiki.restarters.net", "license_check": "site: wiki.restarters.net footer, CC BY-SA 4.0"},
    "gardenology": {"license": "CC-BY-SA-3.0", "attribution": "Gardenology.org contributors",
                    "homepage": "https://www.gardenology.org", "license_check": "site: gardenology.org footer badge, CC BY-SA 3.0"},
    "stackexchange": {"license": "CC-BY-SA-2.5 AND CC-BY-SA-3.0 AND CC-BY-SA-4.0", "attribution": "Stack Exchange contributors",
                      "homepage": "https://stackexchange.com",
                      "license_note": "Posts are CC BY-SA 2.5, 3.0 or 4.0 depending on when they were written.",
                      "license_check": "known: Stack Exchange terms"},
    "vikidia": {"license": "CC-BY-SA-3.0", "attribution": "Vikidia contributors", "homepage": "https://www.vikidia.org",
                "license_check": "site: en.vikidia.org and fr.vikidia.org footers, CC BY-SA 3.0 (es assumed the same)"},
    "phet": {"license": "CC-BY-4.0", "attribution": "PhET Interactive Simulations, University of Colorado Boulder",
             "homepage": "https://phet.colorado.edu",
             "license_note": "Simulations CC BY 4.0. PhET asks that its name and logo stay visible.",
             "license_check": "site: phet.colorado.edu/en/licensing/html"},
    "gutenberg": {"license": "LicenseRef-PublicDomain-US", "attribution": "Project Gutenberg",
                  "homepage": "https://www.gutenberg.org",
                  "license_note": "Public domain in the USA. A few works may still be under copyright in countries with longer terms.",
                  "license_check": "known: Project Gutenberg license"},
    "maps": {"license": "ODbL-1.0", "attribution": "(c) OpenStreetMap contributors",
             "homepage": "https://www.openstreetmap.org",
             "license_note": "Map data from OpenStreetMap under ODbL 1.0. Licenses of the map style, fonts and viewer inside the ZIM are not yet checked (open question).",
             "license_check": "known: OSM data license; scraper github.com/openzim/maps"},
}

# ---------------------------------------------------------------- packs
# (kiwix book name, flavour, source key, topics)
PACKS = [
    # medical
    ("wikipedia_en_medicine", "mini", "wikipedia", ["medical"]),
    ("wikipedia_en_medicine", "nopic", "wikipedia", ["medical"]),
    ("wikipedia_en_medicine", "maxi", "wikipedia", ["medical"]),
    ("wikipedia_fr_medicine", "nopic", "wikipedia", ["medical"]),
    ("wikipedia_fr_medicine", "maxi", "wikipedia", ["medical"]),
    ("wikipedia_es_medicine", "nopic", "wikipedia", ["medical"]),
    ("wikipedia_es_medicine", "maxi", "wikipedia", ["medical"]),
    ("mdwiki_en_all", "maxi", "mdwiki", ["medical"]),
    ("wikem_en_all", "nopic", "wikem", ["medical/first-aid"]),
    ("wikem_en_all", "maxi", "wikem", ["medical/first-aid"]),
    # farming and food
    ("gardening.stackexchange.com_en_all", "", "stackexchange", ["farming/crops"]),
    ("cooking.stackexchange.com_en_all", "", "stackexchange", ["food/cooking"]),
    ("gardenology_en_all", "", "gardenology", ["farming/crops"]),
    ("gutenberg_en_lcc-s", "", "gutenberg", ["farming", "culture/books"]),
    # repair and build
    ("appropedia_en_all", "maxi", "appropedia", ["shelter/building", "water", "energy", "farming"]),
    ("energypedia_en_all", "nopic", "energypedia", ["energy/solar", "energy/cooking"]),
    ("energypedia_en_all", "maxi", "energypedia", ["energy/solar", "energy/cooking"]),
    ("restarters_en_all", "maxi", "restarters", ["repair/electronics"]),
    ("gutenberg_en_lcc-t", "", "gutenberg", ["repair", "culture/books"]),
    ("wikibooks_en_all", "nopic", "wikibooks", ["education", "repair"]),
    ("wikibooks_en_all", "maxi", "wikibooks", ["education", "repair"]),
    ("wikibooks_fr_all", "nopic", "wikibooks", ["education", "repair"]),
    ("wikibooks_fr_all", "maxi", "wikibooks", ["education", "repair"]),
    ("wikibooks_es_all", "nopic", "wikibooks", ["education", "repair"]),
    ("wikibooks_es_all", "maxi", "wikibooks", ["education", "repair"]),
    # survival basics
    ("wikivoyage_en_all", "nopic", "wikivoyage", ["general"]),
    ("wikivoyage_en_all", "maxi", "wikivoyage", ["general"]),
    ("wikivoyage_fr_all", "nopic", "wikivoyage", ["general"]),
    ("wikivoyage_fr_all", "maxi", "wikivoyage", ["general"]),
    ("wikivoyage_es_all", "nopic", "wikivoyage", ["general"]),
    ("wikivoyage_es_all", "maxi", "wikivoyage", ["general"]),
    ("wikipedia_en_knots", "maxi", "wikipedia", ["safety"]),
    # education
    ("wikipedia_en_top", "nopic", "wikipedia", ["general", "education"]),
    ("wikipedia_fr_top", "nopic", "wikipedia", ["general", "education"]),
    ("wikipedia_es_top", "nopic", "wikipedia", ["general", "education"]),
    ("wikipedia_en_all", "mini", "wikipedia", ["general"]),
    ("wikipedia_fr_all", "mini", "wikipedia", ["general"]),
    ("wikipedia_es_all", "mini", "wikipedia", ["general"]),
    ("wikipedia_en_all", "nopic", "wikipedia", ["general"]),
    ("wikipedia_fr_all", "nopic", "wikipedia", ["general"]),
    ("wikipedia_es_all", "nopic", "wikipedia", ["general"]),
    ("phet_en_all", "", "phet", ["education/science", "education/math"]),
    ("phet_fr_all", "", "phet", ["education/science", "education/math"]),
    ("phet_es_all", "", "phet", ["education/science", "education/math"]),
    ("vikidia_en_all", "nopic", "vikidia", ["education"]),
    ("vikidia_en_all", "maxi", "vikidia", ["education"]),
    ("vikidia_fr_all", "nopic", "vikidia", ["education"]),
    ("vikidia_fr_all", "maxi", "vikidia", ["education"]),
    ("vikidia_es_all", "nopic", "vikidia", ["education"]),
    ("vikidia_es_all", "maxi", "vikidia", ["education"]),
    ("wikiversity_en_all", "nopic", "wikiversity", ["education"]),
    ("wikiversity_fr_all", "nopic", "wikiversity", ["education"]),
    ("wikiversity_es_all", "nopic", "wikiversity", ["education"]),
    ("wiktionary_en_all", "nopic", "wiktionary", ["education/languages"]),
    ("wiktionary_fr_all", "nopic", "wiktionary", ["education/languages"]),
    ("wiktionary_es_all", "nopic", "wiktionary", ["education/languages"]),
    ("gutenberg_fr_all", "", "gutenberg", ["culture/books"]),
    ("gutenberg_es_all", "", "gutenberg", ["culture/books"]),
    ("wikisource_es_all", "nopic", "wikisource", ["culture/books"]),
    # maps (language-neutral data, English interface)
    ("maps_en_africa", "", "maps", ["maps"]),
    ("maps_en_asia", "", "maps", ["maps"]),
    ("maps_en_europe", "", "maps", ["maps"]),
    ("maps_en_north-america", "", "maps", ["maps"]),
    ("maps_en_central-america", "", "maps", ["maps"]),
    ("maps_en_south-america", "", "maps", ["maps"]),
    ("maps_en_australia-oceania", "", "maps", ["maps"]),
    ("maps_en_all", "", "maps", ["maps"]),
]


def pid(name, flavour):
    return "kiwix/" + name + ("_" + flavour if flavour else "")


# ---------------------------------------------------------------- kits
# Per language: "packs" in that language, "fallback" English packs the AI can
# still answer from (it translates the question first, D-037), and
# "optional" extras that Seed offers but does not count in the kit size.
def K(name, flavour=""):
    return pid(name, flavour)


EN_MED_C = [K("wikipedia_en_medicine", "nopic"), K("wikem_en_all", "nopic")]
EN_FARM_C = [K("gardening.stackexchange.com_en_all"), K("cooking.stackexchange.com_en_all"),
             K("gardenology_en_all"), K("appropedia_en_all", "maxi")]
EN_REPAIR_FB = [K("appropedia_en_all", "maxi"), K("energypedia_en_all", "nopic"), K("restarters_en_all", "maxi")]
EN_SURV_FB = [K("wikipedia_en_knots", "maxi"), K("appropedia_en_all", "maxi")]

KITS = [
    {"id": "medical", "title": "Medical", "topics": ["medical"],
     "about": "First aid, diseases, medicines, emergency care.",
     "langs": {
         "en": {"compact": {"packs": EN_MED_C},
                "full": {"packs": [K("wikipedia_en_medicine", "maxi"), K("mdwiki_en_all", "maxi"), K("wikem_en_all", "maxi")]}},
         "fr": {"compact": {"packs": [K("wikipedia_fr_medicine", "nopic")], "fallback": EN_MED_C},
                "full": {"packs": [K("wikipedia_fr_medicine", "maxi")], "fallback": EN_MED_C + [K("mdwiki_en_all", "maxi")]}},
         "es": {"compact": {"packs": [K("wikipedia_es_medicine", "nopic")], "fallback": EN_MED_C},
                "full": {"packs": [K("wikipedia_es_medicine", "maxi")], "fallback": EN_MED_C + [K("mdwiki_en_all", "maxi")]}},
     }},
    {"id": "farming-food", "title": "Farming and food", "topics": ["farming", "food", "water"],
     "about": "Growing food, gardening, cooking, food safety.",
     "langs": {
         "en": {"compact": {"packs": EN_FARM_C},
                "full": {"packs": EN_FARM_C + [K("gutenberg_en_lcc-s")]}},
         "fr": {"compact": {"packs": [], "fallback": EN_FARM_C}, "full": {"packs": [], "fallback": EN_FARM_C}},
         "es": {"compact": {"packs": [], "fallback": EN_FARM_C}, "full": {"packs": [], "fallback": EN_FARM_C}},
     }},
    {"id": "repair-build", "title": "Repair and build", "topics": ["repair", "energy", "shelter", "water"],
     "about": "Fixing things, building, energy and water systems.",
     "langs": {
         "en": {"compact": {"packs": EN_REPAIR_FB, "optional": [K("wikibooks_en_all", "nopic")]},
                "full": {"packs": [K("appropedia_en_all", "maxi"), K("energypedia_en_all", "maxi"), K("restarters_en_all", "maxi"),
                                   K("wikibooks_en_all", "maxi"), K("gutenberg_en_lcc-t")]}},
         "fr": {"compact": {"packs": [K("wikibooks_fr_all", "nopic")], "fallback": EN_REPAIR_FB},
                "full": {"packs": [K("wikibooks_fr_all", "maxi")], "fallback": EN_REPAIR_FB}},
         "es": {"compact": {"packs": [K("wikibooks_es_all", "nopic")], "fallback": EN_REPAIR_FB},
                "full": {"packs": [K("wikibooks_es_all", "maxi")], "fallback": EN_REPAIR_FB}},
     }},
    {"id": "survival", "title": "Survival basics", "topics": ["medical/first-aid", "water", "shelter", "safety"],
     "about": "The small first kit: first aid, water, shelter, knots, local travel info.",
     "langs": {
         "en": {"compact": {"packs": [K("wikipedia_en_medicine", "mini"), K("wikivoyage_en_all", "nopic")] + EN_SURV_FB},
                "full": {"packs": [K("wikipedia_en_medicine", "nopic"), K("wikem_en_all", "nopic"), K("wikivoyage_en_all", "maxi")] + EN_SURV_FB}},
         "fr": {"compact": {"packs": [K("wikipedia_fr_medicine", "nopic"), K("wikivoyage_fr_all", "nopic")], "fallback": EN_SURV_FB},
                "full": {"packs": [K("wikipedia_fr_medicine", "maxi"), K("wikivoyage_fr_all", "maxi")], "fallback": EN_SURV_FB}},
         "es": {"compact": {"packs": [K("wikipedia_es_medicine", "nopic"), K("wikivoyage_es_all", "nopic")], "fallback": EN_SURV_FB},
                "full": {"packs": [K("wikipedia_es_medicine", "maxi"), K("wikivoyage_es_all", "maxi")], "fallback": EN_SURV_FB}},
     }},
    {"id": "education", "title": "Education", "topics": ["education", "general", "culture"],
     "about": "Encyclopedia, textbooks, science simulations, dictionary, books.",
     "langs": {
         "en": {"compact": {"packs": [K("wikipedia_en_top", "nopic"), K("phet_en_all"), K("vikidia_en_all", "nopic"), K("wikibooks_en_all", "nopic")],
                            "optional": [K("wikipedia_en_all", "mini")]},
                "full": {"packs": [K("wikipedia_en_all", "nopic"), K("phet_en_all"), K("vikidia_en_all", "maxi"), K("wikibooks_en_all", "maxi"),
                                   K("wikiversity_en_all", "nopic"), K("wiktionary_en_all", "nopic")]}},
         "fr": {"compact": {"packs": [K("wikipedia_fr_top", "nopic"), K("phet_fr_all"), K("vikidia_fr_all", "nopic"), K("wikibooks_fr_all", "nopic")],
                            "optional": [K("wikipedia_fr_all", "mini")]},
                "full": {"packs": [K("wikipedia_fr_all", "nopic"), K("phet_fr_all"), K("vikidia_fr_all", "maxi"), K("wikibooks_fr_all", "maxi"),
                                   K("wikiversity_fr_all", "nopic"), K("wiktionary_fr_all", "nopic"), K("gutenberg_fr_all")]}},
         "es": {"compact": {"packs": [K("wikipedia_es_top", "nopic"), K("phet_es_all"), K("vikidia_es_all", "nopic"), K("wikibooks_es_all", "nopic")],
                            "optional": [K("wikipedia_es_all", "mini")]},
                "full": {"packs": [K("wikipedia_es_all", "nopic"), K("phet_es_all"), K("vikidia_es_all", "maxi"), K("wikibooks_es_all", "maxi"),
                                   K("wikiversity_es_all", "nopic"), K("wiktionary_es_all", "nopic"), K("gutenberg_es_all"),
                                   K("wikisource_es_all", "nopic")]}},
     }},
    {"id": "maps", "title": "Maps", "topics": ["maps"],
     "about": "OpenStreetMap maps with roads and places. Pick one region; the world map is the full option.",
     "region_choice": [K("maps_en_africa"), K("maps_en_asia"), K("maps_en_europe"), K("maps_en_north-america"),
                       K("maps_en_central-america"), K("maps_en_south-america"), K("maps_en_australia-oceania")],
     "langs": {
         "mul": {"compact": {"packs": [K("maps_en_africa")], "note": "Example region; Seed asks which region."},
                 "full": {"packs": [K("maps_en_all")]}},
     }},
]

# ---------------------------------------------------------------- flagged
# Seen in the Kiwix library but NOT in any kit. Not downloaded or hashed.
FLAGGED = [
    ("ifixit_en_all", "", "CC-BY-NC-SA-3.0", "noncommercial", "Clear license, but NonCommercial: not open by the Open Definition. Best repair guides available; candidate for an opt-in noncommercial list (D-046)."),
    ("ifixit_fr_all", "", "CC-BY-NC-SA-3.0", "noncommercial", "As iFixit English."),
    ("ifixit_es_all", "", "CC-BY-NC-SA-3.0", "noncommercial", "As iFixit English."),
    ("khanacademy_en_all", "", "CC-BY-NC-SA-3.0", "noncommercial", "NonCommercial, and 180 GB (100-161 GB for fr/es). Videos from 2023."),
    ("ted_mul_medicine", "", "CC-BY-NC-ND-4.0", "noncommercial", "TED talks are NonCommercial-NoDerivatives. Same for ted_mul_agriculture, _farming, _food, _water."),
    ("wikivet.net_en_all", "maxi", "CC-BY-NC-ND-3.0", "noncommercial", "Site footer says CC BY-NC-ND 3.0."),
    ("libretexts.org_en_med", "", None, "mixed", "LibreTexts books each have their own license, many NonCommercial. Same for all libretexts.org_en_* packs."),
    ("zimgit-post-disaster_en", "", None, "unclear", "Collection of PDFs from many sources; no per-document licenses. Same for zimgit-water, zimgit-medicine, zimgit-knots, zimgit-food-preparation."),
    ("cd3wdproject.org_en_all", "", None, "unclear", "Archive of development documents from many publishers; licenses not stated."),
    ("survivorlibrary.com_en_all", "", None, "unclear", "Claims old public-domain books; not verified, and 220 GB."),
    ("ready.gov_en_all", "", None, "verify", "US federal work, probably public domain (17 U.S.C. 105), but includes third-party material; the site's policy page could not be fetched by script. Worth checking by hand: it also has Spanish (ready.gov_es_all)."),
    ("ready.gov_es_all", "", None, "verify", "As ready.gov English. Would fill the Spanish survival gap."),
    ("usda-2015_en", "", None, "verify", "USDA home canning guide; US federal work, probably public domain. Check by hand."),
    ("irp.fas.org_en_military-medicine", "", None, "verify", "US military medical manuals mirrored by FAS; probably public domain. Check by hand."),
    ("nhs.uk_en_medicines", "", None, "unclear", "Crown copyright; reuse terms for this mirror not verified."),
    ("medlineplus.gov_es_all", "", None, "unclear", "MedlinePlus mixes public-domain pages with licensed third-party content (e.g. the A.D.A.M. encyclopedia)."),
    ("cuisinelibre.org_fr_all", "", None, "verify", "French recipes, reportedly under various free licenses per recipe; site blocked the scripted check."),
    ("publicdomainrecipes.com_en_all", "", None, "verify", "Name suggests public domain or CC0; not checked."),
    ("lrnselfreliance_en_all", "", None, "not-open", "YouTube channel; standard YouTube license. Same for canadian-prepper_*, urban-prepper_en_all, s2underground_en_all, cest-pas-sorcier_fr_*."),
    ("gutenberg_en_lcc-r", "", "LicenseRef-PublicDomain-US", "safety", "License is fine, but these are old medical books with outdated, sometimes dangerous advice. Kept out of the Medical kit."),
]

# ---------------------------------------------------------------- models
MODELS = [
    {"id": "hf/unsloth/Qwen3.5-4B-GGUF/Q4_K_M", "repo": "unsloth/Qwen3.5-4B-GGUF",
     "rev": "e87f176479d0855a907a41277aca2f8ee7a09523", "file": "Qwen3.5-4B-Q4_K_M.gguf",
     "size": 2740937888, "sha256": "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
     "title": "Qwen3.5 4B Instruct, 4-bit (Q4_K_M)", "base_model": "Qwen/Qwen3.5-4B",
     "base_license_url": "https://huggingface.co/Qwen/Qwen3.5-4B/blob/main/LICENSE",
     "role": "answer", "tiers": {"standard": "default"}, "ram_peak": 3400000000,
     "decision": "D-031", "benchmark": "143/160"},
    {"id": "hf/unsloth/Qwen3.5-2B-GGUF/Q4_K_M", "repo": "unsloth/Qwen3.5-2B-GGUF",
     "rev": "f6d5376be1edb4d416d56da11e5397a961aca8ae", "file": "Qwen3.5-2B-Q4_K_M.gguf",
     "size": 1280835840, "sha256": "aaf42c8b7c3cab2bf3d69c355048d4a0ee9973d48f16c731c0520ee914699223",
     "title": "Qwen3.5 2B Instruct, 4-bit (Q4_K_M)", "base_model": "Qwen/Qwen3.5-2B",
     "base_license_url": "https://huggingface.co/Qwen/Qwen3.5-2B/blob/main/LICENSE",
     "role": "answer", "tiers": {"standard": "fallback"}, "ram_peak": 1700000000,
     "decision": "D-031", "benchmark": "114/160"},
    {"id": "hf/unsloth/gemma-4-E4B-it-GGUF/Q4_K_M", "repo": "unsloth/gemma-4-E4B-it-GGUF",
     "rev": "bfc15c382204943c3a8fff0c750b94ae2364d7a3", "file": "gemma-4-E4B-it-Q4_K_M.gguf",
     "size": 4977171584, "sha256": "85a896a047553e842f25297ee5b031d64ff30147d9c4af17b1e4b394cd1fab87",
     "title": "Gemma 4 E4B Instruct, 4-bit (Q4_K_M)", "base_model": "google/gemma-4-E4B-it",
     "base_license_url": "https://ai.google.dev/gemma/docs/gemma_4_license",
     "role": "answer", "tiers": {"enhanced": "default", "power": "default"}, "ram_peak": 4300000000,
     "decision": "D-032", "benchmark": "148/160"},
]

# ---------------------------------------------------------------- stick budget
# Space every stick needs besides the knowledge packs (bytes). Estimates.
BASE = {
    "software": (100_000_000, "NEVO binaries for Linux ARM 32/64, Linux x86, Windows"),
    "os_image": (600_000_000, "Compressed OS image for a Pi (if Seed ships one; open question)"),
    "docs": (50_000_000, "Printable setup guide, licenses and attributions"),
    "helper_models": (1_000_000_000, "Embedding model and reranker (not chosen yet; reserve)"),
    "predigest": (1_000_000_000, "Embeddings and summary cards made by Seed (reserve)"),
}
TIER_MODELS = {
    "standard": ["hf/unsloth/Qwen3.5-4B-GGUF/Q4_K_M", "hf/unsloth/Qwen3.5-2B-GGUF/Q4_K_M"],
    "enhanced": ["hf/unsloth/gemma-4-E4B-it-GGUF/Q4_K_M"],
}
STICKS = [32, 64, 128, 256]
USABLE = 0.90  # of the nominal size: real capacity, exFAT, and a safety margin
INDEX_RATIO = 0.34  # Tantivy index / text size (spikes/zim-search/RESULTS.md)


def fetch(url, dest):
    if dest.exists() and dest.stat().st_size > 0:
        return dest.read_bytes()
    req = urllib.request.Request(url, headers={"User-Agent": "nevo-catalog-draft/0 (+https://github.com)"})
    with urllib.request.urlopen(req, timeout=120) as r:
        data = r.read()
    dest.write_bytes(data)
    return data


def parse_opds(path):
    root = ET.parse(path).getroot()
    out = {}
    for e in root.findall("a:entry", NS):
        g = lambda t: (e.find(t, NS).text or "").strip() if e.find(t, NS) is not None else ""
        acq = [l for l in e.findall("a:link", NS) if l.get("rel") == "http://opds-spec.org/acquisition/open-access"]
        if not acq:
            continue
        out[(g("a:name"), g("a:flavour"))] = {
            "uuid": g("a:id").replace("urn:uuid:", ""), "title": g("a:title"), "summary": g("a:summary"),
            "lang": g("a:language"), "date": g("a:updated")[:10], "articles": int(g("a:articleCount") or 0),
            "media": int(g("a:mediaCount") or 0), "meta4": acq[0].get("href"), "size": int(acq[0].get("length") or 0),
            "tags": g("a:tags"),
        }
    return out


def text_size(pack, by_id):
    """Rough size of the text the index covers."""
    name, flav = pack["_k"]
    if pack["source_key"] == "maps":
        return 0
    if flav == "maxi":
        sib = by_id.get(pid(name, "nopic"))
        return sib["files"][0]["size"] if sib else pack["files"][0]["size"] // 2
    if pack["source_key"] == "phet":
        return 20_000_000
    return pack["files"][0]["size"]


def tier_fit(ts):
    # Proposed rule (D-047): semantic search needs embeddings, which only fit for
    # smaller packs. Keyword search plus the reranker works for every size.
    if ts > 10 * GB:
        return {"standard": "keyword", "enhanced": "keyword", "power": "keyword"}
    if ts > 2 * GB:
        return {"standard": "keyword", "enhanced": "full", "power": "full"}
    return {"standard": "full", "enhanced": "full", "power": "full"}


def main(opds_path, cache, out_path, table_path=None):
    cache = Path(cache)
    cache.mkdir(parents=True, exist_ok=True)
    opds = parse_opds(opds_path)
    packs, by_id, mirror_bases = [], {}, {}
    for name, flav, src, topics in PACKS:
        e = opds[(name, flav)]
        fname = e["meta4"].rsplit("/", 1)[-1].replace(".meta4", "")
        path = re.sub(r"^https://[^/]+/", "", e["meta4"]).replace(".meta4", "")  # zim/<dir>/<file>
        url = "https://download.kiwix.org/" + path
        sha_txt = fetch(url + ".sha256", cache / (fname + ".sha256")).decode().split()
        assert sha_txt[1].lstrip("*") == fname, (sha_txt, fname)
        meta = ET.fromstring(fetch(url + ".meta4", cache / (fname + ".meta4")))
        mf = meta.find("m:file", ML)
        m_size = int(mf.find("m:size", ML).text)
        m_sha = [h.text for h in mf.findall("m:hash", ML) if h.get("type") == "sha-256"][0]
        # OPDS rounds sizes up to whole KiB; the .meta4 has the exact size.
        assert 0 <= e["size"] - m_size < 1024, (fname, m_size, e["size"])
        e["size"] = m_size
        assert m_sha == sha_txt[0], fname
        for u in mf.findall("m:url", ML):
            base = u.text[: u.text.index(path)] if path in u.text else None
            if base:
                mirror_bases[base] = u.get("location")
        s = SOURCES[src]
        langs = [LANG.get(x, x) for x in e["lang"].split(",")]
        p = {
            "id": pid(name, flav), "kind": "zim", "title": e["title"] + (f" ({flav})" if flav else ""),
            "summary": e["summary"], "lang": "mul" if src == "maps" else langs[0], "version": fname.rsplit("_", 1)[-1][:-4],
            "date": e["date"], "topics": topics, "license": s["license"],
        }
        for k in ("license_note", "license_check", "attribution", "homepage"):
            if k in s:
                p[k] = s[k]
        p["upstream"] = {"library": "kiwix", "uuid": e["uuid"], "name": name, "flavour": flav,
                         "articles": e["articles"], "media": e["media"]}
        p["files"] = [{"name": fname, "size": e["size"], "sha256": sha_txt[0], "url": url, "mirror_set": "kiwix",
                       "path": path, "meta4": url + ".meta4", "torrent": url + ".torrent"}]
        p["source_key"], p["_k"] = src, (name, flav)
        packs.append(p)
        by_id[p["id"]] = p
    for p in packs:
        ts = text_size(p, by_id)
        if ts:
            p["derived"] = [{"kind": "search-index", "producer": "nevo-index/1", "for": p["files"][0]["name"],
                             "size_estimate": int(ts * INDEX_RATIO), "built_by": "seed", "estimated": True}]
        p["tier_fit"] = tier_fit(ts) if ts else {"standard": "full", "enhanced": "full", "power": "full"}
        if p["source_key"] == "maps":
            p["tier_fit"]["note"] = "Maps need no AI; any tier can show them."
    for p in packs:
        del p["source_key"], p["_k"]

    models = []
    for m in MODELS:
        url = f"https://huggingface.co/{m['repo']}/resolve/{m['rev']}/{m['file']}"
        models.append({
            "id": m["id"], "kind": "model", "format": "gguf", "title": m["title"], "role": m["role"],
            "license": "Apache-2.0", "license_check": "Hugging Face model card metadata for the base model and the GGUF repo",
            "license_url": m["base_license_url"], "base_model": m["base_model"], "quantized_by": m["repo"].split("/")[0],
            "tiers": m["tiers"], "ram_peak": m["ram_peak"], "decision": m["decision"],
            "benchmark_score": m["benchmark"],
            "files": [{"name": m["file"], "size": m["size"], "sha256": m["sha256"], "url": url, "mirror_set": "huggingface",
                       "path": f"{m['repo']}/resolve/{m['rev']}/{m['file']}"}],
            "verified_note": "SHA-256 matches the file used in the Phase 0 benchmark.",
        })
    msize = {m["id"]: m["files"][0]["size"] for m in models}

    def pack_bytes(ids):
        return sum(by_id[i]["files"][0]["size"] + sum(d["size_estimate"] for d in by_id[i].get("derived", [])) for i in ids)

    base = sum(v[0] for v in BASE.values())
    sizes = []
    for kit in KITS:
        for lang, variants in kit["langs"].items():
            for var, sel in variants.items():
                ids = list(dict.fromkeys(sel["packs"] + sel.get("fallback", [])))
                content = pack_bytes(ids)
                row = {"kit": kit["id"], "lang": lang, "variant": var, "content": content,
                       "native": pack_bytes(sel["packs"])}
                for tier, mids in TIER_MODELS.items():
                    total = content + base + sum(msize[x] for x in mids)
                    row[tier] = {"total": total, "stick_gb": next((s for s in STICKS if total <= s * GB * USABLE), None)}
                sizes.append(row)
    for lang in ("en", "fr", "es"):
        for var in ("compact", "full"):
            ids = []
            for kit in KITS:
                if lang in kit["langs"]:
                    sel = kit["langs"][lang][var]
                    ids += sel["packs"] + sel.get("fallback", [])
            ids = list(dict.fromkeys(ids))
            content = pack_bytes(ids)
            row = {"kit": "all-except-maps", "lang": lang, "variant": var, "content": content}
            for tier, mids in TIER_MODELS.items():
                total = content + base + sum(msize[x] for x in mids)
                row[tier] = {"total": total, "stick_gb": next((s for s in STICKS if total <= s * GB * USABLE), None)}
            sizes.append(row)

    flagged = []
    for name, flav, lic, why, note in FLAGGED:
        e = opds.get((name, flav))
        f = {"id": pid(name, flav), "reason": why, "license": lic, "note": note}
        if e:
            f.update({"title": e["title"], "lang": ",".join(LANG.get(x, x) for x in e["lang"].split(",")),
                      "size": e["size"], "date": e["date"], "url": e["meta4"].replace(".meta4", "").replace("lb.download", "download")})
        flagged.append(f)

    catalog = {
        "t": "catalog-list",
        "v": 1,
        "name": "NEVO project catalog",
        "slug": "nevo",
        "seq": 1,
        "built": BUILT,
        "update": [],
        "draft": {
            "status": "Unsigned JSON for review. The signed form is catalog.cbor inside a catalog revision (docs/spec/catalog.md).",
            "built_from": "https://library.kiwix.org/catalog/v2/entries?count=-1, each file's .sha256 and .meta4, and the Hugging Face API",
            "tool": "catalogs/tools/build_draft.py",
            "units": "Sizes in bytes. GB in notes means 10^9 bytes.",
            "missing": "BLAKE3 blob IDs (need the full files); update URLs (none yet).",
        },
        "license_refs": {
            "LicenseRef-PublicDomain-US": "Public domain in the United States (expired copyright or a US federal work). May still be protected in some other countries.",
        },
        "mirror_sets": {
            "kiwix": {"primary": "https://download.kiwix.org/",
                      "mirrors": sorted(mirror_bases), "note": "A file's URL on a mirror is the mirror base plus the file's path. Each file's .meta4 lists the live mirrors."},
            "huggingface": {"primary": "https://huggingface.co/", "mirrors": []},
        },
        "stick_budget": {"usable_fraction": USABLE, "base": {k: {"size": v[0], "what": v[1]} for k, v in BASE.items()},
                         "tier_models": TIER_MODELS, "index_ratio": INDEX_RATIO},
        "packs": packs,
        "models": models,
        "kits": KITS,
        "kit_sizes": sizes,
        "flagged": flagged,
    }
    Path(out_path).write_text(json.dumps(catalog, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{len(packs)} packs, {len(models)} models, {len(KITS)} kits, {len(flagged)} flagged; base {base/GB:.2f} GB")
    if table_path:
        lines = []
        for r in sizes:
            lines.append(f"{r['kit']}\t{r['lang']}\t{r['variant']}\t{r['content']/GB:.2f}\t{r.get('native', 0)/GB:.2f}\t"
                         f"{r['standard']['total']/GB:.1f}\t{r['standard']['stick_gb']}\t{r['enhanced']['total']/GB:.1f}\t{r['enhanced']['stick_gb']}")
        Path(table_path).write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main(*sys.argv[1:])
