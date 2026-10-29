# Security Policy

## Supported versions

Only the latest release receives security fixes.

## Reporting a vulnerability

Please do **not** open a public issue. Report it privately through
[GitHub's private vulnerability reporting](https://github.com/Padishar24/tessera/security/advisories/new)
instead. You will get an answer within a few days.

## Verifying downloads

Every release zip is built by GitHub Actions from the tagged commit and carries a build
provenance attestation. With the [GitHub CLI](https://cli.github.com) you can check that a
download really comes from this repository:

```bash
gh attestation verify tessera-vX.Y.Z-x64.zip --repo Padishar24/tessera
```

Attestations exist from v0.9.24 on. Each zip also has a `.sha256` file next to it in the release.
