//! Label computation. See the crate docs for the rules.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use nevo_format::{Attestation, Claim, Id, PublicKey, Revision, Sealed, Subject, Topic};
use nevo_store::{AttestationRef, Result, Store};

use crate::config::{all_topics, role_of, Delegation, Role, TrustConfig};

/// The claims about a revision that affect its label or flags.
const COUNTED_CLAIMS: [&str; 4] = ["verified", "dangerous", "outdated", "hide"];

/// How far a revision can be trusted on this node, weakest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Label {
    /// From sources this node doesn't trust. Shown with a warning, and never used by the AI on
    /// safety-critical topics (D-014).
    Unknown,
    /// From a trusted node or author, not verified.
    Community,
    /// Attested `verified` by a curator this node trusts for the topic.
    Verified,
    /// Signed by a catalog this node trusts for the topic: an official pack.
    Reference,
}

impl Label {
    pub fn as_str(self) -> &'static str {
        match self {
            Label::Unknown => "unknown",
            Label::Community => "community",
            Label::Verified => "verified",
            Label::Reference => "reference",
        }
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Label::Unknown => "Unknown",
            Label::Community => "Community",
            Label::Verified => "Verified",
            Label::Reference => "Reference",
        })
    }
}

/// That this node trusts a key as `role` for `topics`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub key: PublicKey,
    pub role: Role,
    /// Known topics only: no trust applies to an unknown top-level topic.
    pub topics: Vec<Topic>,
    /// The `trust` attestation this comes from. `None` for this node's own key.
    pub attestation: Option<Id>,
    /// `None` if this node's admin chose the key. Otherwise the curator or catalog whose `trust`
    /// attestation was followed (one hop).
    pub via: Option<PublicKey>,
}

impl Grant {
    pub fn covers(&self, topic: &Topic) -> bool {
        self.topics.iter().any(|g| g.covers(topic))
    }
}

/// A trusted key behind a label or flag, ready for the UI ("Verified by Dr. A").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub key: PublicKey,
    /// The name from the key's own record. Self-chosen: show it with the key, not instead of it.
    pub name: Option<String>,
    pub role: Role,
    /// See [`Grant::via`].
    pub via: Option<PublicKey>,
}

/// Why a revision got its label, or what was ignored on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Signed by a catalog trusted for `topic`.
    Catalog { by: Source, topic: Topic },
    /// Attested `verified` for `topic` by a trusted curator or catalog.
    Verified { by: Source, topic: Topic, attestation: Id },
    /// Signed by a key trusted for `topic`.
    TrustedAuthor { by: Source, topic: Topic },
    /// Published in the log of a node trusted for `topic`.
    TrustedNode { by: Source, topic: Topic },
    /// Nothing trusted for `topic` vouches for it.
    NoTrustedSource { topic: Topic },
    /// The top-level topic is from a later format version, so no trust applies to it.
    UnknownTopic { topic: Topic },
    /// The key is revoked from clock `from`. What it signed from then on was ignored.
    Revoked { key: PublicKey, from: u64 },
    /// The node signed two different logs. Nothing it signed or published counts.
    Forked { node: PublicKey },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlagKind {
    Dangerous,
    Outdated,
    /// Hidden by default (spam, off-topic).
    Hide,
}

/// A `dangerous`, `outdated` or `hide` attestation from a trusted curator or catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flag {
    pub kind: FlagKind,
    pub by: Source,
    pub attestation: Id,
    /// Why, in the curator's words.
    pub note: Option<String>,
}

/// A revision's label on this node, with the reasons and flags behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub label: Label,
    pub reasons: Vec<Reason>,
    pub flags: Vec<Flag>,
}

impl Assessment {
    pub fn has(&self, kind: FlagKind) -> bool {
        self.flags.iter().any(|f| f.kind == kind)
    }

    /// Whether the revision is hidden by default.
    pub fn hidden(&self) -> bool {
        self.has(FlagKind::Hide)
    }
}

/// What is known about a key, whatever anyone trusts it for.
#[derive(Clone, Debug, Default)]
struct KeyState {
    /// The lowest `from` among authorized revocations.
    revoked_from: Option<u64>,
    forked: bool,
    name: Option<String>,
}

/// Computes labels from the store's attestations.
///
/// A `Trust` caches what it learns about keys, so labeling a page of 50 items costs a few indexed
/// queries per item plus a few per distinct key. It is a snapshot: make a new one for each page
/// or request, so it sees new attestations, revocations and forks.
pub struct Trust<'s> {
    store: &'s Store,
    config: TrustConfig,
    states: HashMap<PublicKey, Rc<KeyState>>,
    trust_refs: HashMap<PublicKey, Rc<[AttestationRef]>>,
    direct: HashMap<PublicKey, Rc<[Grant]>>,
    grants: HashMap<PublicKey, Rc<[Grant]>>,
}

impl<'s> Trust<'s> {
    pub fn new(store: &'s Store, config: TrustConfig) -> Trust<'s> {
        Trust {
            store,
            config,
            states: HashMap::new(),
            trust_refs: HashMap::new(),
            direct: HashMap::new(),
            grants: HashMap::new(),
        }
    }

    pub fn config(&self) -> &TrustConfig {
        &self.config
    }

    /// The label alone. See [`Trust::assess`].
    pub fn label(&mut self, revision: &Sealed<Revision>, context: Option<&Topic>) -> Result<Label> {
        Ok(self.assess(revision, context)?.label)
    }

    /// The revision's label, reasons and flags.
    ///
    /// `context` is the topic being browsed or asked about. The label is computed for each of
    /// the revision's topics under it (or all its topics without one, or if none is under it),
    /// and the weakest wins: a guide tagged `medical` and `water` that a water curator verified
    /// is not Verified for `medical`. Flags are collected over all the revision's topics, so a
    /// `dangerous` flag shows whatever the context.
    pub fn assess(&mut self, revision: &Sealed<Revision>, context: Option<&Topic>) -> Result<Assessment> {
        let rev = &revision.object;
        let own_topics = if rev.topics.is_empty() {
            vec!["general".parse().expect("valid topic")]
        } else {
            rev.topics.clone()
        };
        let topics = in_context(&own_topics, context);
        let mut warnings = Vec::new();

        let author_ok = self.usable(rev.author, rev.clock, &mut warnings)?;
        let mut loggers = Vec::new();
        for node in self.store.logged_by(revision.id())? {
            if self.state(node)?.forked {
                push_once(&mut warnings, Reason::Forked { node });
            } else {
                loggers.push(node);
            }
        }
        let attestations = self.counted_attestations(revision.id(), &mut warnings)?;

        let mut best: Option<(Label, Vec<Reason>)> = None;
        for topic in &topics {
            let (label, reasons) = if !topic.is_known() {
                (Label::Unknown, vec![Reason::UnknownTopic { topic: topic.clone() }])
            } else if !author_ok {
                (Label::Unknown, Vec::new())
            } else {
                self.label_for(rev.author, &loggers, &attestations, topic)?
            };
            if best.as_ref().is_none_or(|(b, _)| label < *b) {
                best = Some((label, reasons));
            }
        }
        let (label, mut reasons) = best.expect("a revision always has a topic");
        for warning in warnings {
            push_once(&mut reasons, warning);
        }

        let mut flag_topics = own_topics;
        for topic in topics {
            if !flag_topics.contains(&topic) {
                flag_topics.push(topic);
            }
        }
        let flags = self.flags(&attestations, &flag_topics)?;
        Ok(Assessment { label, reasons, flags })
    }

    /// Every way this node trusts a key: direct grants first, then one-hop ones. Empty for a
    /// forked node.
    pub fn grants(&mut self, key: PublicKey) -> Result<Rc<[Grant]>> {
        if let Some(g) = self.grants.get(&key) {
            return Ok(g.clone());
        }
        let mut out = self.direct_grants(key)?.to_vec();
        let node = self.config.node;
        let vouched: Vec<AttestationRef> = if self.config.delegation == Delegation::OneHop && key != node {
            let refs = self.trust_refs(key)?;
            refs.iter()
                .filter(|r| r.author != node && r.author != key)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        // Most keys nobody vouches for: settle them without looking the key up.
        if !vouched.is_empty() && !self.state(key)?.forked {
            for r in &vouched {
                let truster = r.author;
                // Only a truster with a direct curator or catalog grant counts: this is the one
                // hop. Check that before anything costlier, so spam costs one cached lookup.
                let trusted: Vec<Grant> = self
                    .direct_grants(truster)?
                    .iter()
                    .filter(|g| g.role.can_attest())
                    .cloned()
                    .collect();
                if trusted.is_empty() || !self.counts(r)? {
                    continue;
                }
                let Some(att) = self.store.get::<Attestation>(r.id)? else {
                    continue;
                };
                let Some(role) = role_of(&att.object) else { continue };
                for g in trusted {
                    let topics = intersect(&g.topics, &att.object.topics);
                    if !topics.is_empty() {
                        out.push(Grant {
                            key,
                            role: role.delegated(),
                            topics,
                            attestation: Some(r.id),
                            via: Some(truster),
                        });
                    }
                }
            }
        }
        let out: Rc<[Grant]> = out.into();
        self.grants.insert(key, out.clone());
        Ok(out)
    }

    // -----------------------------------------------------------------------------------------

    fn label_for(
        &mut self,
        author: PublicKey,
        loggers: &[PublicKey],
        attestations: &[Sealed<Attestation>],
        topic: &Topic,
    ) -> Result<(Label, Vec<Reason>)> {
        if let Some(by) = self.source(author, topic, |r| r == Role::Catalog)? {
            return Ok((
                Label::Reference,
                vec![Reason::Catalog {
                    by,
                    topic: topic.clone(),
                }],
            ));
        }

        let mut verified = Vec::new();
        for att in attestations {
            let a = &att.object;
            if a.claim != Claim::Verified || !a.topics.iter().any(|t| t.covers(topic)) {
                continue;
            }
            if let Some(by) = self.source(a.author, topic, Role::can_attest)? {
                verified.push(Reason::Verified {
                    by,
                    topic: topic.clone(),
                    attestation: att.id(),
                });
            }
        }
        if !verified.is_empty() {
            return Ok((Label::Verified, verified));
        }

        let mut community = Vec::new();
        if let Some(by) = self.source(author, topic, |_| true)? {
            community.push(Reason::TrustedAuthor {
                by,
                topic: topic.clone(),
            });
        }
        for node in loggers {
            if let Some(by) = self.source(*node, topic, Role::vouches_for_log)? {
                community.push(Reason::TrustedNode {
                    by,
                    topic: topic.clone(),
                });
            }
        }
        if !community.is_empty() {
            return Ok((Label::Community, community));
        }
        Ok((Label::Unknown, vec![Reason::NoTrustedSource { topic: topic.clone() }]))
    }

    fn flags(&mut self, attestations: &[Sealed<Attestation>], topics: &[Topic]) -> Result<Vec<Flag>> {
        let mut flags = Vec::new();
        for att in attestations {
            let a = &att.object;
            let kind = match a.claim {
                Claim::Dangerous => FlagKind::Dangerous,
                Claim::Outdated => FlagKind::Outdated,
                Claim::Hide => FlagKind::Hide,
                _ => continue,
            };
            // Topics are optional on flags. When given, they narrow what the flag is about.
            for topic in topics.iter().filter(|t| t.is_known()) {
                if !a.topics.is_empty() && !a.topics.iter().any(|t| t.covers(topic)) {
                    continue;
                }
                if let Some(by) = self.source(a.author, topic, Role::can_attest)? {
                    flags.push(Flag {
                        kind,
                        by,
                        attestation: att.id(),
                        note: a.note.clone(),
                    });
                    break;
                }
            }
        }
        Ok(flags)
    }

    /// Attestations about a revision that could affect its label or flags: from a key with a
    /// curator or catalog grant, valid at their clock, and not withdrawn. Everything else is
    /// dropped after one cached lookup per author, so unknown keys cost almost nothing.
    fn counted_attestations(&mut self, revision: Id, warnings: &mut Vec<Reason>) -> Result<Vec<Sealed<Attestation>>> {
        let mut out = Vec::new();
        for r in self.store.attestation_refs(Subject::Object(revision))? {
            if !COUNTED_CLAIMS.contains(&r.claim.as_str()) {
                continue;
            }
            if !self.grants(r.author)?.iter().any(|g| g.role.can_attest()) {
                continue;
            }
            if !self.usable(r.author, r.clock, warnings)? || self.withdrawn(&r)? {
                continue;
            }
            if let Some(att) = self.store.get::<Attestation>(r.id)? {
                out.push(att);
            }
        }
        Ok(out)
    }

    /// The best grant `key` has for `topic` whose role passes `role_ok`, as a [`Source`].
    /// Direct grants win over one-hop ones, then stronger roles.
    fn source(&mut self, key: PublicKey, topic: &Topic, role_ok: impl Fn(Role) -> bool) -> Result<Option<Source>> {
        let grants = self.grants(key)?;
        let best = grants
            .iter()
            .filter(|g| role_ok(g.role) && g.covers(topic))
            .min_by_key(|g| (g.via.is_some(), std::cmp::Reverse(rank(g.role))));
        let Some(g) = best else { return Ok(None) };
        Ok(Some(Source {
            key,
            name: self.state(key)?.name.clone(),
            role: g.role,
            via: g.via,
        }))
    }

    /// Grants this node's admin made directly, plus this node's own key as a curator for every
    /// known topic.
    fn direct_grants(&mut self, key: PublicKey) -> Result<Rc<[Grant]>> {
        if let Some(g) = self.direct.get(&key) {
            return Ok(g.clone());
        }
        let node = self.config.node;
        let mut out = Vec::new();
        if key == node {
            out.push(Grant {
                key,
                role: Role::Curator,
                topics: all_topics(),
                attestation: None,
                via: None,
            });
        } else {
            let mine: Vec<AttestationRef> = self
                .trust_refs(key)?
                .iter()
                .filter(|r| r.author == node)
                .cloned()
                .collect();
            // Most keys have no grant: settle them without looking the key up.
            if !mine.is_empty() && !self.state(key)?.forked {
                for r in mine {
                    if !self.counts(&r)? {
                        continue;
                    }
                    let Some(att) = self.store.get::<Attestation>(r.id)? else {
                        continue;
                    };
                    let Some(role) = role_of(&att.object) else { continue };
                    let topics: Vec<Topic> = att.object.topics.iter().filter(|t| t.is_known()).cloned().collect();
                    if !topics.is_empty() {
                        out.push(Grant {
                            key,
                            role,
                            topics,
                            attestation: Some(r.id),
                            via: None,
                        });
                    }
                }
            }
        }
        let out: Rc<[Grant]> = out.into();
        self.direct.insert(key, out.clone());
        Ok(out)
    }

    /// The `trust` attestations naming `key`, from the index.
    fn trust_refs(&mut self, key: PublicKey) -> Result<Rc<[AttestationRef]>> {
        if let Some(refs) = self.trust_refs.get(&key) {
            return Ok(refs.clone());
        }
        let refs: Rc<[AttestationRef]> = self
            .store
            .attestation_refs(Subject::Key(key))?
            .into_iter()
            .filter(|r| r.claim == Claim::Trust.as_str())
            .collect();
        self.trust_refs.insert(key, refs.clone());
        Ok(refs)
    }

    /// Whether an attestation still counts: its author's key is valid at its clock and the author
    /// hasn't withdrawn it.
    fn counts(&mut self, r: &AttestationRef) -> Result<bool> {
        Ok(self.usable(r.author, r.clock, &mut Vec::new())? && !self.withdrawn(r)?)
    }

    /// Whether the author took the attestation back. Only the author can, and only with a
    /// withdrawal that is itself valid (a thief can't withdraw after the revocation clock).
    fn withdrawn(&mut self, r: &AttestationRef) -> Result<bool> {
        for w in self.store.attestation_refs(Subject::Object(r.id))? {
            if w.claim == Claim::Withdraw.as_str()
                && w.author == r.author
                && self.usable(w.author, w.clock, &mut Vec::new())?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether an object `key` signed at `clock` still counts: the key is not a forked node, and
    /// not revoked at that clock. Records why not in `warnings`.
    fn usable(&mut self, key: PublicKey, clock: u64, warnings: &mut Vec<Reason>) -> Result<bool> {
        let state = self.state(key)?;
        if state.forked {
            push_once(warnings, Reason::Forked { node: key });
            return Ok(false);
        }
        if let Some(from) = state.revoked_from.filter(|from| clock >= *from) {
            push_once(warnings, Reason::Revoked { key, from });
            return Ok(false);
        }
        Ok(true)
    }

    fn state(&mut self, key: PublicKey) -> Result<Rc<KeyState>> {
        if let Some(s) = self.states.get(&key) {
            return Ok(s.clone());
        }
        let records: Vec<_> = self.store.key_records(key)?.into_iter().map(|r| r.object).collect();
        // Every record counts for authorizing a revocation (D-039), so a thief's new record
        // can't block the owner's recovery key.
        let revoked_from = self
            .store
            .revocations(key)?
            .into_iter()
            .filter(|r| r.object.is_authorized(&records))
            .map(|r| r.object.from)
            .min();
        // Records are oldest first. A record signed after the revocation clock may be a thief's.
        let name = records
            .iter()
            .rev()
            .find(|r| revoked_from.is_none_or(|from| r.clock < from))
            .map(|r| r.name.clone());
        let state = Rc::new(KeyState {
            revoked_from,
            forked: self.store.is_forked(key)?,
            name,
        });
        self.states.insert(key, state.clone());
        Ok(state)
    }
}

/// The topics to label `topics` for under `context`: those it covers, or the context itself
/// where a topic is broader than it. All of them if none relate to the context.
fn in_context(topics: &[Topic], context: Option<&Topic>) -> Vec<Topic> {
    let Some(context) = context else {
        return topics.to_vec();
    };
    let mut out: Vec<Topic> = Vec::new();
    for topic in topics {
        let t = if context.covers(topic) {
            topic
        } else if topic.covers(context) {
            context
        } else {
            continue;
        };
        if !out.contains(t) {
            out.push(t.clone());
        }
    }
    if out.is_empty() {
        topics.to_vec()
    } else {
        out
    }
}

/// The topics two sets of topics both cover: for each pair, the narrower one.
fn intersect(a: &[Topic], b: &[Topic]) -> Vec<Topic> {
    let mut out: Vec<Topic> = Vec::new();
    for x in a {
        for y in b {
            let narrower = if x.covers(y) {
                y
            } else if y.covers(x) {
                x
            } else {
                continue;
            };
            if narrower.is_known() && !out.contains(narrower) {
                out.push(narrower.clone());
            }
        }
    }
    out
}

fn rank(role: Role) -> u8 {
    match role {
        Role::Node | Role::Author => 0,
        Role::Curator => 1,
        Role::Catalog => 2,
    }
}

fn push_once(reasons: &mut Vec<Reason>, reason: Reason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}
