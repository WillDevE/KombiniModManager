# Contributing

Thanks for helping out! Bug reports, ideas and pull requests are all welcome.

## Before you start

- For anything bigger than a small fix, open an issue first so we can agree on the approach.
- Keep pull requests focused: one change per PR.

## Development

```powershell
cargo run -p kombini-app
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same three checks on Windows, and all of them must pass.

A few tests read the real game read-only when `DEADLOCK_PAK` points at
`<Deadlock>\game\citadel\pak01_dir.vpk`. Without it they're skipped.

## Rules

- **Tests never write to a real game folder.** Build a synthetic game folder in a temp directory, as
  the existing tests do.
- Don't use `unwrap()` or `expect()` outside tests unless the invariant is provably unreachable, and
  say why in a comment.
- Log with `tracing`; don't use `println!` outside the CLI.
- Before adding a dependency, check its license is compatible with MIT.
- **Don't copy code from GPL projects** (for example, Deadlock Mod Manager). Reading their file
  formats for interoperability is fine. Code ported from permissively licensed projects (such as
  Grimoire, which is MIT) must be credited in `THIRD-PARTY-NOTICES.md` and on the app's Credits page.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `feat:`, `fix:`, `docs:`, `build:` and so on.

## Releases

Releases are built only by GitHub Actions:

1. Bump `version` in the root `Cargo.toml` and commit.
2. Tag the commit `v<version>` (for example, `v0.2.0`) and push the tag.
3. The `release` workflow builds the `dist` profile, compresses the exe with UPX, and publishes it
   with `SHA256SUMS.txt` to a GitHub release.
