# NEVO kits (draft)

**Draft, 2026-10-22.** What NEVO Seed will offer in its built-in catalog, in English, French and Spanish. The data, with every file's URL, size, date, SHA-256 and license, is in [catalogs/nevo-catalog-draft.json](../catalogs/nevo-catalog-draft.json). The format is in [docs/spec/catalog.md](spec/catalog.md).

All packs are Kiwix ZIM files from [download.kiwix.org](https://download.kiwix.org/zim/), as listed in the Kiwix library on 2026-10-22. Sizes are in GB (10^9 bytes).

## How to read this

- Every kit comes in two variants. **Compact** leaves out pictures and keeps the smallest useful set. **Full** adds pictures and bigger collections.
- **"+ English"** packs are added to French and Spanish kits where those languages have little content. The built-in AI translates a question into English before searching them, and answers in the user's language (D-037, D-049).
- **Only openly licensed content** is in kits (D-046). What was left out, and why, is at the end.
- A pack can be in several kits (Appropedia is in three). It is downloaded once.

## Which stick you need

A stick also holds NEVO itself, a reserve for helper models and pre-digested data (2.75 GB together), and the AI models for the tier: **4.0 GB on Standard** (Qwen3.5 4B plus the 2B fallback) or **5.0 GB on Enhanced and Power** (Gemma 4 E4B). Only 90% of a stick's label is planned for: 28.8 GB of a 32 GB stick, 57.6 GB of 64 GB, 115.2 GB of 128 GB.

Content sizes below include an estimated search index (about a third of the text, built by Seed). French and Spanish sizes include their English packs.

| Kit | Variant | English | French | Spanish |
|---|---|---|---|---|
| Medical | compact | 1.9 GB | 2.3 GB | 2.1 GB |
| | full | 5.4 GB | 5.5 GB | 5.1 GB |
| Farming and food | compact | 2.4 GB | 2.4 GB (English only) | 2.4 GB (English only) |
| | full | 8.5 GB | 2.4 GB (English only) | 2.4 GB (English only) |
| Repair and build | compact | 1.7 GB | 2.1 GB | 1.8 GB |
| | full | 26.7 GB | 2.9 GB | 2.1 GB |
| Survival basics | compact | 1.3 GB | 1.2 GB | 1.0 GB |
| | full | 3.7 GB | 1.9 GB | 1.4 GB |
| Education | compact | 8.5 GB | 2.7 GB | 1.9 GB |
| | full | 92.6 GB | 41.1 GB | 22.4 GB |
| **All five kits** | compact | **14.4 GB** | **8.5 GB** | **7.5 GB** |
| | full | **128.3 GB** | **50.3 GB** | **31.0 GB** |
| Maps | one region | 1.7 to 26.7 GB (Africa: 8.1 GB) | same | same |
| | whole world | 77.7 GB | same | same |

**Smallest stick for each tier** (content + NEVO + reserves + models):

| Selection | English, Standard | English, Enhanced | French, Standard | French, Enhanced | Spanish, Standard | Spanish, Enhanced |
|---|---|---|---|---|---|---|
| Any single kit, compact | 32 GB | 32 GB | 32 GB | 32 GB | 32 GB | 32 GB |
| Any single kit, full, except Education and English Repair | 32 GB | 32 GB | 32 GB | 32 GB | 32 GB | 32 GB |
| All five kits, compact | 32 GB (21.2) | 32 GB (22.1) | 32 GB (15.3) | 32 GB (16.2) | 32 GB (14.2) | 32 GB (15.2) |
| All five compact + Africa map | 64 GB (29.3) | 64 GB (30.2) | 32 GB (23.4) | 32 GB (24.3) | 32 GB (22.4) | 32 GB (23.3) |
| Education, full | 128 GB (99.4) | 128 GB (100.4) | 64 GB (47.9) | 64 GB (48.8) | 64 GB (29.2) | 64 GB (30.1) |
| All five kits, full | 256 GB (135.0) | 256 GB (136.0) | 64 GB (57.1) | 128 GB (58.1) | 64 GB (37.8) | 64 GB (38.8) |
| World map alone | 128 GB (84.5) | 128 GB (85.5) | | | | |

Numbers in brackets are the total GB on the stick. **A 64 GB stick is the sensible default**: it holds every compact kit plus a regional map in any of the three languages, with room left for local content. A 32 GB stick is enough for one language's compact kits without maps. Full English needs 256 GB, mostly for full English Wikipedia (52.7 GB plus an index of about 18 GB).

## Medical

| Pack | Lang | Size | License | Good for | In |
|---|---|---|---|---|---|
| WikiMed, no pictures | en | 1.05 GB | CC BY-SA 4.0 | About 104,000 medical Wikipedia articles: diseases, drugs, anatomy, first aid | compact |
| WikiMed, with pictures | en | 1.86 GB | CC BY-SA 4.0 | The same, with images | full |
| MDWiki | en | 2.30 GB | CC BY-SA 4.0 | Health articles curated by WikiProjectMed | full |
| WikEM | en | 0.35 / 0.37 GB | CC BY-SA 4.0 | Emergency medicine: what to do, step by step | compact / full |
| WikiMed (Encyclopédie médicale) | fr | 0.32 / 0.86 GB | CC BY-SA 4.0 | French medical Wikipedia | compact / full |
| Wikipedia Médica | es | 0.17 / 0.47 GB | CC BY-SA 4.0 | Spanish medical Wikipedia | compact / full |
| + English | fr, es | | | WikiMed and WikEM compact; MDWiki in full | |

## Farming and food

| Pack | Lang | Size | License | Good for | In |
|---|---|---|---|---|---|
| Gardening & Landscaping Q&A | en | 0.93 GB | CC BY-SA 2.5–4.0 | Practical answers on growing plants, pests, soil | compact |
| Cooking Q&A | en | 0.24 GB | CC BY-SA 2.5–4.0 | Cooking, food storage and food safety questions | compact |
| Gardenology | en | 0.11 GB | CC BY-SA 3.0 | Plant encyclopedia | compact |
| Appropedia | en | 0.58 GB | CC BY-SA 3.0 and 4.0 | Appropriate technology: water, sanitation, food, building | compact |
| Project Gutenberg, agriculture (LCC S) | en | 4.53 GB | Public domain (US) | Old farming, gardening and animal books. Dated but often still useful. | full |
| French, Spanish | | | | **No open native content found**; the English packs are used | |

## Repair and build

| Pack | Lang | Size | License | Good for | In |
|---|---|---|---|---|---|
| Appropedia | en | 0.58 GB | CC BY-SA 3.0 and 4.0 | Building, water, energy and sanitation projects | compact |
| energypedia | en | 0.72 / 0.80 GB | CC BY-SA 4.0 | Solar, biogas, cookstoves, mini-grids | compact / full |
| Restarters wiki | en | 0.02 GB | CC BY-SA 4.0 | How things work and how to fix them | compact |
| Wikibooks | en | 3.51 / 6.18 GB | CC BY-SA 4.0 | Textbooks and how-to books (electronics, bicycles, …) | optional / full |
| Project Gutenberg, technology (LCC T) | en | 13.15 GB | Public domain (US) | Old engineering, building and crafts books | full |
| Wikilivres | fr | 0.33 / 1.15 GB | CC BY-SA 4.0 | French Wikibooks | compact / full |
| Wikilibros | es | 0.11 / 0.35 GB | CC BY-SA 4.0 | Spanish Wikibooks | compact / full |
| + English | fr, es | | | Appropedia, energypedia, Restarters | |

iFixit (the largest repair guide collection, also in French and Spanish) is **left out** because it is NonCommercial (see below).

## Survival basics

The small first kit: first aid, water, shelter, knots, local information.

| Pack | Lang | Size | License | Good for | In |
|---|---|---|---|---|---|
| WikiMed, introductions only | en | 0.17 GB | CC BY-SA 4.0 | Short medical summaries | compact |
| WikiMed, no pictures + WikEM | en | 1.05 + 0.35 GB | CC BY-SA 4.0 | Full medical articles, emergency steps | full |
| Wikivoyage | en | 0.27 / 1.07 GB | CC BY-SA 4.0 | Places, getting around, staying safe | compact / full |
| Knots by Wikipedia | en | 0.01 GB | CC BY-SA 4.0 | Knots and their uses | compact |
| Appropedia | en | 0.58 GB | CC BY-SA 3.0 and 4.0 | Water treatment, shelter, sanitation | compact |
| WikiMed + Wikivoyage | fr | 0.32 + 0.07 / 0.86 + 0.25 GB | CC BY-SA 4.0 | French medical and travel content | compact / full |
| Wikipedia Médica + Wikiviajes | es | 0.17 + 0.04 / 0.47 + 0.14 GB | CC BY-SA 4.0 | Spanish medical and travel content | compact / full |
| + English | fr, es | | | Knots, Appropedia | |

## Education

| Pack | Lang | Size | License | Good for | In |
|---|---|---|---|---|---|
| Best of Wikipedia, no pictures | en / fr / es | 2.75 / 1.48 / 1.21 GB | CC BY-SA 4.0 | The most-read articles, complete | compact |
| Wikipedia, introductions only | en / fr / es | 14.39 / 3.58 / 4.11 GB | CC BY-SA 4.0 | Every article's introduction | optional |
| Wikipedia, no pictures | en / fr / es | 52.69 / 12.91 / 13.30 GB | CC BY-SA 4.0 | All of Wikipedia, text only | full |
| PhET simulations | en / fr / es | 0.11 GB each | CC BY 4.0 | Interactive math and science for schools | both |
| Vikidia | en / fr / es | 0.01 / 0.10 / 0.02 GB (no pictures) | CC BY-SA 3.0 | Encyclopedia for children aged 8–13 | compact (full: with pictures) |
| Wikibooks | en / fr / es | 3.51 / 0.33 / 0.11 GB (no pictures) | CC BY-SA 4.0 | Open textbooks | compact (full: with pictures) |
| Wikiversity, no pictures | en / fr / es | 1.63 / 0.18 / 0.02 GB | CC BY-SA 4.0 | Courses and learning materials | full |
| Wiktionary, no pictures | en / fr / es | 9.16 / 4.94 / 0.66 GB | CC BY-SA 4.0 | Dictionary | full |
| Project Gutenberg | fr / es | 10.53 / 1.86 GB | Public domain (US) | Classic books | full |
| Wikisource, no pictures | es | 0.40 GB | CC BY-SA 4.0 | Source texts | full |

The full English Gutenberg library (221 GB) is too big for any kit; Seed can offer its subject parts (LCC A–Z, 0.01–40 GB each) separately.

## Maps

OpenStreetMap maps with roads, places and place search (openZIM `maps` packs), licensed **ODbL 1.0** ("© OpenStreetMap contributors"). Place names are in local languages; the viewer is in English. Seed asks for one region:

| Region | Size | Region | Size |
|---|---|---|---|
| Central America | 1.73 GB | Asia | 21.08 GB |
| South America | 5.46 GB | North America | 22.65 GB |
| Africa | 8.11 GB | Europe | 26.70 GB |
| Australia and Oceania | 8.58 GB | **World** | **77.73 GB** |

Kiwix also has single countries (e.g. Haiti, Nigeria); a later catalog can list them. Licenses of the map style and fonts inside the ZIM are not checked yet.

## AI models

| Tier | Model | File size | License | Source |
|---|---|---|---|---|
| Standard (4 GB) | Qwen3.5 4B, Q4_K_M | 2.74 GB | Apache-2.0 | [unsloth/Qwen3.5-4B-GGUF](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF) |
| Standard fallback | Qwen3.5 2B, Q4_K_M | 1.28 GB | Apache-2.0 | [unsloth/Qwen3.5-2B-GGUF](https://huggingface.co/unsloth/Qwen3.5-2B-GGUF) |
| Enhanced and Power | Gemma 4 E4B, Q4_K_M | 4.98 GB | Apache-2.0 | [unsloth/gemma-4-E4B-it-GGUF](https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF) |

These are the exact files used in the Phase 0 benchmark (same SHA-256).

## Left out, and why

Seen in the Kiwix library but not in any kit. Details are in the draft's `flagged` list.

| Content | Why | Notes |
|---|---|---|
| iFixit (en, fr, es; 3.6 GB each) | CC BY-NC-SA 3.0: NonCommercial | Excellent repair guides. Could be an opt-in extra, clearly labeled (open question in D-046). |
| Khan Academy (100–180 GB) | CC BY-NC-SA: NonCommercial | Also very large, from 2023 |
| TED collections (medicine, agriculture, farming, food, water) | CC BY-NC-ND: NonCommercial, NoDerivatives | |
| WikiVet | CC BY-NC-ND 3.0 | |
| LibreTexts (medicine, biology, K-12, …) | Each book has its own license, many NonCommercial | Could be split by book later |
| zimgit collections (water, medicine, knots, food, post-disaster) | PDFs from many sources, licenses unknown | Very relevant topics; each document would need checking |
| CD3WD, Survivor Library | Licenses not stated or not verified | Survivor Library is also 220 GB |
| Ready.gov (en, es), USDA canning guide, US military medicine | US federal works, probably public domain, but not verified and with some third-party material | **Worth checking by hand**: Ready.gov Spanish would fill the Spanish survival gap |
| NHS medicines A–Z, MedlinePlus Spanish | Crown copyright; MedlinePlus includes licensed third-party content | |
| Cuisine libre (fr), Public Domain Recipes | Licenses not verified | Cuisine libre would add French food content |
| Prepper and other YouTube channels, C'est pas sorcier | Standard YouTube license, not open | |
| Gutenberg medicine (LCC R) | License fine, but old medical books with outdated, sometimes dangerous advice | Kept out of the Medical kit on purpose |

## Gaps

- **French and Spanish farming and food:** no openly licensed native packs in the Kiwix library. Candidates to check: Cuisine libre, FAO publications (many are CC BY-NC-SA, so NonCommercial), Wikibooks cookbooks, or new packs built with Seed's website capture.
- **French and Spanish repair and survival:** only Wikibooks and Wikivoyage; English packs fill in. Ready.gov Spanish needs a license check.
- **Open textbooks:** OpenStax (named in VISION) is not in the Kiwix library; Seed would need to fetch its PDFs directly.
- **Maps:** fine for coverage, but only an English interface, and the style and font licenses need checking.
- **Other languages** (Arabic, Portuguese, Swahili, Hindi, …): PhET has about 100 languages and Wikipedia many; the rest is thin. Next catalog round.
