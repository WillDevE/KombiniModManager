# Test fixtures

All files here are tiny RAR archives copied unmodified from the `data/` folder of the
[`unrar` crate](https://crates.io/crates/unrar) (`unrar-0.5.8`, <https://github.com/muja/unrar.rs>).
The Rust bindings of that crate are licensed `MIT OR Apache-2.0`; these fixture files are part of that
published crate source.

| File | Origin (`unrar-0.5.8/data/`) | Size | Purpose |
| --- | --- | --- | --- |
| `probe.rar` | `version.rar` | 87 bytes | Embedded with `include_bytes!` by `backend_status()` to prove the built-in UnRAR can list an archive. Contains one file, `VERSION` (`unrar-0.4.0`). Also used for the built-in RAR extraction test. |
| `crypted.rar` | `crypted.rar` | 109 bytes | RAR with an encrypted entry (`.gitignore`); used to test that RAR encryption maps to `ArchiveError::Encrypted`. |
| `100M.part00002.rar` | `100M.part00002.rar` | 1886 bytes | A non-first volume of a multi-volume set; used to test `ArchiveError::MissingVolume`. |
