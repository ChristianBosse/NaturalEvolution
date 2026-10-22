use nevo_search::*;

/// (key, source, lang, title, aliases, topics, body)
type Fixture = (
    &'static str,
    Source,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static str,
);

const LIBRARY: &[Fixture] = &[
    (
        "zim:med/Myocardial_infarction",
        Source::Zim,
        "en",
        "Myocardial infarction",
        &["Heart attack", "Heart attacks", "Acute myocardial infarction", "MI"],
        &["medical"],
        "A myocardial infarction happens when blood flow to part of the heart muscle stops and the muscle is \
         damaged. The most common symptom is chest pain that may spread to the shoulder, arm, back, neck or jaw. \
         Other symptoms are shortness of breath, nausea, feeling faint and a cold sweat. Most cases are caused by \
         coronary artery disease. Treatment includes aspirin and restoring blood flow quickly.",
    ),
    (
        "zim:med/Panic_attack",
        Source::Zim,
        "en",
        "Panic attack",
        &["Panic attacks"],
        &["medical/mental-health"],
        "Panic attacks are sudden periods of intense fear. Symptoms include palpitations, sweating, chest pain, \
         shortness of breath and a feeling of losing control. Many people having a panic attack fear they are having \
         a heart attack, but a panic attack does not damage the heart. Attacks usually peak within ten minutes.",
    ),
    (
        "zim:med/Bleeding",
        Source::Zim,
        "en",
        "Bleeding",
        &["Hemorrhage", "Haemorrhage", "Blood loss"],
        &["medical/first-aid"],
        "Bleeding is the loss of blood from the circulatory system. Severe bleeding can cause shock and death \
         within minutes. To stop severe bleeding, press firmly on the wound with a clean cloth and keep pressing. \
         If bleeding from an arm or leg does not stop with direct pressure, a tourniquet can be applied above the \
         wound. Raise the injured part and keep the person warm.",
    ),
    (
        "zim:med/First_aid",
        Source::Zim,
        "en",
        "First aid",
        &["First-aid"],
        &["medical/first-aid"],
        "First aid is the help given to a sick or injured person until full medical treatment is available. It \
         covers checking breathing, stopping bleeding, treating burns, splinting broken bones and caring for bites \
         and stings. Every household should keep a first aid kit. First aid training is offered by many groups.",
    ),
    (
        "zim:med/Snakebite",
        Source::Zim,
        "en",
        "Snakebite",
        &["Snakebites", "Ophidism"],
        &["medical/first-aid"],
        "A snakebite is an injury caused by the bite of a snake. Venomous snakes inject venom that can cause pain, \
         swelling, bleeding and paralysis. Keep the person calm and still, remove rings and tight clothing, and get \
         to a health centre quickly. Do not cut the wound, suck out the venom or apply ice. Antivenom is the main \
         treatment.",
    ),
    (
        "zim:med/Snake",
        Source::Zim,
        "en",
        "Snake",
        &["Snakes", "Serpentes"],
        &["general"],
        "Snakes are long, legless reptiles. Most snake species are not venomous. Snakes eat rodents, birds and \
         eggs. A few species bite humans when threatened. Snake skins are shed several times a year.",
    ),
    (
        "zim:med/Insect_bites_and_stings",
        Source::Zim,
        "en",
        "Insect bites and stings",
        &["Insect bite", "Bee sting"],
        &["medical/first-aid"],
        "Most insect bites and stings cause only mild pain and swelling. First aid for a bee sting: scrape out the \
         stinger, wash the area and apply a cold pack. Get help at once if the person has trouble breathing after \
         a bite or sting.",
    ),
    (
        "zim:med/Diarrhea",
        Source::Zim,
        "en",
        "Diarrhea",
        &["Diarrhoea", "Loose stools"],
        &["medical/infections"],
        "Diarrhea is having at least three loose or liquid bowel movements each day. It often lasts a few days and \
         can cause dehydration. The most common cause is an infection of the intestines by a virus, bacteria or \
         parasites. Drinking oral rehydration solution and continuing to eat are the main treatments. Children \
         with diarrhea should also receive zinc.",
    ),
    (
        "zim:med/Oral_rehydration_therapy",
        Source::Zim,
        "en",
        "Oral rehydration therapy",
        &["Oral rehydration solution", "ORS"],
        &["medical"],
        "Oral rehydration therapy is a way of replacing fluids to prevent and treat dehydration, especially from \
         diarrhea. It means drinking water with small amounts of sugar and salts. A simple home solution is six \
         level teaspoons of sugar and half a level teaspoon of salt in one litre of clean water.",
    ),
    (
        "zim:med/Dehydration",
        Source::Zim,
        "en",
        "Dehydration",
        &["Fluid loss"],
        &["medical"],
        "Dehydration is a lack of total body water. Signs include thirst, dark urine, a dry mouth and, in children, \
         sunken eyes and no tears when crying. Mild dehydration is treated by drinking fluids; severe dehydration \
         needs fluids given through a vein.",
    ),
    (
        "zim:med/Hypothermia",
        Source::Zim,
        "en",
        "Hypothermia",
        &["Low body temperature"],
        &["medical/first-aid"],
        "Hypothermia is a body temperature below 35 degrees. Symptoms include shivering, confusion and slow \
         breathing. Move the person to a warm, dry place, remove wet clothing and warm the body slowly with \
         blankets. Warm sweet drinks help if the person is awake.",
    ),
    (
        "lib:sand-filter",
        Source::Library,
        "en",
        "Build a sand water filter",
        &[],
        &["water/treatment"],
        "A slow sand filter removes most germs from river water. Fill a barrel with layers of gravel and fine sand \
         and let the water pass through slowly. Boil or chlorinate the filtered water before drinking it.",
    ),
    (
        "lib:unsafe-markup",
        Source::Library,
        "en",
        "Wound care <script>alert(1)</script>",
        &[],
        &["medical/first-aid"],
        "<script>alert('x')</script> Clean every wound & cover it. \"Bleeding\" that won't stop needs pressure.",
    ),
    (
        "zim:med_fr/Diarrhée",
        Source::Zim,
        "fr",
        "Diarrhée",
        &["Diarrhées"],
        &["medical/infections"],
        "La diarrhée est l'émission de selles trop liquides et trop fréquentes. Elle provoque souvent une \
         déshydratation, surtout chez les jeunes enfants. Le traitement repose sur les solutions de réhydratation \
         orale.",
    ),
    (
        "zim:med_fr/Morsure_de_serpent",
        Source::Zim,
        "fr-FR",
        "Morsure de serpent",
        &[],
        &["medical/first-aid"],
        "Une morsure de serpent venimeux est une urgence. Il faut rassurer la victime, immobiliser le membre mordu \
         et l'emmener rapidement vers un centre de santé. Ne pas inciser la plaie.",
    ),
    (
        "zim:med_fr/Hémorragie",
        Source::Zim,
        "fra",
        "Hémorragie",
        &["Saignement"],
        &["medical/first-aid"],
        "Une hémorragie est un écoulement de sang hors des vaisseaux. Pour arrêter un saignement abondant, \
         comprimez fortement la plaie avec un linge propre.",
    ),
    (
        "zim:med_es/Diarrea",
        Source::Zim,
        "es",
        "Diarrea",
        &[],
        &["medical/infections"],
        "La diarrea es la evacuación de heces líquidas tres o más veces al día. Puede causar deshidratación grave \
         en los niños. Se trata con sales de rehidratación oral.",
    ),
    (
        "zim:med_es/Mordedura_de_serpiente",
        Source::Zim,
        "es",
        "Mordedura de serpiente",
        &[],
        &["medical/first-aid"],
        "Las mordeduras de serpientes venenosas causan dolor, hinchazón y hemorragias. Mantenga a la persona \
         tranquila y llévela a un centro de salud.",
    ),
    (
        "zim:med_es/Hemorragia",
        Source::Zim,
        "es-MX",
        "Hemorragia",
        &[],
        &["medical/first-aid"],
        "Una hemorragia es la salida de sangre de los vasos sanguíneos. Las hemorragias graves pueden causar la \
         muerte si no se detienen.",
    ),
    (
        "lib:kuhara",
        Source::Library,
        "sw",
        "Kuhara kwa watoto",
        &[],
        &["medical/child"],
        "Kuhara ni kutoa kinyesi cha majimaji mara tatu au zaidi kwa siku. Mpe mtoto maji mengi na ORS.",
    ),
];

fn document(f: &Fixture) -> Document {
    let (key, source, lang, title, aliases, topics, body) = *f;
    Document {
        key: key.into(),
        source,
        title: title.into(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        lang: lang.into(),
        topics: topics.iter().map(|s| s.to_string()).collect(),
        body: body.into(),
    }
}

/// Passages as an extractor would make them: one per two sentences.
fn passages(f: &Fixture) -> Vec<Passage> {
    let sentences: Vec<&str> = f.6.split_inclusive(". ").collect();
    let headings = ["Overview", "Signs", "Treatment", "More"];
    (sentences.chunks(2).enumerate())
        .map(|(i, chunk)| Passage {
            heading: headings[i.min(3)].into(),
            text: chunk.concat().trim().into(),
        })
        .collect()
}

fn library() -> SearchIndex {
    let index = SearchIndex::in_memory(Config::default()).unwrap();
    for f in LIBRARY {
        index.put(&document(f), &passages(f)).unwrap();
    }
    index.commit().unwrap();
    index
}

fn titles(hits: &[Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.title.as_str()).collect()
}

fn search(index: &SearchIndex, query: &str) -> Vec<Hit> {
    index.search(query, &Filter::default(), 10).unwrap()
}

// ---------------------------------------------------------------------------------------------
// Ranking: the spike's problem queries

#[test]
fn heart_attack_finds_the_alias_first() {
    let index = library();
    for query in ["heart attack", "Heart Attack?", "heart_attack", "HEART  ATTACKS"] {
        let hits = search(&index, query);
        assert_eq!(hits[0].title, "Myocardial infarction", "{query}: {:?}", titles(&hits));
        assert!(!hits[0].fuzzy);
        assert_eq!(hits[1].title, "Panic attack", "{query}");
    }
}

#[test]
fn stop_words_are_ignored() {
    let index = library();
    let hits = search(&index, "how to stop severe bleeding");
    assert_eq!(hits[0].title, "Bleeding", "{:?}", titles(&hits));
}

#[test]
fn snake_bite_finds_snakebite() {
    let index = library();
    let hits = search(&index, "snake bite first aid");
    let rank = titles(&hits).iter().position(|t| *t == "Snakebite");
    assert!(rank.is_some_and(|r| r < 3), "{:?}", titles(&hits));
    assert_eq!(search(&index, "snake bite")[0].title, "Snakebite");
}

#[test]
fn other_spike_queries() {
    let index = library();
    for (query, top) in [
        ("oral rehydration solution", "Oral rehydration therapy"),
        ("hypothermia treatment", "Hypothermia"),
        ("dehydration in children", "Dehydration"),
        ("tourniquet", "Bleeding"),
    ] {
        let hits = search(&index, query);
        assert_eq!(hits[0].title, top, "{query}: {:?}", titles(&hits));
    }
}

#[test]
fn title_beats_body() {
    let index = library();
    // "dehydration" is in the bodies of Diarrhea and ORT too.
    let hits = search(&index, "dehydration");
    assert_eq!(hits[0].title, "Dehydration");
    assert!(hits.len() >= 3);
}

// ---------------------------------------------------------------------------------------------
// Languages

#[test]
fn french_is_stemmed_and_folded() {
    let index = library();
    let hits = search(&index, "morsures de serpents");
    assert_eq!(hits[0].title, "Morsure de serpent", "{:?}", titles(&hits));
    // Plural. Without the accent, only the typo fallback finds it.
    let hits = search(&index, "diarrhées");
    assert_eq!(hits[0].title, "Diarrhée", "{:?}", titles(&hits));
    assert!(!hits[0].fuzzy);
    let hits = search(&index, "diarrhees");
    assert_eq!(hits[0].title, "Diarrhée", "{:?}", titles(&hits));
    let hits = search(&index, "arrêter les saignements abondants");
    assert_eq!(hits[0].title, "Hémorragie", "{:?}", titles(&hits));
}

#[test]
fn spanish_is_stemmed() {
    let index = library();
    let hits = search(&index, "hemorragias graves");
    assert_eq!(hits[0].title, "Hemorragia", "{:?}", titles(&hits));
    let hits = search(&index, "mordedura de serpientes venenosas");
    assert_eq!(hits[0].title, "Mordedura de serpiente", "{:?}", titles(&hits));
    let hits = search(&index, "deshidratación");
    assert_eq!(hits[0].title, "Diarrea", "{:?}", titles(&hits));
}

#[test]
fn unknown_languages_still_match_words() {
    let index = library();
    let hits = search(&index, "kuhara");
    assert_eq!(hits[0].key, "lib:kuhara");
    assert_eq!(hits[0].lang, "sw");
}

#[test]
fn language_filter() {
    let index = library();
    for lang in ["fr", "fr-CA", "fra", "FR"] {
        let filter = Filter {
            lang: Some(lang.into()),
            ..Filter::default()
        };
        let hits = index.search("diarrhée diarrhea diarrea", &filter, 10).unwrap();
        assert_eq!(titles(&hits), ["Diarrhée"], "{lang}");
    }
    let filter = Filter {
        lang: Some("es".into()),
        ..Filter::default()
    };
    let hits = index.search("hemorragia", &filter, 10).unwrap();
    assert_eq!(titles(&hits), ["Hemorragia", "Mordedura de serpiente"]);
}

// ---------------------------------------------------------------------------------------------
// Filters

#[test]
fn source_filter() {
    let index = library();
    let filter = Filter {
        source: Some(Source::Library),
        ..Filter::default()
    };
    let hits = index.search("water", &filter, 10).unwrap();
    assert_eq!(titles(&hits), ["Build a sand water filter"]);
    assert_eq!(hits[0].source, Source::Library);
    assert_eq!(hits[0].topics, ["water/treatment"]);
    let filter = Filter {
        source: Some(Source::Zim),
        ..Filter::default()
    };
    assert!(index
        .search("water", &filter, 10)
        .unwrap()
        .iter()
        .all(|h| h.source == Source::Zim));
}

#[test]
fn topic_filter_includes_sub_topics_only() {
    let index = SearchIndex::in_memory(Config::default()).unwrap();
    for (key, topics) in [
        ("a", &["medical/first-aid"][..]),
        ("b", &["medical"]),
        ("c", &["medical-x/foo"]),
        ("d", &["water/treatment"]),
        ("e", &["Medical/Dental/"]),
        ("f", &["water", "medical/child"]),
        ("g", &[]),
    ] {
        let doc = Document {
            key: key.into(),
            source: Source::Library,
            title: format!("Guide {key}"),
            aliases: vec![],
            lang: "en".into(),
            topics: topics.iter().map(|s| s.to_string()).collect(),
            body: "common words for every guide".into(),
        };
        index.put(&doc, &[]).unwrap();
    }
    index.commit().unwrap();
    let keys = |topics: &[&str]| {
        let filter = Filter {
            topics: topics.iter().map(|s| s.to_string()).collect(),
            ..Filter::default()
        };
        let mut keys: Vec<String> = index
            .search("guide", &filter, 10)
            .unwrap()
            .into_iter()
            .map(|h| h.key)
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(keys(&["medical"]), ["a", "b", "e", "f"]);
    assert_eq!(keys(&["medical/first-aid"]), ["a"]);
    assert_eq!(keys(&["/Medical/"]), ["a", "b", "e", "f"]);
    assert_eq!(keys(&["medical/dental"]), ["e"]);
    assert_eq!(keys(&["medical-x"]), ["c"]);
    assert!(keys(&["med"]).is_empty());
    assert!(keys(&["medical/first"]).is_empty());
    assert_eq!(keys(&["water", "medical-x"]), ["c", "d", "f"]);
    assert_eq!(keys(&[]).len(), 7);
    assert_eq!(keys(&["", "/"]).len(), 7, "empty paths are ignored");
}

#[test]
fn filters_combine_and_do_not_change_ranking() {
    let index = library();
    let all = search(&index, "bleeding");
    let filter = Filter {
        source: Some(Source::Zim),
        topics: vec!["medical/first-aid".into()],
        lang: Some("en".into()),
    };
    let filtered = index.search("bleeding", &filter, 10).unwrap();
    assert_eq!(filtered[0].title, "Bleeding");
    let expected: Vec<&Hit> = all
        .iter()
        .filter(|h| {
            h.source == Source::Zim && h.lang == "en" && h.topics.iter().any(|t| t.starts_with("medical/first-aid"))
        })
        .collect();
    assert_eq!(filtered.len(), expected.len());
    for (f, e) in filtered.iter().zip(expected) {
        assert_eq!(f.key, e.key);
        assert_eq!(f.score, e.score);
    }
}

// ---------------------------------------------------------------------------------------------
// Typos

#[test]
fn fuzzy_fallback_fixes_typos() {
    let index = library();
    for (typo, title) in [
        ("diarhea", "Diarrhea"),
        ("hypotermia", "Hypothermia"),
        ("bleding", "Bleeding"),
    ] {
        let hits = search(&index, typo);
        assert_eq!(
            hits.first().map(|h| h.title.as_str()),
            Some(title),
            "{typo}: {:?}",
            titles(&hits)
        );
        assert!(hits[0].fuzzy);
    }
    // Normal hits first, typo hits after.
    let hits = search(&index, "kuhara diarhea");
    assert_eq!(hits[0].key, "lib:kuhara");
    assert!(!hits[0].fuzzy);
    assert!(hits.iter().any(|h| h.title == "Diarrhea" && h.fuzzy));
}

#[test]
fn fuzzy_is_limited() {
    let index = library();
    // Short words don't get typo matching: "snak" is not "snake".
    assert!(search(&index, "snak").is_empty());
    // Enough normal hits: no typo matching.
    assert!(search(&index, "bleeding").iter().all(|h| !h.fuzzy));
    // Can be turned off.
    let strict = SearchIndex::in_memory(Config {
        fuzzy_below: 0,
        ..Config::default()
    })
    .unwrap();
    let f = &LIBRARY[7];
    strict.put(&document(f), &[]).unwrap();
    strict.commit().unwrap();
    assert!(strict.search("diarhea", &Filter::default(), 10).unwrap().is_empty());
    assert_eq!(strict.search("diarrhea", &Filter::default(), 10).unwrap().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Passages

#[test]
fn passages_return_quotable_text() {
    let index = library();
    let hits = index.search_passages("tourniquet", &Filter::default(), 5).unwrap();
    assert_eq!(hits[0].key, "zim:med/Bleeding");
    let expected = &passages(&LIBRARY[2])[hits[0].number as usize];
    assert!(hits[0].text.contains("tourniquet"));
    assert_eq!(hits[0].text, expected.text);
    assert_eq!(hits[0].heading, expected.heading);
    assert_eq!(
        index.num_passages(),
        LIBRARY.iter().map(|f| passages(f).len() as u64).sum::<u64>()
    );
}

#[test]
fn passages_rank_like_documents() {
    let index = library();
    let hits = index.search_passages("heart attack", &Filter::default(), 5).unwrap();
    assert_eq!(
        hits[0].key, "zim:med/Myocardial_infarction",
        "exact alias match goes first"
    );
    let hits = index
        .search_passages("how to stop severe bleeding", &Filter::default(), 3)
        .unwrap();
    assert_eq!(hits[0].key, "zim:med/Bleeding");
    assert!(hits[0].text.contains("stop severe bleeding"));
    let filter = Filter {
        lang: Some("fr".into()),
        ..Filter::default()
    };
    let hits = index.search_passages("saignement", &filter, 3).unwrap();
    assert_eq!(hits[0].key, "zim:med_fr/Hémorragie");
    let hits = index.search_passages("diarhea", &Filter::default(), 3).unwrap();
    assert!(hits.iter().any(|h| h.key == "zim:med/Diarrhea" && h.fuzzy));
}

// ---------------------------------------------------------------------------------------------
// Updates

fn guide(key: &str, title: &str, body: &str) -> Document {
    Document {
        key: key.into(),
        source: Source::Library,
        title: title.into(),
        aliases: vec![],
        lang: "en".into(),
        topics: vec!["repair/tools".into()],
        body: body.into(),
    }
}

fn passage(text: &str) -> Passage {
    Passage {
        heading: "Steps".into(),
        text: text.into(),
    }
}

#[test]
fn put_replaces_by_key() {
    let index = SearchIndex::in_memory(Config::default()).unwrap();
    index
        .put(
            &guide("g1", "Sharpen an axe", "Use a flat file."),
            &[passage("one"), passage("two file")],
        )
        .unwrap();
    index
        .put(&guide("g2", "Sharpen a knife", "Use a whetstone."), &[passage("stone")])
        .unwrap();
    index.commit().unwrap();
    assert_eq!(index.num_docs(), 2);
    assert_eq!(index.num_passages(), 3);

    index
        .put(
            &guide("g1", "Sharpen a machete", "Use a grinder."),
            &[passage("grinder steps")],
        )
        .unwrap();
    // Not visible before the commit.
    assert_eq!(search(&index, "axe").len(), 1);
    index.commit().unwrap();
    assert!(search(&index, "axe").is_empty());
    assert!(index.search_passages("file", &Filter::default(), 5).unwrap().is_empty());
    let hits = search(&index, "machete");
    assert_eq!(titles(&hits), ["Sharpen a machete"]);
    assert_eq!(index.num_docs(), 2);
    assert_eq!(index.num_passages(), 2);
    let p = index.search_passages("grinder", &Filter::default(), 5).unwrap();
    assert_eq!((p[0].key.as_str(), p[0].number), ("g1", 0));

    // Replacing twice before one commit keeps only the last version.
    index.put(&guide("g2", "Version one", "alpha"), &[]).unwrap();
    index.put(&guide("g2", "Version two", "beta"), &[]).unwrap();
    index.commit().unwrap();
    assert!(search(&index, "alpha").is_empty());
    assert_eq!(titles(&search(&index, "beta")), ["Version two"]);
    assert_eq!(index.num_docs(), 2);
}

#[test]
fn delete_removes_document_and_passages() {
    let index = SearchIndex::in_memory(Config::default()).unwrap();
    index
        .put(&guide("g1", "Patch a tyre", "Find the hole."), &[passage("hole")])
        .unwrap();
    index
        .put(&guide("g2", "Patch a tube", "Find the hole."), &[passage("hole")])
        .unwrap();
    index.commit().unwrap();
    index.delete("g1").unwrap();
    index.delete("no-such-key").unwrap();
    index.commit().unwrap();
    assert_eq!(titles(&search(&index, "patch")), ["Patch a tube"]);
    let p = index.search_passages("hole", &Filter::default(), 5).unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].key, "g2");
}

#[test]
fn empty_key_is_rejected() {
    let index = SearchIndex::in_memory(Config::default()).unwrap();
    let err = index.put(&guide("", "No key", "x"), &[]).unwrap_err();
    assert!(matches!(err, SearchError::InvalidDocument(_)));
}

#[test]
fn index_on_disk_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    {
        let index = SearchIndex::open(dir.path(), Config::default()).unwrap();
        index
            .put(
                &guide("g1", "Fix a bicycle chain", "Use a chain tool."),
                &[passage("chain tool")],
            )
            .unwrap();
        index.finish().unwrap();
        // Writing again after finish opens a new writer.
        index.put(&guide("g2", "Oil a chain", "Use thin oil."), &[]).unwrap();
        index.commit().unwrap();
    }
    let index = SearchIndex::open(dir.path(), Config::default()).unwrap();
    assert_eq!(index.num_docs(), 2);
    assert_eq!(search(&index, "bicycle")[0].key, "g1");
    assert_eq!(
        index.search_passages("tool", &Filter::default(), 1).unwrap()[0].key,
        "g1"
    );
}

#[test]
fn wrong_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    drop(SearchIndex::open(dir.path(), Config::default()).unwrap());
    std::fs::write(dir.path().join("VERSION"), "99\n").unwrap();
    let err = SearchIndex::open(dir.path(), Config::default()).err().unwrap();
    assert!(matches!(err, SearchError::Version(v) if v == "99"));

    std::fs::remove_file(dir.path().join("VERSION")).unwrap();
    let err = SearchIndex::open(dir.path(), Config::default()).err().unwrap();
    assert!(
        matches!(err, SearchError::Version(_)),
        "index data without a version file"
    );
}

#[test]
fn bad_config_is_refused() {
    for config in [
        Config {
            writer_memory: 20_000_000,
            ..Config::default()
        },
        Config {
            writer_threads: 0,
            ..Config::default()
        },
        Config {
            writer_threads: 2,
            ..Config::default()
        },
    ] {
        assert!(matches!(SearchIndex::in_memory(config), Err(SearchError::Config(_))));
    }
    assert!(SearchIndex::in_memory(Config {
        writer_memory: 60_000_000,
        writer_threads: 2,
        fuzzy_below: 3
    })
    .is_ok());
}

// ---------------------------------------------------------------------------------------------
// Bad input

#[test]
fn odd_queries_never_fail() {
    let index = library();
    let long = "bleeding ".repeat(2000);
    for query in [
        "",
        " ",
        "\"",
        "\"heart attack\"",
        "(((",
        ")",
        "title_en:",
        "nosuchfield:bleeding",
        "-",
        "-bleeding",
        "+",
        "*",
        "?",
        "AND OR NOT",
        "[a TO",
        "{}",
        "bleeding^",
        "bleeding^abc",
        "body_en:[0 TO 9]",
        "~",
        "bleeding~2",
        "\\",
        "\0",
        "🩸 bleeding 🩸",
        "the a of",
        "ここは日本語",
        long.as_str(),
    ] {
        let hits = index.search(query, &Filter::default(), 10);
        assert!(hits.is_ok(), "{query:?}: {hits:?}");
        assert!(
            index.search_passages(query, &Filter::default(), 10).is_ok(),
            "{query:?}"
        );
    }
    assert!(index.search("bleeding", &Filter::default(), 0).unwrap().is_empty());
    let hits = index.search("\"heart attack\"", &Filter::default(), 10).unwrap();
    assert_eq!(hits[0].title, "Myocardial infarction");
}

// ---------------------------------------------------------------------------------------------
// Snippets

#[test]
fn snippet_escapes_everything_but_the_highlights() {
    let body = LIBRARY[12].6;
    let html = snippet("bleeding", body, "en", 200);
    assert_eq!(
        html,
        "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; Clean every wound &amp; cover it. \
         &quot;<b>Bleeding</b>&quot; that won&#39;t stop needs pressure."
    );
    assert!(!snippet("script alert", body, "en", 200).contains("<script"));
    assert_eq!(snippet("script", "<script>", "en", 50), "&lt;<b>script</b>&gt;");
}

#[test]
fn snippet_highlights_stems_and_typos() {
    let html = snippet(
        "stop bleeding",
        "Bleeds stop when you press. Stopping blood loss matters.",
        "en",
        200,
    );
    assert_eq!(
        html,
        "<b>Bleeds</b> <b>stop</b> when you press. <b>Stopping</b> blood loss matters."
    );
    let html = snippet("diarhea", "Children with diarrhea need fluids.", "en", 200);
    assert_eq!(html, "Children with <b>diarrhea</b> need fluids.");
    let html = snippet("morsures", "Une morsure de serpent.", "fr", 200);
    assert_eq!(html, "Une <b>morsure</b> de serpent.");
    let html = snippet("snake bite", "A snakebite needs care.", "en", 200);
    assert_eq!(html, "A <b>snakebite</b> needs care.");
}

#[test]
fn snippet_picks_the_best_window() {
    let filler = "Nothing to see in this sentence at all. ".repeat(20);
    let text = format!("{filler}A tourniquet stops bleeding from a leg. {filler}Only bleeding here. {filler}");
    let html = snippet("tourniquet bleeding", &text, "en", 120);
    assert!(html.starts_with('…') && html.ends_with('…'), "{html}");
    assert!(
        html.contains("A <b>tourniquet</b> stops <b>bleeding</b> from a leg."),
        "{html}"
    );
    let visible = html.replace("<b>", "").replace("</b>", "");
    assert!(visible.chars().count() <= 122, "{} chars", visible.chars().count());

    // No match: the start of the text.
    let html = snippet("volcano", &text, "en", 40);
    assert!(html.starts_with("Nothing to see") && html.ends_with('…'), "{html}");
    assert_eq!(snippet("x", "short", "en", 0), "");
    assert_eq!(snippet("", "  short   text ", "en", 100), "short text");
}
