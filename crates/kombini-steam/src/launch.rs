//! Launching Deadlock through Steam and launch-option helpers.

use crate::SteamError;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMethod {
    SteamExe,
    SteamUri,
}

/// Splits on whitespace, keeping double-quoted segments together (quotes removed).
pub fn split_launch_options(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    out.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        out.push(current);
    }
    out
}

fn quote(token: &str) -> String {
    if token.chars().any(char::is_whitespace) || token.is_empty() {
        format!("\"{token}\"")
    } else {
        token.to_owned()
    }
}

fn is_flag(token: &str) -> bool {
    token.starts_with('-') || token.starts_with('+')
}

/// Groups tokens into units: a flag followed by a non-flag value is one unit.
fn units(tokens: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if is_flag(&tokens[i]) && i + 1 < tokens.len() && !is_flag(&tokens[i + 1]) {
            out.push(vec![tokens[i].clone(), tokens[i + 1].clone()]);
            i += 2;
        } else {
            out.push(vec![tokens[i].clone()]);
            i += 1;
        }
    }
    out
}

fn unit_key(unit: &[String]) -> String {
    unit.join(" ").to_lowercase()
}

/// Appends units of `add` not already present (case-insensitive). Tokens that
/// contain whitespace are re-quoted in the output.
pub fn merge_launch_options(existing: &str, add: &str) -> String {
    let mut merged = units(&split_launch_options(existing));
    let present: std::collections::BTreeSet<String> = merged.iter().map(|u| unit_key(u)).collect();
    for unit in units(&split_launch_options(add)) {
        if !present.contains(&unit_key(&unit)) {
            merged.push(unit);
        }
    }
    merged
        .iter()
        .flatten()
        .map(|t| quote(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when the options already execute autoexec with the console enabled.
pub fn has_autoexec_options(options: &str) -> bool {
    let tokens: Vec<String> = split_launch_options(options)
        .into_iter()
        .map(|t| t.to_lowercase())
        .collect();
    let console = tokens.iter().any(|t| t == "-console");
    let exec = tokens.windows(2).any(|w| {
        (w[0] == "-exec" || w[0] == "+exec") && w[1].trim_end_matches(".cfg") == "autoexec"
    });
    console && exec
}

pub fn steam_executable(steam_dir: &Path) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["steam.exe"]
    } else {
        &["steam.sh", "steam"]
    };
    names
        .iter()
        .map(|n| steam_dir.join(n))
        .find(|p| p.is_file())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Steam URI used when the Steam executable cannot be found.
pub fn launch_uri(options: &str) -> String {
    format!(
        "steam://run/{}//{}/",
        crate::APP_ID,
        percent_encode(options.trim())
    )
}

/// Starts Deadlock through Steam (never directly), detached.
pub fn launch(steam_dir: Option<&Path>, options: &str) -> Result<LaunchMethod, SteamError> {
    let args = split_launch_options(options);
    if let Some(exe) = steam_dir.and_then(steam_executable) {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("-applaunch")
            .arg(crate::APP_ID.to_string())
            .args(&args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.spawn()
            .map_err(|e| SteamError::Launch(format!("{}: {e}", exe.display())))?;
        return Ok(LaunchMethod::SteamExe);
    }
    let uri = launch_uri(options);
    let opener = if cfg!(windows) {
        "explorer.exe"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&uri)
        .spawn()
        .map_err(|e| SteamError::Launch(format!("{opener} {uri}: {e}")))?;
    Ok(LaunchMethod::SteamUri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(
            split_launch_options(r#"-novid  -console "+exec my cfg""#),
            vec!["-novid", "-console", "+exec my cfg"]
        );
        assert!(split_launch_options("   ").is_empty());
        assert_eq!(split_launch_options(r#""""#), vec![""]);
    }

    #[test]
    fn merge() {
        let merged = merge_launch_options("-novid", "-console -exec autoexec");
        assert_eq!(merged, "-novid -console -exec autoexec");
        assert_eq!(
            merge_launch_options(&merged, "-console -exec autoexec"),
            merged
        );
        assert_eq!(
            merge_launch_options("-EXEC autoexec", "-exec autoexec"),
            "-EXEC autoexec"
        );
        assert_eq!(
            merge_launch_options(r#""+exec my cfg""#, "-novid"),
            r#""+exec my cfg" -novid"#
        );
        assert!(has_autoexec_options(&merged));
        assert!(!has_autoexec_options("-novid"));
    }

    #[test]
    fn uri() {
        assert_eq!(
            launch_uri("-console -exec autoexec"),
            "steam://run/1422450//-console%20-exec%20autoexec/"
        );
    }
}
