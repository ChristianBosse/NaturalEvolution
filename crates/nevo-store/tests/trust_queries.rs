//! Queries made for the trust layer. Kept in their own file so they merge cleanly.

use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_store::Store;

fn guide(author: &Keypair, title: &str) -> Sealed<Revision> {
    let revision = Revision {
        item: None,
        parents: vec![],
        author: author.public(),
        clock: 1,
        time: Time {
            ms: 0,
            src: TimeSource::Unknown,
        },
        kind: Kind::Guide,
        title: title.into(),
        lang: "en".into(),
        topics: vec!["general".parse().unwrap()],
        license: "CC-BY-SA-4.0".into(),
        vis: Visibility::Public,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob: Id::of(title.as_bytes()),
            size: 1,
            mime: "text/markdown".into(),
        }],
        source: None,
        summary: None,
        extra: Map::new(),
    };
    seal(&revision, author).unwrap()
}

#[test]
fn logged_by_lists_the_nodes_whose_log_has_the_object() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let author = Keypair::from_secret([1; 32]);
    let (a, b) = (Keypair::from_secret([2; 32]), Keypair::from_secret([3; 32]));

    let shared = guide(&author, "Shared");
    let only_stored = guide(&author, "Only stored");
    store.publish(&a, shared.bytes()).unwrap();
    store.publish(&b, shared.bytes()).unwrap();
    store.put_object(only_stored.bytes()).unwrap();

    let mut expected = vec![a.public(), b.public()];
    expected.sort();
    assert_eq!(store.logged_by(shared.id()).unwrap(), expected);
    assert!(store.logged_by(only_stored.id()).unwrap().is_empty());
    assert!(store.logged_by(Id::of(b"missing")).unwrap().is_empty());
}

#[test]
fn attestation_refs_list_the_index_without_loading() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (author, curator, other) = (
        Keypair::from_secret([1; 32]),
        Keypair::from_secret([2; 32]),
        Keypair::from_secret([3; 32]),
    );
    let rev = guide(&author, "Guide");
    store.put_object(rev.bytes()).unwrap();
    let attest = |by: &Keypair, about: Subject, claim: Claim, topics: &[&str], clock: u64| {
        let att = Attestation {
            about,
            claim,
            topics: topics.iter().map(|t| t.parse().unwrap()).collect(),
            related: None,
            note: None,
            author: by.public(),
            clock,
            time: Time {
                ms: 0,
                src: TimeSource::Unknown,
            },
            extra: Map::new(),
        };
        let sealed = seal(&att, by).unwrap();
        store.put_object(sealed.bytes()).unwrap();
        sealed.id()
    };
    let hide = attest(&other, Subject::Object(rev.id()), Claim::Hide, &[], 5);
    let verified = attest(&curator, Subject::Object(rev.id()), Claim::Verified, &["general"], 2);
    let trust = attest(&curator, Subject::Key(author.public()), Claim::Trust, &["general"], 3);

    let refs = store.attestation_refs(Subject::Object(rev.id())).unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(
        (refs[0].id, refs[0].author, refs[0].claim.as_str(), refs[0].clock),
        (verified, curator.public(), "verified", 2),
        "oldest clock first"
    );
    assert_eq!((refs[1].id, refs[1].claim.as_str()), (hide, "hide"));

    let refs = store.attestation_refs(Subject::Key(author.public())).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!((refs[0].id, refs[0].claim.as_str()), (trust, "trust"));
    assert!(store
        .attestation_refs(Subject::Key(curator.public()))
        .unwrap()
        .is_empty());
}
