//! Pure, IO-free analysis of gameinfo.gi text.
//!
//! The scanner is hand written: it understands `//` line comments, `/* */` block comments and
//! `"..."` strings with `\` escapes, which is all the KeyValues3-ish dialect of gameinfo.gi needs.

use serde::Serialize;

use crate::{MANAGED_END, MANAGED_START, is_managed_addon_path};

const REQUIRED_ROLES: [(&str, &str); 4] = [
    ("Mod", "citadel"),
    ("Write", "citadel"),
    ("Mod", "core"),
    ("Write", "core"),
];

/// One direct key/value line inside `SearchPaths`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MountLine {
    /// 0-based line index in the whole text.
    pub line_index: usize,
    /// As written, quotes stripped, e.g. "Game", "Game+Mod", "Game_UILanguage".
    pub key: String,
    /// Key split on '+', lowercased: ["game"], ["game", "mod"].
    pub roles: Vec<String>,
    /// Value, quotes stripped.
    pub path: String,
    /// The line has a trailing `[$...]` condition after the value.
    pub conditional: bool,
    pub in_managed_block: bool,
    pub in_foreign_fence: bool,
}

/// Result of [`analyze`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Analysis {
    pub search_paths_found: bool,
    /// Byte range strictly inside the braces.
    pub search_paths_body: Option<(usize, usize)>,
    /// Direct key/value lines inside SearchPaths only.
    pub mounts: Vec<MountLine>,
    /// Line indices of start and end marker (inclusive).
    pub managed_block: Option<(usize, usize)>,
    /// False if markers are duplicated, unpaired, out of order, or outside SearchPaths.
    pub markers_ok: bool,
    /// Unconditional Game-role mounts with `is_managed_addon_path`, in file order.
    pub addon_paths: Vec<String>,
    /// First unconditional addon Game mount appears before the first unconditional `Game citadel`.
    pub addons_above_citadel: bool,
    /// Subset of ["Mod citadel", "Write citadel", "Mod core", "Write core"] nothing satisfies.
    pub missing_roles: Vec<String>,
    /// Unconditional addon Game mounts outside our managed block and outside foreign fences.
    pub duplicate_addon_mounts: usize,
    /// "DMM", "unknown" or None.
    pub other_manager: Option<String>,
    /// A `Game_Language` key exists.
    pub obsolete_game_language: bool,
    pub patched: bool,
}

/// One line of the text: `text[start..end]` is the content, `text[start..next]` includes the
/// terminator (`\n` or `\r\n`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Line {
    pub start: usize,
    pub end: usize,
    pub next: usize,
}

pub(crate) fn split_lines(text: &str) -> Vec<Line> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let (end, next) = match bytes[start..].iter().position(|&b| b == b'\n') {
            Some(offset) => {
                let newline = start + offset;
                let end = if newline > start && bytes[newline - 1] == b'\r' {
                    newline - 1
                } else {
                    newline
                };
                (end, newline + 1)
            }
            None => (bytes.len(), bytes.len()),
        };
        lines.push(Line { start, end, next });
        start = next;
    }
    lines
}

pub(crate) fn line_content<'a>(text: &'a str, line: &Line) -> &'a str {
    &text[line.start..line.end]
}

pub(crate) fn is_start_marker(content: &str) -> bool {
    content.trim() == MANAGED_START
}

pub(crate) fn is_end_marker(content: &str) -> bool {
    content.trim() == MANAGED_END
}

// ---------------------------------------------------------------- tokenizer

enum Token {
    Open(usize),
    Close(usize),
    Word(usize, usize),
    Str,
}

fn is_word_end(b: &[u8], i: usize) -> bool {
    let c = b[i];
    c.is_ascii_whitespace()
        || c == b'{'
        || c == b'}'
        || c == b'"'
        || (c == b'/' && matches!(b.get(i + 1), Some(b'/' | b'*')))
}

/// Next significant token at or after `*i`, skipping whitespace and comments.
fn next_token(b: &[u8], i: &mut usize) -> Option<Token> {
    loop {
        if *i >= b.len() {
            return None;
        }
        let c = b[*i];
        let n = b.get(*i + 1).copied();
        if c.is_ascii_whitespace() {
            *i += 1;
        } else if c == b'/' && n == Some(b'/') {
            while *i < b.len() && b[*i] != b'\n' {
                *i += 1;
            }
        } else if c == b'/' && n == Some(b'*') {
            *i += 2;
            while *i < b.len() && !(b[*i] == b'*' && b.get(*i + 1) == Some(&b'/')) {
                *i += 1;
            }
            *i = (*i + 2).min(b.len());
        } else if c == b'"' {
            *i += 1;
            while *i < b.len() {
                match b[*i] {
                    b'\\' => *i += 2,
                    b'"' => {
                        *i += 1;
                        break;
                    }
                    _ => *i += 1,
                }
            }
            *i = (*i).min(b.len());
            return Some(Token::Str);
        } else if c == b'{' {
            *i += 1;
            return Some(Token::Open(*i - 1));
        } else if c == b'}' {
            *i += 1;
            return Some(Token::Close(*i - 1));
        } else {
            let start = *i;
            while *i < b.len() && !is_word_end(b, *i) {
                *i += 1;
            }
            return Some(Token::Word(start, *i));
        }
    }
}

/// Index of the `}` matching the `{` at `open`, ignoring braces in comments and strings.
fn find_matching_brace(b: &[u8], open: usize) -> Option<usize> {
    let mut i = open;
    let mut depth = 0i32;
    while let Some(token) = next_token(b, &mut i) {
        match token {
            Token::Open(_) => depth += 1,
            Token::Close(pos) => {
                depth -= 1;
                if depth == 0 {
                    return Some(pos);
                }
            }
            Token::Word(..) | Token::Str => {}
        }
    }
    None
}

/// `(open, close)` byte positions of the first `SearchPaths { ... }` key.
fn find_search_paths(text: &str) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let mut i = 0;
    while let Some(token) = next_token(b, &mut i) {
        let Token::Word(start, end) = token else {
            continue;
        };
        if text[start..end].eq_ignore_ascii_case("SearchPaths") {
            let mut j = i;
            if let Some(Token::Open(open)) = next_token(b, &mut j) {
                return find_matching_brace(b, open).map(|close| (open, close));
            }
        }
    }
    None
}

/// Number of `{` and `}` outside comments and strings.
pub(crate) fn count_braces(text: &str) -> (usize, usize) {
    let b = text.as_bytes();
    let mut i = 0;
    let (mut open, mut close) = (0, 0);
    while let Some(token) = next_token(b, &mut i) {
        match token {
            Token::Open(_) => open += 1,
            Token::Close(_) => close += 1,
            Token::Word(..) | Token::Str => {}
        }
    }
    (open, close)
}

// ------------------------------------------------------------- line scanner

#[derive(Default)]
struct ScanState {
    depth: i32,
    in_block: bool,
}

/// Advances `state` over one line segment. Returns true when the segment is "complex": it began
/// inside a block comment or contains a brace or a block comment opener.
fn advance(seg: &str, state: &mut ScanState) -> bool {
    let b = seg.as_bytes();
    let mut i = 0;
    let mut complex = state.in_block;
    while i < b.len() {
        if state.in_block {
            if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                state.in_block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => break,
            b'/' if b.get(i + 1) == Some(&b'*') => {
                state.in_block = true;
                complex = true;
                i += 2;
            }
            b'"' => {
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'{' => {
                state.depth += 1;
                complex = true;
                i += 1;
            }
            b'}' => {
                state.depth -= 1;
                complex = true;
                i += 1;
            }
            _ => i += 1,
        }
    }
    complex
}

/// Reads one optionally quoted token. Returns the unquoted token and the remaining text.
fn read_token(s: &str) -> Option<(&str, &str)> {
    if let Some(rest) = s.strip_prefix('"') {
        let b = rest.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'"' => {
                    let after = &rest[i + 1..];
                    if after.is_empty() || after.starts_with(|c: char| c.is_ascii_whitespace()) {
                        return Some((&rest[..i], after));
                    }
                    return None;
                }
                _ => i += 1,
            }
        }
        return None;
    }
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && !is_word_end(b, i) {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    Some((&s[..i], &s[i..]))
}

/// Parses `KEY WS VALUE [WS CONDITION] [WS // comment]`. Returns `(key, value, conditional)`.
fn parse_mount(seg: &str) -> Option<(String, String, bool)> {
    let (key, rest) = read_token(seg.trim_start())?;
    let (value, rest) = read_token(rest.trim_start())?;
    let rest = rest.trim_start();
    let conditional = if rest.is_empty() || rest.starts_with("//") {
        false
    } else if rest.starts_with('[') {
        let close = rest.find(']')?;
        let after = rest[close + 1..].trim_start();
        if !(after.is_empty() || after.starts_with("//")) {
            return None;
        }
        true
    } else {
        return None;
    };
    Some((key.to_string(), value.to_string(), conditional))
}

// ----------------------------------------------------------------- analysis

/// Analyses the text. Pure; never fails.
pub fn analyze(text: &str) -> Analysis {
    let lines = split_lines(text);

    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for (k, line) in lines.iter().enumerate() {
        let content = line_content(text, line);
        if is_start_marker(content) {
            starts.push(k);
        } else if is_end_marker(content) {
            ends.push(k);
        }
    }

    let span = find_search_paths(text);
    let mut mounts: Vec<MountLine> = Vec::new();
    let mut dmm = false;
    let mut any_fence = false;

    if let Some((open, close)) = span {
        let body_start = open + 1;
        let body_end = close;
        let first = lines.partition_point(|l| l.next <= open);
        let last = lines.partition_point(|l| l.next <= close);
        let mut state = ScanState::default();
        let mut in_managed = false;
        let mut fence_open = false;
        for (k, line) in lines.iter().copied().enumerate().take(last + 1).skip(first) {
            let seg_start = line.start.max(body_start);
            let seg_end = line.end.min(body_end);
            let seg = if seg_start < seg_end {
                &text[seg_start..seg_end]
            } else {
                ""
            };
            let eligible = line.start >= body_start && line.end <= body_end;
            let before_depth = state.depth;
            let before_block = state.in_block;
            let complex = advance(seg, &mut state);
            if !eligible {
                continue;
            }
            if seg.to_ascii_lowercase().contains("dmm-search-path") {
                dmm = true;
            }
            if is_start_marker(seg) {
                in_managed = true;
                continue;
            }
            if is_end_marker(seg) {
                in_managed = false;
                continue;
            }
            if before_depth != 0 || before_block {
                continue;
            }
            if seg.trim_start().starts_with("//") {
                let upper = seg.to_uppercase();
                if fence_open {
                    if upper.contains("END") || upper.contains("<<<") {
                        fence_open = false;
                        if upper.contains("DMM") {
                            dmm = true;
                        }
                    }
                } else if upper.contains("BEGIN")
                    || upper.contains("START")
                    || upper.contains(">>>")
                {
                    fence_open = true;
                    any_fence = true;
                    if upper.contains("DMM") {
                        dmm = true;
                    }
                }
                continue;
            }
            if complex {
                continue;
            }
            if let Some((key, path, conditional)) = parse_mount(seg) {
                let roles = key
                    .split('+')
                    .map(|r| r.trim().to_ascii_lowercase())
                    .collect();
                mounts.push(MountLine {
                    line_index: k,
                    key,
                    roles,
                    path,
                    conditional,
                    in_managed_block: in_managed,
                    in_foreign_fence: fence_open,
                });
            }
        }
    }

    let markers_ok = if starts.is_empty() && ends.is_empty() {
        true
    } else if let (Some((open, close)), [s], [e]) = (span, starts.as_slice(), ends.as_slice()) {
        let inside = |k: usize| lines[k].start > open && lines[k].end <= close;
        s < e && inside(*s) && inside(*e)
    } else {
        false
    };
    let managed_block = if markers_ok && !starts.is_empty() {
        Some((starts[0], ends[0]))
    } else {
        None
    };

    let has_role = |m: &MountLine, role: &str| m.roles.iter().any(|r| r == role);
    let is_addon =
        |m: &MountLine| !m.conditional && has_role(m, "game") && is_managed_addon_path(&m.path);

    let addon_paths: Vec<String> = mounts
        .iter()
        .filter(|m| is_addon(m))
        .map(|m| m.path.clone())
        .collect();
    let first_addon = mounts.iter().position(is_addon);
    let first_citadel = mounts.iter().position(|m| {
        !m.conditional && has_role(m, "game") && m.path.eq_ignore_ascii_case("citadel")
    });
    let addons_above_citadel = match (first_addon, first_citadel) {
        (Some(_), None) => true,
        (Some(a), Some(c)) => a < c,
        _ => false,
    };
    let missing_roles: Vec<String> = REQUIRED_ROLES
        .iter()
        .filter(|(role, path)| {
            let role_lower = role.to_ascii_lowercase();
            !mounts.iter().any(|m| {
                !m.conditional && has_role(m, &role_lower) && m.path.eq_ignore_ascii_case(path)
            })
        })
        .map(|(role, path)| format!("{role} {path}"))
        .collect();
    let duplicate_addon_mounts = mounts
        .iter()
        .filter(|m| is_addon(m) && !m.in_managed_block && !m.in_foreign_fence)
        .count();
    let other_manager = if dmm {
        Some("DMM".to_string())
    } else if any_fence {
        Some("unknown".to_string())
    } else {
        None
    };
    let obsolete_game_language = mounts
        .iter()
        .any(|m| m.key.eq_ignore_ascii_case("game_language"));
    let search_paths_found = span.is_some();
    let patched = search_paths_found
        && markers_ok
        && !addon_paths.is_empty()
        && addons_above_citadel
        && missing_roles.is_empty();

    Analysis {
        search_paths_found,
        search_paths_body: span.map(|(open, close)| (open + 1, close)),
        mounts,
        managed_block,
        markers_ok,
        addon_paths,
        addons_above_citadel,
        missing_roles,
        duplicate_addon_mounts,
        other_manager,
        obsolete_game_language,
        patched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(body: &str) -> String {
        format!(
            "\"GameInfo\"\n{{\n\tFileSystem\n\t{{\n\t\tSearchPaths\n\t\t{{\n{body}\t\t}}\n\t}}\n}}\n"
        )
    }

    #[test]
    fn splits_lines_with_terminators() {
        let text = "a\r\nb\nc";
        let lines = split_lines(text);
        assert_eq!(lines.len(), 3);
        assert_eq!(line_content(text, &lines[0]), "a");
        assert_eq!(&text[lines[0].start..lines[0].next], "a\r\n");
        assert_eq!(line_content(text, &lines[1]), "b");
        assert_eq!(line_content(text, &lines[2]), "c");
        assert_eq!(lines[2].next, text.len());
    }

    #[test]
    fn parses_mount_variants() {
        assert_eq!(
            parse_mount("\t\tGame\t\tcitadel"),
            Some(("Game".into(), "citadel".into(), false))
        );
        assert_eq!(
            parse_mount("\"Game\" \"citadel/addons\" // note"),
            Some(("Game".into(), "citadel/addons".into(), false))
        );
        assert_eq!(
            parse_mount("Game citadel/addons [$WIN64]"),
            Some(("Game".into(), "citadel/addons".into(), true))
        );
        assert_eq!(
            parse_mount("Game citadel/addons [$WIN64 && $X] // c"),
            Some(("Game".into(), "citadel/addons".into(), true))
        );
        assert_eq!(parse_mount("Game citadel extra"), None);
        assert_eq!(parse_mount("// only comment"), None);
        assert_eq!(parse_mount("   "), None);
        assert_eq!(parse_mount("Game"), None);
    }

    #[test]
    fn stock_body_is_analyzed() {
        let text = wrap(
            "\t\t\tGame_UILanguage\t\tcitadel_*LANGUAGE*\n\t\t\tGame\tcitadel\n\t\t\tGame\tcore\n",
        );
        let a = analyze(&text);
        assert!(a.search_paths_found);
        assert_eq!(a.mounts.len(), 3);
        assert_eq!(a.mounts[0].roles, vec!["game_uilanguage"]);
        assert!(!a.patched);
        assert!(a.markers_ok);
        assert_eq!(a.missing_roles.len(), 4);
        assert!(a.addon_paths.is_empty());
        assert!(!a.addons_above_citadel);
    }

    #[test]
    fn searchpaths_in_comment_is_ignored() {
        let text = format!(
            "// SearchPaths {{ Game citadel/addons }}\n/* SearchPaths\n{{ */\n{}",
            wrap("\t\t\tGame\tcitadel\n")
        );
        let a = analyze(&text);
        assert!(a.search_paths_found);
        assert_eq!(a.mounts.len(), 1);
        assert_eq!(a.mounts[0].path, "citadel");
    }

    #[test]
    fn nested_blocks_and_comments_are_not_mounts() {
        let text = wrap(
            "\t\t\t// Game citadel/addons\n\t\t\t/* Game citadel/addons2\n\t\t\tGame citadel/addons3 */\n\t\t\tNested\n\t\t\t{\n\t\t\t\tGame citadel/addons4\n\t\t\t}\n\t\t\tGame citadel\n",
        );
        let a = analyze(&text);
        let paths: Vec<_> = a.mounts.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, vec!["citadel"]);
    }

    #[test]
    fn markers_and_block_are_detected() {
        let body = format!(
            "\t\t\t{MANAGED_START}\n\t\t\tGame citadel/addons\n\t\t\tMod citadel\n\t\t\tWrite citadel\n\t\t\tMod core\n\t\t\tWrite core\n\t\t\t{MANAGED_END}\n\t\t\tGame citadel\n"
        );
        let a = analyze(&wrap(&body));
        assert_eq!(a.managed_block, Some((6, 12)));
        assert!(a.markers_ok);
        assert!(a.patched);
        assert_eq!(a.duplicate_addon_mounts, 0);
        assert!(a.mounts.iter().take(5).all(|m| m.in_managed_block));
        assert!(!a.mounts[5].in_managed_block);
    }

    #[test]
    fn markers_outside_searchpaths_are_not_ok() {
        let text = format!("{MANAGED_START}\n{}", wrap("\t\t\tGame citadel\n"));
        let a = analyze(&text);
        assert!(!a.markers_ok);
        assert_eq!(a.managed_block, None);
    }

    #[test]
    fn unclosed_fence_extends_to_end_of_body() {
        let text = wrap("\t\t\t// BEGIN other\n\t\t\tGame citadel/addons\n\t\t\tGame citadel\n");
        let a = analyze(&text);
        assert!(a.mounts.iter().all(|m| m.in_foreign_fence));
        assert_eq!(a.other_manager.as_deref(), Some("unknown"));
    }

    #[test]
    fn counts_braces_outside_comments_and_strings() {
        assert_eq!(count_braces("{ \"}\" // }\n /* { */ }"), (1, 1));
    }
}
