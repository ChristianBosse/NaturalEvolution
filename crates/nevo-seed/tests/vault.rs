//! Vault manifests: create, verify, detect damage, log checks.

use std::fs;

use nevo_seed::vault::{self, Problem, Stamp, CHECKS, MANIFEST};

fn sample() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("packs/medical")).unwrap();
    fs::write(dir.path().join("packs/medical/first-aid.zim"), vec![7u8; 100_000]).unwrap();
    fs::write(dir.path().join("packs/water.zim"), b"boil it").unwrap();
    fs::write(dir.path().join("README.txt"), b"NEVO vault").unwrap();
    dir
}

#[test]
fn verifies_and_finds_damage() {
    let dir = sample();
    let created = Stamp::parse("2026-01-10T08:00:00Z").unwrap();
    let manifest = vault::create(dir.path(), created, &mut |_| {}).unwrap();
    let paths: Vec<&str> = manifest.files.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["README.txt", "packs/medical/first-aid.zim", "packs/water.zim"]);
    assert_eq!(vault::load(dir.path()).unwrap(), manifest);

    let report = vault::verify(dir.path(), &mut |_| {}).unwrap();
    assert!(report.ok());
    assert_eq!((report.files, report.bytes), (3, 100_000 + 7 + 10));
    assert!(report.extra.is_empty());

    // One flipped bit, one lost file, one stray file.
    let zim = dir.path().join("packs/medical/first-aid.zim");
    let mut bytes = fs::read(&zim).unwrap();
    bytes[54_321] ^= 0x10;
    fs::write(&zim, bytes).unwrap();
    fs::remove_file(dir.path().join("packs/water.zim")).unwrap();
    fs::write(dir.path().join("packs/new.txt"), b"?").unwrap();

    let report = vault::verify(dir.path(), &mut |_| {}).unwrap();
    assert!(!report.ok());
    assert_eq!(report.missing, ["packs/water.zim"]);
    assert_eq!(report.corrupt.len(), 1);
    assert_eq!(report.corrupt[0].0, "packs/medical/first-aid.zim");
    assert!(matches!(report.corrupt[0].1, Problem::Hash { .. }));
    assert_eq!(report.extra, ["packs/new.txt"]);

    // A truncated file is reported by size, without hashing.
    fs::write(dir.path().join("README.txt"), b"NEVO").unwrap();
    let report = vault::verify(dir.path(), &mut |_| {}).unwrap();
    assert!(report.corrupt.iter().any(|(p, problem)| p == "README.txt"
        && *problem
            == Problem::Size {
                expected: 10,
                actual: 4
            }));
}

#[test]
fn checks_are_logged_and_due_yearly() {
    let dir = sample();
    let created = Stamp::parse("2025-06-01T12:00:00Z").unwrap();
    vault::create(dir.path(), created, &mut |_| {}).unwrap();
    assert_eq!(vault::last_verified(dir.path()).unwrap(), created);
    assert!(vault::is_due(created, Stamp::parse("2026-06-01T12:00:00Z").unwrap()));

    let at = Stamp::parse("2026-03-01T09:30:00Z").unwrap();
    let report = vault::verify(dir.path(), &mut |_| {}).unwrap();
    let check = vault::record(dir.path(), &report, at).unwrap();
    assert_eq!((check.files, check.missing, check.corrupt), (3, 0, 0));
    assert_eq!(vault::history(dir.path()).unwrap(), [check]);
    assert_eq!(vault::last_verified(dir.path()).unwrap(), at);
    assert!(!vault::is_due(at, Stamp::parse("2026-06-01T12:00:00Z").unwrap()));

    // The vault's own files are never reported as extra.
    let report = vault::verify(dir.path(), &mut |_| {}).unwrap();
    assert!(report.extra.is_empty());
    assert!(fs::read_to_string(dir.path().join(CHECKS))
        .unwrap()
        .contains("2026-03-01T09:30:00Z"));
}

#[test]
fn refuses_to_overwrite_a_manifest() {
    let dir = sample();
    vault::create(dir.path(), Stamp::now(), &mut |_| {}).unwrap();
    assert!(vault::create(dir.path(), Stamp::now(), &mut |_| {}).is_err());
}

#[test]
fn rejects_paths_outside_the_vault() {
    let dir = sample();
    let manifest = format!(
        r#"{{"format": "nevo-vault", "version": 1, "created": "2026-01-01T00:00:00Z",
            "files": [{{"path": "../secret", "size": 1, "sha256": "{}"}}]}}"#,
        "0".repeat(64)
    );
    fs::write(dir.path().join(MANIFEST), manifest).unwrap();
    assert!(vault::verify(dir.path(), &mut |_| {}).is_err());
}
