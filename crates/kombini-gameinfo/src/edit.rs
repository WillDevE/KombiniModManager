//! Pure text edits. Every function works line by line and keeps each line's original terminator;
//! lines it does not own are never reformatted.

use crate::parse::{
    Analysis, Line, MountLine, analyze, count_braces, is_end_marker, is_start_marker, line_content,
    split_lines,
};
use crate::{MANAGED_END, MANAGED_START, addon_paths};

const DEFAULT_INDENT: &str = "\t\t\t";

/// Why an edit was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    #[error("SearchPaths block not found in gameinfo.gi; the file was left unchanged")]
    MissingSearchPaths,
    #[error("Kombini's gameinfo.gi markers are duplicated or out of order; use Repair markers")]
    MarkersCorrupt,
    #[error("the edited gameinfo.gi failed verification ({0}); the file was left unchanged")]
    VerificationFailed(String),
}

fn leading_whitespace(s: &str) -> &str {
    &s[..s.len() - s.trim_start_matches([' ', '\t']).len()]
}

/// Unconditional, non-compound mount of a kind Kombini owns, outside foreign fences.
fn is_owned(m: &MountLine) -> bool {
    if m.conditional || m.in_foreign_fence || m.roles.len() != 1 {
        return false;
    }
    match m.roles[0].as_str() {
        "game" => crate::is_managed_addon_path(&m.path),
        "mod" | "write" => {
            m.path.eq_ignore_ascii_case("citadel") || m.path.eq_ignore_ascii_case("core")
        }
        _ => false,
    }
}

fn desired_block(indent: &str, shard_count: u8) -> Vec<String> {
    let mut block = vec![format!("{indent}{MANAGED_START}")];
    for path in addon_paths(shard_count) {
        block.push(format!("{indent}Game {path}"));
    }
    for line in ["Mod citadel", "Write citadel", "Mod core", "Write core"] {
        block.push(format!("{indent}{line}"));
    }
    block.push(format!("{indent}{MANAGED_END}"));
    block
}

enum Plan {
    /// Existing block already equals the desired block.
    Keep,
    /// Replace lines `start..=end` with the block.
    Replace {
        start: usize,
        end: usize,
        block: Vec<String>,
    },
    /// Insert the block before line `before`.
    Insert { before: usize, block: Vec<String> },
}

fn plan_block(
    text: &str,
    lines: &[Line],
    a: &Analysis,
    shard_count: u8,
) -> Result<Plan, EditError> {
    if let Some((start, end)) = a.managed_block {
        let indent = leading_whitespace(line_content(text, &lines[start]));
        let block = desired_block(indent, shard_count);
        let existing: Vec<&str> = (start..=end)
            .map(|k| line_content(text, &lines[k]))
            .collect();
        if existing
            .iter()
            .copied()
            .eq(block.iter().map(String::as_str))
        {
            return Ok(Plan::Keep);
        }
        return Ok(Plan::Replace { start, end, block });
    }

    let citadel = a
        .mounts
        .iter()
        .find(|m| !m.conditional && m.roles == ["game"] && m.path.eq_ignore_ascii_case("citadel"));
    if let Some(m) = citadel {
        let indent = leading_whitespace(line_content(text, &lines[m.line_index]));
        let indent = if indent.is_empty() {
            DEFAULT_INDENT
        } else {
            indent
        };
        return Ok(Plan::Insert {
            before: m.line_index,
            block: desired_block(indent, shard_count),
        });
    }

    let (body_start, body_end) = a.search_paths_body.ok_or(EditError::MissingSearchPaths)?;
    let open_line = lines.partition_point(|l| l.next < body_start);
    let close_line = lines.partition_point(|l| l.next <= body_end);
    if open_line >= close_line {
        return Err(EditError::VerificationFailed(
            "the SearchPaths braces share one line".to_string(),
        ));
    }
    let indent = (open_line + 1..close_line)
        .map(|k| line_content(text, &lines[k]))
        .find(|c| !c.trim().is_empty())
        .map(leading_whitespace)
        .unwrap_or(DEFAULT_INDENT);
    Ok(Plan::Insert {
        before: open_line + 1,
        block: desired_block(indent, shard_count),
    })
}

fn language_lines(text: &str, a: &Analysis) -> Vec<String> {
    let lines = split_lines(text);
    a.mounts
        .iter()
        .filter(|m| {
            m.key.eq_ignore_ascii_case("game_uilanguage")
                || m.key.eq_ignore_ascii_case("game_lowviolence")
        })
        .map(|m| line_content(text, &lines[m.line_index]).to_string())
        .collect()
}

fn verify(
    input: &str,
    input_analysis: &Analysis,
    output: &str,
    shard_count: u8,
) -> Result<(), EditError> {
    let fail = |reason: &str| Err(EditError::VerificationFailed(reason.to_string()));
    let b = analyze(output);
    if !b.patched {
        return fail("the result is not patched");
    }
    if !b.markers_ok || b.managed_block.is_none() {
        return fail("the result does not contain exactly one managed block");
    }
    let block: Vec<(&str, &str)> = b
        .mounts
        .iter()
        .filter(|m| m.in_managed_block)
        .map(|m| (m.key.as_str(), m.path.as_str()))
        .collect();
    let mut expected: Vec<(String, String)> = addon_paths(shard_count)
        .into_iter()
        .map(|p| ("Game".to_string(), p))
        .collect();
    for (key, path) in [
        ("Mod", "citadel"),
        ("Write", "citadel"),
        ("Mod", "core"),
        ("Write", "core"),
    ] {
        expected.push((key.to_string(), path.to_string()));
    }
    if !block
        .iter()
        .copied()
        .eq(expected.iter().map(|(k, p)| (k.as_str(), p.as_str())))
    {
        return fail("the managed block does not contain the expected mounts");
    }
    if language_lines(input, input_analysis) != language_lines(output, &b) {
        return fail("Game_UILanguage/Game_LowViolence lines changed");
    }
    if count_braces(input) != count_braces(output) {
        return fail("brace balance changed");
    }
    Ok(())
}

/// Returns the new text (unchanged text if already correct). `shard_count` is clamped to 1..=10.
pub fn ensure_text(text: &str, shard_count: u8, crlf: bool) -> Result<String, EditError> {
    let a = analyze(text);
    if !a.search_paths_found {
        return Err(EditError::MissingSearchPaths);
    }
    if !a.markers_ok {
        return Err(EditError::MarkersCorrupt);
    }
    let lines = split_lines(text);
    let newline = if crlf { "\r\n" } else { "\n" };

    let mut remove = vec![false; lines.len()];
    for m in &a.mounts {
        if !m.in_managed_block && is_owned(m) {
            remove[m.line_index] = true;
        }
    }
    let plan = plan_block(text, &lines, &a, shard_count)?;

    let mut out = String::with_capacity(text.len() + 512);
    let push_block = |out: &mut String, block: &[String]| {
        for line in block {
            out.push_str(line);
            out.push_str(newline);
        }
    };
    for (k, line) in lines.iter().enumerate() {
        match &plan {
            Plan::Insert { before, block } if *before == k => push_block(&mut out, block),
            Plan::Replace { start, end, block } => {
                if k == *start {
                    push_block(&mut out, block);
                }
                if (*start..=*end).contains(&k) {
                    continue;
                }
            }
            _ => {}
        }
        if remove[k] {
            continue;
        }
        out.push_str(&text[line.start..line.next]);
    }

    verify(text, &a, &out, shard_count)?;
    Ok(out)
}

/// Removes our marker lines and everything between each pair; no-op if none. Never touches other lines.
pub fn unpatch_text(text: &str) -> String {
    let lines = split_lines(text);
    let last_end = lines
        .iter()
        .rposition(|l| is_end_marker(line_content(text, l)));
    let any_start = lines.iter().any(|l| is_start_marker(line_content(text, l)));
    if last_end.is_none() && !any_start {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for (k, line) in lines.iter().enumerate() {
        let content = line_content(text, line);
        if is_start_marker(content) {
            // Only open a region when an end marker follows; an unpaired start drops just itself.
            if last_end.is_some_and(|e| k < e) {
                inside = true;
            }
            continue;
        }
        if is_end_marker(content) {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(&text[line.start..line.next]);
        }
    }
    out
}

/// Removes ALL our marker lines and the mounts between any start/end pair (tolerant of corruption),
/// then runs [`ensure_text`].
pub fn repair_markers_text(text: &str, shard_count: u8, crlf: bool) -> Result<String, EditError> {
    let lines = split_lines(text);
    let mut stripped = String::with_capacity(text.len());
    for line in &lines {
        let content = line_content(text, line);
        if is_start_marker(content) || is_end_marker(content) {
            continue;
        }
        stripped.push_str(&text[line.start..line.next]);
    }
    // Once the markers are gone, every unconditional addon/Mod/Write mount of ours that sat between
    // them (or anywhere else outside a foreign fence) is removed by `ensure_text`.
    ensure_text(&stripped, shard_count, crlf)
}

/// Removes unconditional, non-compound addon Game mounts inside foreign fences (explicit user action only).
pub fn remove_other_manager_mounts_text(text: &str) -> String {
    let a = analyze(text);
    let targets: Vec<usize> = a
        .mounts
        .iter()
        .filter(|m| {
            m.in_foreign_fence
                && !m.conditional
                && m.roles == ["game"]
                && crate::is_managed_addon_path(&m.path)
        })
        .map(|m| m.line_index)
        .collect();
    if targets.is_empty() {
        return text.to_string();
    }
    let lines = split_lines(text);
    let mut out = String::with_capacity(text.len());
    for (k, line) in lines.iter().enumerate() {
        if !targets.contains(&k) {
            out.push_str(&text[line.start..line.next]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: &str = "\"GameInfo\"\n{\n\tFileSystem\n\t{\n\t\tSearchPaths\n\t\t{\n\t\t\tGame_UILanguage\tcitadel_*LANGUAGE*\n\n\t\t\tGame\tcitadel\n\t\t\tGame\tcore\n\t\t}\n\t}\n}\n";

    #[test]
    fn ensure_inserts_above_citadel_and_unpatch_restores() {
        let patched = ensure_text(STOCK, 1, false).unwrap();
        assert!(patched.contains(
            "\t\t\t// >>> Kombini Mod Manager (managed block) >>>\n\t\t\tGame citadel/addons\n\t\t\tMod citadel\n\t\t\tWrite citadel\n\t\t\tMod core\n\t\t\tWrite core\n\t\t\t// <<< Kombini Mod Manager <<<\n\t\t\tGame\tcitadel\n"
        ));
        assert_eq!(ensure_text(&patched, 1, false).unwrap(), patched);
        assert_eq!(unpatch_text(&patched), STOCK);
    }

    #[test]
    fn missing_search_paths_is_an_error() {
        assert_eq!(
            ensure_text("\"GameInfo\"\n{\n}\n", 1, false),
            Err(EditError::MissingSearchPaths)
        );
    }

    #[test]
    fn insertion_after_open_brace_when_no_citadel_mount() {
        let text = "SearchPaths\n{\n\tGame\tcore\n}\n";
        let out = ensure_text(text, 1, false).unwrap();
        assert!(out.starts_with("SearchPaths\n{\n\t// >>> Kombini"), "{out}");
        assert!(analyze(&out).patched);
    }

    #[test]
    fn single_line_block_is_refused() {
        let err = ensure_text("SearchPaths { Game core }\n", 1, false).unwrap_err();
        assert!(matches!(err, EditError::VerificationFailed(_)));
    }

    #[test]
    fn unpatch_without_markers_is_noop() {
        assert_eq!(unpatch_text(STOCK), STOCK);
    }

    #[test]
    fn unpaired_start_marker_drops_only_itself() {
        let text = format!("a\n{MANAGED_START}\nb\n");
        assert_eq!(unpatch_text(&text), "a\nb\n");
    }
}
