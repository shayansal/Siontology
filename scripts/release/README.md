# Releases

A release is made by `.github/workflows/release.yml` when a tag `v*` is
pushed: `sion` built for Linux (x86_64, aarch64) and macOS (Apple silicon,
Intel), one archive each (`sion-<target>.tar.gz`), their SHA-256 sums
(`SHA256SUMS`), the sums' SSH signature (`SHA256SUMS.sig`), and the
installer.

`allowed_signers` holds the release key's public half. `sion` is built with
it, and `sion update` installs only a release whose `SHA256SUMS` that key
signed; the installer checks the same. While it holds no key, no build
updates itself, and the workflow refuses to publish.

## Once: the release key

```bash
ssh-keygen -t ed25519 -N '' -C releases@siontology.org -f sion-release
printf 'releases@siontology.org namespaces="file" %s\n' "$(cat sion-release.pub)" \
  >> scripts/release/allowed_signers
gh secret set SION_RELEASE_SSH_KEY < sion-release
```

Commit `allowed_signers`, then keep `sion-release` offline (or delete it:
the secret is the copy the workflow uses). Builds made before the key was
committed cannot update themselves; install a release over them once.

To replace the key, add the new one's line, release with it, and remove
the old line a release later: a build accepts any key its file names.

## Each release

```bash
# the workspace's version, in Cargo.toml, is the tag's
git tag -s v0.2.0 -m 'sion 0.2.0'
git push origin v0.2.0
```

The workflow checks that the tag is the workspace's version, signs the
sums, checks the signature against `allowed_signers`, and publishes. Run it
by hand (Actions → release → Run workflow) to build and sign without
publishing.
