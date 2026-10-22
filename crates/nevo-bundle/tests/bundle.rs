//! Bundles and folder export with real stores in temporary folders.

use std::fs;
use std::io::Cursor;

use nevo_bundle::*;
use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_store::Store;

fn key(n: u8) -> Keypair {
    Keypair::from_secret([n; 32])
}

fn time() -> Time {
    Time {
        ms: 1_792_000_000_000,
        src: TimeSource::Unknown,
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("library")).unwrap();
    (dir, store)
}

fn revision(author: &Keypair, title: &str, topic: &str, vis: Visibility, blob: Id, size: u64) -> Revision {
    Revision {
        item: None,
        parents: vec![],
        author: author.public(),
        clock: 1,
        time: time(),
        kind: Kind::Guide,
        title: title.into(),
        lang: "en".into(),
        topics: vec![topic.parse().unwrap()],
        license: "CC-BY-SA-4.0".into(),
        vis,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob,
            size,
            mime: "text/markdown".into(),
        }],
        source: None,
        summary: None,
        extra: Map::new(),
    }
}

/// Stores a guide with `body` as its file. Public guides are published to `node`'s log;
/// others are only stored, as the node does for private content.
fn guide(store: &Store, node: &Keypair, title: &str, topic: &str, vis: Visibility, body: &[u8]) -> Sealed<Revision> {
    let blob = store.put_blob(body).unwrap();
    let rev = seal(&revision(node, title, topic, vis, blob.id, blob.size), node).unwrap();
    if vis == Visibility::Public {
        store.publish(node, rev.bytes()).unwrap();
    } else {
        store.put_object(rev.bytes()).unwrap();
    }
    rev
}

fn bundle_bytes(store: &Store, key: &Keypair, options: &ExportOptions) -> (Vec<u8>, ExportReport) {
    let mut out = Vec::new();
    let report = export_bundle(store, key, options, &mut out).unwrap();
    (out, report)
}

fn import(store: &Store, bytes: &[u8]) -> ImportReport {
    import_bundle(store, Cursor::new(bytes)).unwrap()
}

fn options() -> ExportOptions {
    ExportOptions {
        time: time(),
        ..ExportOptions::default()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Library A: node `a` with two public guides (one edited), an attestation, a private and a
/// node-only guide, plus a copy of node `c`'s log received earlier by bundle.
struct Library {
    _dir: tempfile::TempDir,
    store: Store,
    a: Keypair,
    c: Keypair,
    public: Vec<Sealed<Revision>>,
    hidden: Vec<Sealed<Revision>>,
}

fn library() -> Library {
    let (a, c) = (key(1), key(3));

    // Node C publishes a guide and passes it to A on a stick.
    let (_c_dir, c_store) = store();
    let from_c = guide(
        &c_store,
        &c,
        "Solar panel care",
        "energy/solar",
        Visibility::Public,
        b"Wipe the panels.",
    );
    let (c_bundle, _) = bundle_bytes(&c_store, &c, &options());

    let (dir, store) = store();
    assert!(import(&store, &c_bundle).complete());

    let water = guide(
        &store,
        &a,
        "Boiling water",
        "water/treatment",
        Visibility::Public,
        b"Boil it.",
    );
    let stove = guide(
        &store,
        &a,
        "Rocket stove",
        "energy/cooking",
        Visibility::Public,
        b"Stack bricks.",
    );
    let edit_blob = store.put_blob(&b"Boil it for one minute."[..]).unwrap();
    let edit = seal(
        &Revision {
            item: Some(water.id()),
            parents: vec![water.id()],
            clock: 2,
            ..revision(
                &a,
                "Boiling water",
                "water/treatment",
                Visibility::Public,
                edit_blob.id,
                edit_blob.size,
            )
        },
        &a,
    )
    .unwrap();
    store.publish(&a, edit.bytes()).unwrap();
    let verified = seal(
        &Attestation {
            about: Subject::Object(stove.id()),
            claim: Claim::Verified,
            topics: vec!["energy".parse().unwrap()],
            related: None,
            note: None,
            author: a.public(),
            clock: 3,
            time: time(),
            extra: Map::new(),
        },
        &a,
    )
    .unwrap();
    store.publish(&a, verified.bytes()).unwrap();

    let private = guide(
        &store,
        &a,
        "My diary",
        "general",
        Visibility::Private,
        b"SECRET-DIARY-TEXT",
    );
    let node_only = guide(&store, &a, "Door code", "general", Visibility::Node, b"NODE-ONLY-TEXT");

    Library {
        _dir: dir,
        store,
        a,
        c,
        public: vec![from_c, water, stove, edit],
        hidden: vec![private, node_only],
    }
}

#[test]
fn a_to_b_carries_everything_public() {
    let lib = library();
    let (bytes, written) = bundle_bytes(&lib.store, &lib.a, &options());
    assert_eq!(written.logs, 2);
    assert_eq!(written.entries, 5, "1 entry of c, 4 of a");
    assert!(written.problems.is_empty(), "{:?}", written.problems);
    // A's own log got a signed head, so all of it could travel.
    assert_eq!(lib.store.log_head(lib.a.public()).unwrap().unwrap().0, 4);

    let (_dir, b) = store();
    let report = import(&b, &bytes);
    assert!(report.complete(), "{:?}", report.problems);
    assert_eq!(report.by, lib.a.public());
    assert_eq!((report.heads, report.entries), (2, 5));

    for node in [lib.a.public(), lib.c.public()] {
        assert_eq!(b.log_position(node).unwrap(), lib.store.log_position(node).unwrap());
        assert_eq!(b.log_head(node).unwrap(), lib.store.log_head(node).unwrap());
        assert_eq!(
            b.log_entries(node, 0, 100).unwrap(),
            lib.store.log_entries(node, 0, 100).unwrap()
        );
    }
    for rev in &lib.public {
        assert!(b.has_object(rev.id()).unwrap());
        for f in &rev.object.content {
            assert!(b.has_blob(f.blob));
            assert_eq!(b.verify_blob(f.blob).unwrap(), Some(true));
        }
    }
    let water = lib.public[1].id();
    assert_eq!(b.item_tips(water).unwrap()[0].id(), lib.public[3].id());
    assert_eq!(
        b.attestations_about(Subject::Object(lib.public[2].id())).unwrap().len(),
        1
    );

    // B can pass A's and C's logs on in turn.
    let (again, _) = bundle_bytes(&b, &key(9), &options());
    let (_dir2, d) = store();
    assert!(import(&d, &again).complete());
    assert_eq!(d.log_position(lib.c.public()).unwrap(), 1);
}

#[test]
fn private_content_is_never_exported() {
    let lib = library();
    let (bytes, report) = bundle_bytes(&lib.store, &lib.a, &options());
    for rev in &lib.hidden {
        assert!(find(&bytes, rev.id().to_string().as_bytes()).is_none());
        assert!(find(&bytes, rev.bytes()).is_none());
    }
    assert!(find(&bytes, b"SECRET-DIARY-TEXT").is_none());
    assert!(find(&bytes, b"NODE-ONLY-TEXT").is_none());
    assert_eq!(report.skipped_objects, 0);

    let (_dir, b) = store();
    import(&b, &bytes);
    for rev in &lib.hidden {
        assert!(!b.has_object(rev.id()).unwrap());
        assert!(!b.has_blob(rev.object.content[0].blob));
    }
}

#[test]
fn a_tampered_blob_is_rejected() {
    let lib = library();
    let (mut bytes, _) = bundle_bytes(&lib.store, &lib.a, &options());
    let at = find(&bytes, b"Stack bricks.").unwrap();
    bytes[at] = b'X';

    let (_dir, b) = store();
    let report = import(&b, &bytes);
    let stove_blob = lib.public[2].object.content[0].blob;
    assert!(!b.has_blob(stove_blob));
    assert!(!report.complete());
    assert!(
        report.problems.listed.iter().any(|p| p.contains("damaged")),
        "{:?}",
        report.problems
    );
    // Everything else arrived.
    assert!(b.has_object(lib.public[2].id()).unwrap());
    assert!(b.has_blob(lib.public[1].object.content[0].blob));
}

#[test]
fn a_cut_off_bundle_keeps_the_valid_part_and_resumes() {
    let lib = library();
    let big = vec![7u8; 200_000];
    let big_rev = guide(&lib.store, &lib.a, "Field atlas", "maps", Visibility::Public, &big);
    let (bytes, _) = bundle_bytes(&lib.store, &lib.a, &options());

    // Cut in the middle of the big blob, which comes last.
    let (_dir, b) = store();
    let report = import(&b, &bytes[..bytes.len() - 100_000]);
    assert!(!report.complete());
    assert!(
        report.problems.listed.iter().any(|p| p.contains("cut off")),
        "{:?}",
        report.problems
    );
    assert!(b.has_object(big_rev.id()).unwrap(), "metadata comes before blobs");
    assert_eq!(b.log_position(lib.a.public()).unwrap(), 5);
    assert!(!b.has_blob(big_rev.object.content[0].blob));

    // Importing the whole bundle later fills in the rest.
    let report = import(&b, &bytes);
    assert!(report.complete(), "{:?}", report.problems);
    assert_eq!(b.verify_blob(big_rev.object.content[0].blob).unwrap(), Some(true));

    // A cut anywhere never panics or leaves a bad state; past bundle.cbor it is never fatal.
    let first_member_end = 1024;
    for cut in (0..bytes.len()).step_by(bytes.len() / 37) {
        let (_dir, c) = store();
        match import_bundle(&c, Cursor::new(&bytes[..cut])) {
            Ok(report) => assert!(!report.complete() || cut >= bytes.len() - 1024, "cut at {cut}"),
            Err(BundleError::NotABundle(_)) => assert!(cut < first_member_end + 512, "cut at {cut}"),
            Err(e) => panic!("cut at {cut}: {e}"),
        }
        // Whatever arrived can be completed.
        assert!(import(&c, &bytes).complete());
    }
}

#[test]
fn importing_twice_is_harmless() {
    let lib = library();
    let (bytes, _) = bundle_bytes(&lib.store, &lib.a, &options());
    let (_dir, b) = store();
    let first = import(&b, &bytes);
    let second = import(&b, &bytes);
    assert!(second.complete(), "{:?}", second.problems);
    assert_eq!(
        (second.heads, second.entries, second.objects, second.blobs),
        (0, 0, 0, 0)
    );
    assert_eq!(second.already_had, first.objects + first.blobs);
    assert_eq!(b.log_position(lib.a.public()).unwrap(), 4);
}

#[test]
fn later_bundles_carry_only_new_entries() {
    let lib = library();
    let (first, _) = bundle_bytes(&lib.store, &lib.a, &options());
    let (_dir, b) = store();
    import(&b, &first);

    let newer = guide(
        &lib.store,
        &lib.a,
        "Seed saving",
        "farming/seeds",
        Visibility::Public,
        b"Dry them.",
    );
    let mut opts = options();
    opts.known = [lib.a.public(), lib.c.public()]
        .into_iter()
        .map(|n| (n, b.log_position(n).unwrap()))
        .collect();
    let (second, written) = bundle_bytes(&lib.store, &lib.a, &opts);
    assert_eq!((written.logs, written.entries, written.objects), (1, 1, 1));

    let report = import(&b, &second);
    assert!(report.complete(), "{:?}", report.problems);
    assert!(b.has_object(newer.id()).unwrap());
    assert_eq!(b.log_position(lib.a.public()).unwrap(), 5);

    // Without the first bundle the entries can't attach, but the content still arrives.
    let (_dir2, fresh) = store();
    let report = import(&fresh, &second);
    assert!(report.problems.listed.iter().any(|p| p.contains("earlier bundle")));
    assert!(fresh.has_object(newer.id()).unwrap());
    assert_eq!(fresh.log_position(lib.a.public()).unwrap(), 0);
}

#[test]
fn blob_filters() {
    let lib = library();
    let mut opts = options();
    opts.blobs.topics = vec!["energy".parse().unwrap()];
    let (bytes, written) = bundle_bytes(&lib.store, &lib.a, &opts);
    assert_eq!(written.blobs, 2, "the stove and C's solar guide");
    let (_dir, b) = store();
    assert!(import(&b, &bytes).complete());
    assert!(b.has_blob(lib.public[2].object.content[0].blob));
    assert!(!b.has_blob(lib.public[1].object.content[0].blob));
    assert!(b.has_object(lib.public[1].id()).unwrap(), "metadata always moves");

    let mut opts = options();
    opts.blobs.max_size = Some(10);
    let (_, written) = bundle_bytes(&lib.store, &lib.a, &opts);
    assert_eq!(written.blobs, 1, "only `Boil it.`");

    let mut opts = options();
    opts.blobs = BlobFilter::none();
    opts.nodes = Some(vec![lib.c.public()]);
    let (_, written) = bundle_bytes(&lib.store, &lib.a, &opts);
    assert_eq!((written.logs, written.blobs), (1, 0));
}

/// A bundle with chosen extra members, each with a raw (unchecked) name.
fn with_members(start: &[u8], members: &[(&[u8], &[u8])]) -> Vec<u8> {
    // Keep bundle.cbor and its data (the first member), drop the end-of-archive blocks.
    let size = usize::try_from(tar::Header::from_byte_slice(&start[..512]).entry_size().unwrap()).unwrap();
    let mut out = start[..512 + size.div_ceil(512) * 512].to_vec();
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in members {
        let mut header = tar::Header::new_ustar();
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append(&header, *data).unwrap();
    }
    out.extend(builder.into_inner().unwrap());
    out
}

#[test]
fn odd_members_are_refused() {
    let (dir, a) = store();
    let (empty, _) = bundle_bytes(&a, &key(1), &options());
    let stray = seal(
        &revision(&key(2), "Stray", "general", Visibility::Public, Id::of(b"x"), 1),
        &key(2),
    )
    .unwrap();
    let stray_name = format!("objects/{}", stray.id());
    let bytes = with_members(
        &empty,
        &[
            (b"../evil", b"x"),
            (b"/tmp/evil", b"x"),
            (b"objects/../../evil", b"x"),
            (stray_name.as_bytes(), stray.bytes()),
            (format!("blobs/{}", Id::of(b"x")).as_bytes(), b"x"),
        ],
    );
    let (_dir, b) = store();
    let report = import(&b, &bytes);
    assert_eq!(report.problems.len(), 5, "{:?}", report.problems);
    assert!(!b.has_object(stray.id()).unwrap(), "objects must come with a log");
    assert!(!b.has_blob(Id::of(b"x")), "blobs must be listed");
    assert!(!dir.path().join("evil").exists() && !dir.path().parent().unwrap().join("evil").exists());
}

#[test]
fn a_bad_bundle_object_stops_the_import() {
    let lib = library();
    let (bytes, _) = bundle_bytes(&lib.store, &lib.a, &options());
    let (_dir, b) = store();

    // Any change to bundle.cbor (just after the first 512-byte header) breaks it or its signature.
    let mut bad = bytes.clone();
    bad[512 + 40] ^= 1;
    assert!(matches!(
        import_bundle(&b, Cursor::new(&bad)),
        Err(BundleError::NotABundle(_))
    ));

    // bundle.cbor must come first.
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_path("objects/x").unwrap();
    header.set_size(1);
    header.set_cksum();
    builder.append(&header, &b"x"[..]).unwrap();
    let not_first = builder.into_inner().unwrap();
    assert!(matches!(
        import_bundle(&b, Cursor::new(&not_first)),
        Err(BundleError::NotABundle(_))
    ));
    assert!(matches!(
        import_bundle(&b, Cursor::new(&[] as &[u8])),
        Err(BundleError::NotABundle(_))
    ));
    assert_eq!(b.log_position(lib.a.public()).unwrap(), 0);
}

#[test]
fn bundle_files_are_written_whole() {
    let lib = library();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kit.nevo");
    let written = export_bundle_file(&lib.store, &lib.a, &options(), &path).unwrap();
    assert!(!dir.path().join("kit.nevo.part").exists());
    assert!(matches!(
        export_bundle_file(&lib.store, &lib.a, &options(), &path),
        Err(BundleError::Target(_))
    ));
    let (_dir, b) = store();
    let report = import_bundle_file(&b, &path).unwrap();
    assert_eq!(report.bundle, written.bundle);
    assert!(report.complete());
}

#[test]
fn folder_export() {
    let lib = library();
    guide(
        &lib.store,
        &lib.a,
        "boiling WATER",
        "water",
        Visibility::Public,
        b"Same title, other item.",
    );
    guide(
        &lib.store,
        &lib.a,
        "CON",
        "general",
        Visibility::Public,
        b"Reserved on Windows.",
    );
    guide(
        &lib.store,
        &lib.a,
        "What? A/B: \"test\" ",
        "general",
        Visibility::Public,
        b"Odd characters.",
    );

    let out = tempfile::tempdir().unwrap();
    let dir = out.path().join("export");
    let report = export_folders(&lib.store, &dir, &FolderOptions::default()).unwrap();
    assert_eq!(report.skipped_private, 2);
    assert_eq!(report.items, 6);
    assert!(report.problems.is_empty(), "{:?}", report.problems);

    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Boiling water",
            "Rocket stove",
            "Solar panel care",
            "What_ A_B_ _test_",
            "_CON",
            "boiling WATER (2)"
        ]
    );
    // The current version of an edited item.
    assert_eq!(
        fs::read_to_string(dir.join("Boiling water").join("guide.md")).unwrap(),
        "Boil it for one minute."
    );
    let meta = fs::read_to_string(dir.join("Boiling water").join("meta.json")).unwrap();
    for expected in [
        "\"title\": \"Boiling water\"",
        "\"license\": \"CC-BY-SA-4.0\"",
        "\"water/treatment\"",
        &format!("\"item\": \"{}\"", lib.public[1].id()),
        &format!("\"revision\": \"{}\"", lib.public[3].id()),
        "\"current\": true",
        "\"current\": false",
    ] {
        assert!(meta.contains(expected), "{expected} missing from\n{meta}");
    }
    assert!(!dir.join("My diary").exists());

    // The folder must be empty, so a second export can't mix with the first.
    assert!(matches!(
        export_folders(&lib.store, &dir, &FolderOptions::default()),
        Err(BundleError::Target(_))
    ));

    let all = out.path().join("all");
    let report = export_folders(&lib.store, &all, &FolderOptions { include_private: true }).unwrap();
    assert_eq!((report.items, report.skipped_private), (8, 0));
    assert_eq!(
        fs::read_to_string(all.join("My diary").join("guide.md")).unwrap(),
        "SECRET-DIARY-TEXT"
    );
}
