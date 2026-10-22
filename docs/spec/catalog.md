# NEVO catalog format

**Status: draft 0.1, 2026-10-22. Not locked.** Open questions are marked **(open)** and collected in section 11. Proposed decisions are D-045 to D-060 in [DECISIONS.md](../../DECISIONS.md).

A catalog tells NEVO Seed what it can download: knowledge packs, AI models and kits, with their sizes, hashes, licenses and sources. It also tells a node which packs to label **Reference** (VISION: "Trust"). This spec builds on the [content model](content-model.md), section 3: a catalog is a revision with `kind: "catalog"`.

The first real catalog, as reviewable JSON, is [catalogs/nevo-catalog-draft.json](../../catalogs/nevo-catalog-draft.json). A summary for people is in [docs/kits.md](../kits.md).

## 1. Where a catalog lives

| Part | What it is |
|---|---|
| **Catalog key** | An Ed25519 key with a key record of `kind: "catalog"` (content model, section 2). It signs every revision of the catalog. Kept offline; a recovery key can revoke it. |
| **Catalog item** | The catalog's stable identity: the ID of its first revision. |
| **Catalog revision** | A normal signed revision: `kind: "catalog"`, `title` (e.g. "NEVO project catalog"), `lang: "mul"`, `topics` (every top-level topic it covers), `license` of the list itself (proposed `CC0-1.0`), and `content` with one file, `catalog.cbor`. |
| **`catalog.cbor`** | The list itself: packs, models, kits. A blob, so it is stored and checked like any other file. |

A newer catalog is a new revision of the same item, with the previous revision in `parents` (section 8).

### Why CBOR, not JSON

`catalog.cbor` uses the same **deterministic CBOR** as every other NEVO object (content model, section 1):

- **One exact byte form**, so the blob ID and the signature cover exactly one list. JSON allows many spellings of the same data (spaces, key order, `1e3` vs `1000`).
- **No new parser** on nodes: `nevo-format` already reads and checks strict CBOR.
- **Byte strings** for hashes and keys, and **no floats** (sizes are integers, as everywhere in NEVO).
- **Smaller**: the draft is 150 KB as indented JSON.

JSON is for people. Seed can export a catalog as JSON for review and import a JSON draft to sign it. The mapping is one to one: CBOR byte strings become lowercase hex, and the JSON draft's field names are the CBOR keys. A JSON file is never signed or trusted by itself. (D-045)

## 2. `catalog.cbor` top level

A map with `t: "catalog-list"` and `v: 1`, plus:

| Field | Type | Meaning |
|---|---|---|
| `name` | text | Display name, e.g. "NEVO project catalog" |
| `slug` | text | Short name used in pack IDs from this catalog, e.g. `nevo` |
| `seq` | integer | Goes up by one in every new revision. Lets Seed refuse an older list (section 8). |
| `built` | text | Date the list was built (`YYYY-MM-DD`). Information only; nodes don't trust clocks. |
| `update` | array of text, optional | URLs where Seed can fetch newer revisions (section 8) |
| `license_refs` | map | Text of every `LicenseRef-…` used in the list (section 5) |
| `mirror_sets` | map | Named groups of download servers (section 4) |
| `packs` | array | Knowledge packs (section 3) |
| `models` | array | AI models (section 6) |
| `kits` | array | Named bundles (section 7) |
| `stick_budget` | map, optional | The space reserves Seed assumes (section 9) |
| `flagged` | array, optional | Content looked at and left out, with the reason. Information only; Seed never downloads it from here. |
| `kit_sizes` | array, optional | Kit totals per language, variant and tier, as computed by the build tool. Information only; Seed recomputes them for the actual selection. |

Unknown fields are kept and ignored, as in every NEVO format.

## 3. Packs

One entry per downloadable pack. The `id` stays the same across versions; the files change.

| Field | Type | Meaning |
|---|---|---|
| `id` | text | Stable name: `<source>/<name>`, e.g. `kiwix/wikipedia_en_medicine_nopic`. For Kiwix it is the book name plus flavour, which stays the same from release to release. |
| `kind` | text | File type: `zim`, `pdf`, `warc`, `nevo` (a ready `.nevo` pack), `osm`, … |
| `title`, `summary` | text | |
| `lang` | text | BCP 47 tag (`en`, `fr`, `es`), or `mul` for language-neutral content such as maps |
| `version` | text | Upstream version, e.g. `2026-10` |
| `date` | text | Upstream release date |
| `topics` | array of text | Topic paths from the content model, section 6 |
| `license` | text | SPDX license expression (section 5) |
| `license_note` | text, optional | What the expression doesn't say: which parts have which license, what to keep visible |
| `license_check` | text, optional | How and when the maintainer checked the license |
| `attribution` | text | The credit line the license requires |
| `homepage` | text | The original project |
| `upstream` | map | Upstream identifiers, e.g. the Kiwix book UUID, name, flavour, article count |
| `files` | array | The files to download (section 4) |
| `derived` | array, optional | Data Seed builds from the files, such as the search index (section 4) |
| `tier_fit` | map | How each tier can use the pack: `full`, `keyword` or `skip` (below) |
| `requires` | array of IDs, optional | Other packs or models this pack needs |
| `withdrawn` | text, optional | Reason the pack should no longer be installed (e.g. a license problem). Seed hides it; nodes show the reason. |

**Tier fit.** Every tier can read and keyword-search every pack. Semantic search needs embeddings, which only fit for smaller packs (D-015). Proposed rule (D-047), from the text size the search index covers:

| Text size | Standard (4 GB) | Enhanced (8 GB) | Power (16 GB+) |
|---|---|---|---|
| up to 2 GB | `full` | `full` | `full` |
| 2 to 10 GB | `keyword` | `full` | `full` |
| over 10 GB | `keyword` | `keyword` | `keyword` |

`skip` means "don't offer this pack for this tier" and is not used yet. Maps need no AI and are `full` everywhere.

## 4. Files, mirrors, derived data and checks

Each entry in `files`:

| Field | Type | Meaning |
|---|---|---|
| `name` | text | File name on the stick |
| `size` | integer | Exact size in bytes |
| `sha256` | bytes | The upstream SHA-256, as published next to the file (Kiwix `.sha256`, Hugging Face LFS hash) |
| `blob` | ID | The NEVO blob ID (BLAKE3). **Required in a signed catalog** (D-048); the draft doesn't have it yet because computing it needs the whole file. |
| `url` | text | Main download URL |
| `mirror_set`, `path` | text | Name of a mirror set, and the file's path on every server in it |
| `meta4`, `torrent` | text, optional | Metalink and BitTorrent files, when the upstream publishes them |

A **mirror set** (`mirror_sets` at the top level) has a `primary` base URL and a list of `mirrors`. A file's URL on a mirror is the base plus `path`. For Kiwix, each file's `.meta4` also lists the mirrors that hold it right now, with SHA-1 hashes of every 4 MiB piece. A community catalog can add its own mirror set, for example a server in its region.

**How Seed checks a download:**

1. Free space and the stick's real capacity are checked first (VISION: "Drive handling").
2. Seed downloads from the primary or any mirror, in pieces, resuming after interruptions. A torrent is an alternative source for the same bytes.
3. When the file is complete, its **size** must match exactly, then its **SHA-256**. In the same pass Seed computes the **BLAKE3 blob ID** and, if the catalog has `blob`, compares it too.
4. On a mismatch, Seed throws the file away and tries another mirror. With a `.meta4`, it can re-fetch only the bad pieces.
5. Only checked files are written to the stick. After writing, Seed reads them back and checks the blob ID again.

**What the node checks.** A node doesn't trust whoever built or copied the stick. On import it computes each file's blob ID and looks for it in the catalogs it trusts. A match makes the pack **Reference** for that catalog's topics; no match makes it **Unknown** (D-024, D-048). This is why a signed catalog must carry the BLAKE3 `blob` and not only the SHA-256: hashing a large file twice on a Pi is slow, and BLAKE3 is the hash nodes already use.

**Derived data.** Each entry in `derived` describes something Seed builds from a file before writing the stick (D-029, D-036):

| Field | Meaning |
|---|---|
| `kind` | `search-index` (Tantivy, D-036), later `embeddings`, `summary-cards` |
| `producer` | Tool and version, e.g. `nevo-index/1`. Node and Seed must use the same one. |
| `for` | The file it is built from |
| `size_estimate` | Bytes, for stick planning. The draft uses 34% of the text size (spikes/zim-search/RESULTS.md). |
| `built_by` | `seed` (built on the online computer) or `catalog` (published ready-made, with its own `blob`) **(open)** |

On the stick, derived data travels as `derived` objects (content model, section 2), signed by the Seed that made them. Nodes may use them to save work but never trust them more than their source.

## 5. Licenses

- `license` is an [SPDX license expression](https://spdx.org/licenses/), e.g. `CC-BY-SA-4.0`. A collection whose parts have different licenses uses `AND`: `CC-BY-SA-3.0 AND CC-BY-SA-4.0`.
- Anything without an SPDX identifier uses `LicenseRef-<name>`, and the catalog's `license_refs` holds its full text. The draft uses `LicenseRef-PublicDomain-US` for Project Gutenberg.
- `license_note` says what the expression can't: for Wikipedia, that images keep their own licenses; for PhET, that the PhET name must stay visible.
- **Only open content goes into kits** (D-046): licenses that meet the [Open Definition](https://opendefinition.org/), such as CC0, CC BY, CC BY-SA, ODbL, public domain, Apache-2.0 and MIT. NonCommercial or NoDerivatives content (iFixit, TED, Khan Academy) and anything with unclear licensing stays out of kits and is listed in `flagged` with the reason.

**How licenses reach people:**

- **Seed** shows the license next to every pack, model and kit, and a kit's page lists every license in it.
- When Seed wraps a downloaded file into a NEVO pack revision, it copies `license` into the revision and fills `source` (`title`, `url`, `author` = the attribution, `retrieved` = the download date). The license then travels with the pack, even to nodes that never saw the catalog.
- **The node** shows each pack's license, attribution and source on its page, and has one "Licenses" page listing every pack and model.
- The stick's `docs/` folder holds a plain-text `LICENSES.txt` generated from the catalog, so the information survives without NEVO.

## 6. Models

Models are entries in `models`, with the pack fields that apply (`id`, `title`, `license`, `files`, …) plus:

| Field | Meaning |
|---|---|
| `kind`, `format` | `model`, `gguf` |
| `role` | `answer` (the chat model), later `embed` and `rerank` |
| `base_model`, `license_url` | The original model and its license text. The GGUF repository's own license must match. |
| `quantized_by` | Who made the GGUF file |
| `tiers` | Map of tier to `default` or `fallback`. Seed picks the model for the target RAM (VISION: "Kits"). |
| `ram_peak` | Measured peak memory in bytes with NEVO's llama.cpp settings (D-033) |
| `decision`, `benchmark_score` | Where it was chosen and how it scored |

Model files are pinned to an exact repository commit (`/resolve/<commit>/<file>`), so the URL always serves the same bytes. AI models must have permissive licenses so they can be bundled (VISION: "Licensing").

The draft lists the three models from D-031 and D-032. Their SHA-256 hashes match the files used in the Phase 0 benchmark.

| Model | File | Size | License | Tiers |
|---|---|---|---|---|
| Qwen3.5 4B, Q4_K_M | [unsloth/Qwen3.5-4B-GGUF](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF) | 2.74 GB | Apache-2.0 | Standard (default) |
| Qwen3.5 2B, Q4_K_M | [unsloth/Qwen3.5-2B-GGUF](https://huggingface.co/unsloth/Qwen3.5-2B-GGUF) | 1.28 GB | Apache-2.0 | Standard (fallback) |
| Gemma 4 E4B, Q4_K_M | [unsloth/gemma-4-E4B-it-GGUF](https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF) | 4.98 GB | Apache-2.0 | Enhanced and Power |

A Standard stick carries both Qwen models, so a node can switch to the 2B model without a new stick if the 4B model is too slow on its Pi (D-060).

## 7. Kits

A kit is a named list of packs for one purpose (VISION: Medical, Farming and food, Repair and build, Survival basics, Education, Maps).

| Field | Meaning |
|---|---|
| `id`, `title`, `about` | e.g. `medical`, "Medical", "First aid, diseases, medicines, emergency care." |
| `topics` | Topics the kit covers |
| `langs` | Map of language to variants |
| `region_choice` | For kits where the user picks a region (Maps): the packs to choose from |

Each language has two **variants**, `compact` (no pictures, smallest useful set) and `full` (pictures, larger collections). Each variant has:

- `packs`: packs in that language.
- `fallback`: English packs added because the language has little of its own. The built-in AI translates a question into the pack's language before searching (D-037), so French and Spanish users still get answers from them, in their own language. On by default, can be unticked (D-049).
- `optional`: extras Seed offers but doesn't count in the kit's size.

Kits only refer to packs in the same catalog. Packs shared by several kits (Appropedia is in three) are downloaded and stored once. **(open)** Whether kit titles need translations inside the catalog, or come from NEVO's interface translations.

## 8. Updates

**A newer catalog** is a new revision of the catalog item: `parents` holds the previous revision, `seq` is one higher, and it is signed by the same catalog key. Seed:

1. fetches the newest revision from the catalog's `update` URLs (or from a file the user gives it),
2. checks the signature against the catalog key it already has,
3. refuses a revision whose `seq` is lower than the one it has, so a mirror can't roll it back to an old list,
4. shows what changed: new packs, new versions, withdrawn packs.

**A newer pack** keeps its `id` and gets new `files` and a new `version`. When Seed wraps it, the new pack revision has the previous pack revision as its parent, so the node knows it replaces the old one. The admin decides whether the old files are purged.

**Updating a node** (VISION: Seed modes): the node exports an inventory (pack IDs, versions and blob IDs). Seed compares it with the catalog and downloads only what's missing or newer.

**Withdrawing** a pack: the catalog sets `withdrawn` with a reason, and the catalog key can sign an `outdated` or `dangerous` attestation about the pack revision (content model, section 2), which reaches nodes through normal sync.

**Upstream files disappear.** Kiwix keeps only the latest one or two versions of a file (on 2026-10-22 its directory held the `2026-04` and `2026-10` WikiMed files). A catalog revision therefore goes stale within months. **(open)** Whether the NEVO project keeps its own mirror of every file in a released catalog, publishes a new catalog revision every month, or lets Seed offer a newer upstream file that isn't in the signed catalog yet, marked Unknown.

## 9. Stick budget

Seed tells the user whether a selection fits before downloading anything. Besides the packs and their indexes, every stick needs:

| Reserve | Size | What |
|---|---|---|
| Software | 0.10 GB | NEVO for Linux ARM 32/64, Linux x86 and Windows |
| OS image | 0.60 GB | A compressed OS image for a Pi, if Seed ships one **(open)** |
| Docs | 0.05 GB | Printable setup guide, `LICENSES.txt` |
| Helper models | 1.00 GB | Embedding model and reranker, not chosen yet **(open)** |
| Pre-digest | 1.00 GB | Embeddings and summary cards made by Seed **(open: grows with content)** |
| **Total** | **2.75 GB** | |
| Standard models | 4.02 GB | Qwen3.5 4B + 2B |
| Enhanced / Power models | 4.98 GB | Gemma 4 E4B |

Only **90%** of a stick's nominal size is planned for: real capacity is a little lower than the label, exFAT needs some room, and cheap sticks vary. So a 32 GB stick holds 28.8 GB, a 64 GB stick 57.6 GB and a 128 GB stick 115.2 GB. exFAT has no 4 GB file limit, so large ZIM files need no splitting.

The results per kit and language are in [docs/kits.md](../kits.md) and in the draft's `kit_sizes`.

## 10. Community catalogs

Anyone can publish a catalog with their own key (D-025). NEVO's catalog is built into Seed; others are added by the user:

1. **From a file or URL**: a catalog revision with its key record, e.g. on a USB stick from a partner organization.
2. **From a QR code**: the catalog key (`k…`, 53 characters) plus an update URL. Seed fetches the catalog and checks it was signed by that key.

Seed shows the catalog's name, key fingerprint, the topics it covers and how many packs it has, and asks the user to confirm. The user can limit it to some topics: a medical NGO's catalog can be trusted for `medical` only. On the stick, the trust choice goes with it: a new node trusts the catalogs that signed its seed, for the topics chosen (D-024).

When two catalogs list a file with the same blob ID, it is downloaded once and both catalogs vouch for it. When they list different files under similar names, both are shown with their catalog's name. If the NEVO project disappears, community catalogs keep working, because nothing depends on a central server.

## 11. Open questions

1. **Upstream churn** (section 8): own mirror, monthly catalogs, or unverified newer files?
2. **Pre-built indexes** (section 4): should catalogs publish ready-made search indexes and embeddings, so Seed doesn't have to build them? It would make the producer version part of the catalog.
3. **OS image** (section 9): does Seed ship a Pi OS image, a NEVO image, or only NEVO binaries?
4. **Helper models**: which embedding model and reranker, and their sizes and licenses.
5. **Stick layout:** VISION's layout has `manifest`, `software/`, `models/`, `packs/`, `docs/`. The node also needs the signed catalog revisions and catalog key records to check packs offline. Proposal: a `catalogs/` folder. The `manifest` itself is a catalog revision signed by the Seed that wrote the stick, listing exactly what is on it (content model, section 3). Since nodes trust the source catalogs and not the stick's maker, who signs the manifest matters little.
6. **Maps:** Kiwix's map packs (openZIM `maps_en_*`) are OpenStreetMap data under ODbL. The licenses of the map style, fonts and viewer inside the ZIM aren't checked yet. The interface is English only; place names are in local languages.
7. **Kit translations** (section 7).
8. **Noncommercial content** (D-046): offer iFixit and similar as a clearly labeled opt-in list, or leave them out entirely?
9. **Unclear licenses** in the `flagged` list (ready.gov, USDA canning guide, US military medicine) need a person to check them; some would fill real gaps, such as Spanish survival content.
