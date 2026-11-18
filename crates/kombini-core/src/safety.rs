//! Static safety scan of VPK paths. Never executes anything.

use crate::model::{Level, SafetyCode, SafetyFinding};

const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "dll", "bat", "cmd", "ps1", "scr", "com", "msi", "jar", "vbs", "sh", "py", "pyc", "app",
    "dylib", "so", "lnk", "reg", "js", "hta",
];

const SAMPLE_LIMIT: usize = 20;

pub fn scan_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<SafetyFinding> {
    let mut gameplay = Vec::new();
    let mut skeletons = Vec::new();
    let mut executables = Vec::new();
    for path in paths {
        let lowered = path.replace('\\', "/").to_lowercase();
        if let Some(rest) = lowered.strip_prefix("scripts/")
            && (lowered.ends_with(".vdata_c") || rest.contains('/'))
        {
            gameplay.push(path.to_owned());
        }
        if lowered.ends_with(".vnmskel_c") {
            skeletons.push(path.to_owned());
        }
        let file = lowered.rsplit('/').next().unwrap_or(&lowered);
        if let Some((_, ext)) = file.rsplit_once('.')
            && EXECUTABLE_EXTENSIONS.contains(&ext)
        {
            executables.push(path.to_owned());
        }
    }
    let mut findings = Vec::new();
    let mut push = |level, code, message: &str, mut paths: Vec<String>| {
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        paths.truncate(SAMPLE_LIMIT);
        findings.push(SafetyFinding {
            level,
            code,
            message: message.to_owned(),
            samples: paths,
            count,
        });
    };
    push(
        Level::Error,
        SafetyCode::GameplayData,
        "Ships gameplay data under scripts/. This can break after patches, affect balance, or \
         remove fields Valve added in later builds.",
        gameplay,
    );
    push(
        Level::Warning,
        SafetyCode::Skeleton,
        "Replaces a hero skeleton. Valve changes rigs regularly; old skeletons break animations \
         and can affect other mods.",
        skeletons,
    );
    push(
        Level::Error,
        SafetyCode::Executable,
        "Contains executable or script files. A VPK mod never needs these.",
        executables,
    );
    findings
}

/// Most severe level among findings, or None when clean.
pub fn worst_level(findings: &[SafetyFinding]) -> Option<Level> {
    findings.iter().map(|f| f.level).min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findings() {
        let f = scan_paths([
            "scripts/heroes.vdata_c",
            "scripts/sub/thing.txt",
            "models/heroes/x/x.vnmskel_c",
            "bin/evil.DLL",
            "materials/ok.vmat_c",
            "scripts.txt",
        ]);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].code, SafetyCode::GameplayData);
        assert_eq!(f[0].count, 2);
        assert_eq!(f[2].code, SafetyCode::Executable);
        assert_eq!(worst_level(&f), Some(Level::Error));
        assert!(scan_paths(["materials/a.vmat_c"]).is_empty());
        assert_eq!(worst_level(&[]), None);
    }
}
