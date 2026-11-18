# Third-party notices

## UnRAR

RAR extraction uses the UnRAR source code by Alexander L. Roshal (RARLAB), compiled in via the `unrar` / `unrar_sys` crates.

> The source code of UnRAR utility is freeware. You may use UnRAR source code to handle RAR archives without limitation, but you are not allowed to use it to re-create the RAR compression algorithm, which is proprietary. Distribution of modified UnRAR source code in separate form or as a part of other software is permitted, provided that it is clearly stated in the documentation and source comments that the code may not be used to develop a RAR (WinRAR) compatible archiver.

This application only extracts RAR archives and does not create them. The UnRAR code may not be used to develop a RAR (WinRAR) compatible archiver.

## Grimoire, vpkmerge and morphic (mod repairs)

The Mod issues feature is based on the mod repairs in the Grimoire mod manager
(https://github.com/Slush97/grimoire). Its repair engine vpkmerge
(https://github.com/Slush97/vpkmerge, commit df41586) and the Source 2 library it ships, morphic, were
ported into `crates/kombini-repair` and `crates/kombini-source2`. Both are MIT licensed:

> MIT License
>
> Copyright (c) 2026 esoc
>
> Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
> associated documentation files (the "Software"), to deal in the Software without restriction,
> including without limitation the rights to use, copy, modify, merge, publish, distribute,
> sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or
> substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
> NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
> NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES
> OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
> CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

The exact license files are kept next to the ported code (`LICENSE-vpkmerge`, `LICENSE-morphic`).
morphic's Source 2 resource and KeyValues3 layouts follow ValveResourceFormat (MIT).

## Grimoire (crosshair designer and command list)

The crosshair convars, the game's default values, the preview's layout math and the curated autoexec
command list in the Customize tab come from Grimoire's crosshair editor and command presets
(https://github.com/Slush97/grimoire), MIT License, Copyright (c) 2025 esoc.

## Grimoire (importing from Deadlock Mod Manager)

`kombini-import/src/managers.rs` reads Deadlock Mod Manager's data the way Grimoire's DMM migration
does (`dmmReader.ts`, `dmmState.ts`, `dmmMigration.ts` in https://github.com/Slush97/grimoire),
ported to Rust: the `state.json` envelope, mod id forms, manifest versions, shard folders and the
file lookup fallbacks. MIT License, Copyright (c) 2025 esoc. Nothing from Deadlock Mod Manager's own
source (GPL-3.0) is used; only its on-disk formats are read.

## rodio and Symphonia

Sound previews play through rodio (MIT OR Apache-2.0) with Symphonia decoders (MPL-2.0, used
unmodified as a library).

## Lucide icons

The app embeds a small selection of Lucide icons (via `gpui-kit-assets`), licensed under the ISC License; icons derived from Feather are under the MIT License. Copyright (c) Lucide Contributors 2022; portions Copyright (c) 2013-2022 Cole Bemis.

## GPUI

The desktop UI is built with GPUI (Zed Industries) and GPUI Component / GPUI Kit, licensed under Apache-2.0.

## Other dependencies

All other Rust dependencies are distributed under their own permissive licenses (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0). See `Cargo.lock` and run `cargo about generate` or `cargo deny list` for the full list.

## UPX

Release executables are compressed with UPX (https://upx.github.io/). UPX is GPL-2.0 with a special
exception that allows compressed programs under any license; only its small decompression stub is
part of the released exe.
