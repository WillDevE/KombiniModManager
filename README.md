# Kombini Mod Manager

A fast, safe desktop mod manager for Valve's **Deadlock**. It has a Rust core and a native,
GPU-rendered UI built with [GPUI](https://github.com/zed-industries/zed) and gpui-component. It
doesn't use a web view and stays light on memory.

## Download

Download `KombiniModManager-<version>-windows-x64.exe` from the **Releases** page and run it. It's a
single portable exe, so there's no installer and nothing else to set up. Settings, the mod library and
logs live in `%LOCALAPPDATA%\Kombini\ModManager`.

The release exe is compressed with [UPX](https://upx.github.io/) to keep the download small. Some
antivirus products flag UPX-compressed programs out of caution. Each release lists the SHA-256 of the
exe in `SHA256SUMS.txt`, so you can check your download. If you'd rather not run a compressed exe,
[build it from source](#building-from-source).

## Features

- **Library:** install mods from archives (zip, 7z, rar), folders or VPKs. Enable them, drag to reorder
  the load order, group them, and see which files they conflict on. Every change is a journaled
  transaction, and snapshots let you roll back.
- **Profiles:** switch between whole mod setups, each with its own load order and autoexec.
- **Discover:** browse, search and filter GameBanana with infinite scroll. Install mods, whole
  collections or a pasted link. Updates and "may be outdated" mods are flagged after game updates.
- **Heroes:** the game's hero-select grid, showing each hero's active skins. Pick which skin to keep
  when several are enabled.
- **Customize:** a crosshair designer with a live preview, plus a curated list of autoexec commands.
- **Mod tools:** a VPK inspector with sound previews, mod repairs after game updates (missing data,
  outdated hero files), a health check, and a bisect helper for finding the mod that breaks the game.
- **Imports:** bring over a whole library from Deadlock Mod Manager or Grimoire, adopt addon files you
  already have, and read Deadlock Mod Interchange bundles and Grimoire `mp1:` profile codes.

Kombini is not affiliated with Valve or GameBanana.

## Building from source

You need Windows, [Rust](https://rustup.rs/) 1.88 or newer, and the MSVC build tools (the
"Desktop development with C++" workload).

```powershell
cargo run -p kombini-app                            # run the app (debug)
cargo build --profile dist -p kombini-app           # the release exe: target\dist\kombini-mod-manager.exe
cargo run -p kombini-cli -- --help                  # headless CLI for every engine operation
```

The app writes to the game's `game\citadel\addons` folder and `gameinfo.gi` when it opens. To
experiment safely, point it at a copy of the game folder and set `KOMBINI_DATA_DIR` and
`KOMBINI_CACHE_DIR` to keep its library separate. Logs are written to `<data>\logs\app.log`.

## Project layout

| Crate | What it does |
| --- | --- |
| `kombini-core` | Domain model and pure algorithms: load order, pak-slot layout, conflicts, heroes, safety scan |
| `kombini-vpk`, `kombini-gameinfo`, `kombini-archive`, `kombini-steam`, `kombini-storage` | VPK files, `gameinfo.gi`, archive extraction, Steam, SQLite and the blob store |
| `kombini-source2`, `kombini-repair` | Source 2 resources and KV3, and mod repairs (ported from Grimoire's vpkmerge) |
| `kombini-engine` | Install, enable and reorder transactions, snapshots, health, launch, bisect |
| `kombini-gamebanana`, `kombini-download`, `kombini-import` | GameBanana API, download queue, imports and exports |
| `kombini-cli` | `kombini`, a headless harness |
| `kombini-app` | The desktop app |

To go deeper, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/BENCHMARKS.md](docs/BENCHMARKS.md) and the manual
[release checklist](docs/ACCEPTANCE-CHECKLIST.md).

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). To report a security
issue, see [SECURITY.md](SECURITY.md).

## License

MIT, see [LICENSE](LICENSE). Third-party code and credits (Grimoire, vpkmerge, morphic, UnRAR and
others) are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
