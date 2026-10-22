# Security policy

NEVO runs offline on small boxes that people may depend on in a crisis, so security problems matter to us even before v1.0.

## Reporting a vulnerability

Please report it privately through **GitHub security advisories**:
<https://github.com/ChristianBosse/NaturalEvolution/security/advisories/new>
(the "Report a vulnerability" button on the repository's Security tab).

Please don't open a public issue for a vulnerability.

Include:

- what you found and what it affects (node, Seed, format, a content pack)
- steps or a file to reproduce it
- the version or commit you tested

We aim to answer within a week. Fixes are credited to the reporter unless you ask otherwise.

Dangerous or wrong **content** (a bad guide, a wrong dose) is not a code vulnerability. Report it as an ordinary issue so others can see it.

## Supported versions

NEVO is in early development. Only the latest code on `main` gets security fixes.

## What we defend against

The threat model, with every known gap and its status, is in [docs/security.md](docs/security.md).
