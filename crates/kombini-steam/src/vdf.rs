//! Minimal Valve KeyValues (VDF) text parser for libraryfolders.vdf and
//! appmanifest files. Tolerant: malformed input never panics.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvValue {
    Str(String),
    Obj(Vec<(String, KvValue)>),
}

impl KvValue {
    /// Case-insensitive lookup of the first child with this key.
    pub fn get(&self, key: &str) -> Option<&KvValue> {
        self.children()
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            Self::Obj(_) => None,
        }
    }

    pub fn children(&self) -> &[(String, KvValue)] {
        match self {
            Self::Obj(children) => children,
            Self::Str(_) => &[],
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Vec<Token> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '{' {
            tokens.push(Token::Open);
            i += 1;
        } else if c == '}' {
            tokens.push(Token::Close);
            i += 1;
        } else if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    match chars[i + 1] {
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        other => {
                            s.push('\\');
                            s.push(other);
                        }
                    }
                    i += 2;
                } else {
                    s.push(chars[i]);
                    i += 1;
                }
            }
            i += 1; // closing quote (or end of text)
            tokens.push(Token::Str(s));
        } else {
            let mut s = String::new();
            while i < chars.len()
                && !chars[i].is_whitespace()
                && !matches!(chars[i], '{' | '}' | '"')
            {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(Token::Str(s));
        }
    }
    tokens
}

/// Parses VDF text into a top-level object.
pub fn parse_kv(text: &str) -> KvValue {
    let tokens = tokenize(text);
    let mut pos = 0;
    KvValue::Obj(parse_object(&tokens, &mut pos, 0))
}

fn parse_object(tokens: &[Token], pos: &mut usize, depth: usize) -> Vec<(String, KvValue)> {
    let mut out = Vec::new();
    while *pos < tokens.len() {
        match &tokens[*pos] {
            Token::Close => {
                *pos += 1;
                if depth > 0 {
                    return out;
                }
                // Stray close brace at top level: ignore.
            }
            Token::Open => {
                // Object without a key: parse and discard to stay in sync.
                *pos += 1;
                let _ = parse_object(tokens, pos, depth + 1);
            }
            Token::Str(key) => {
                *pos += 1;
                match tokens.get(*pos) {
                    Some(Token::Str(value)) => {
                        out.push((key.clone(), KvValue::Str(value.clone())));
                        *pos += 1;
                    }
                    Some(Token::Open) => {
                        *pos += 1;
                        let children = parse_object(tokens, pos, depth + 1);
                        out.push((key.clone(), KvValue::Obj(children)));
                    }
                    Some(Token::Close) | None => {} // dangling key dropped
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = "\u{feff}\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"c:\\\\program files (x86)\\\\steam\"\n\t\t// comment \"x\"\n\t\t\"apps\" { \"228980\" \"0\" }\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";

    #[test]
    fn parses_library_file() {
        let kv = parse_kv(LIBRARY);
        let folders = kv.get("LibraryFolders").unwrap();
        assert_eq!(folders.children().len(), 2);
        assert_eq!(
            folders.get("0").unwrap().get("path").unwrap().as_str(),
            Some("c:\\program files (x86)\\steam")
        );
        assert_eq!(
            folders
                .get("0")
                .unwrap()
                .get("apps")
                .unwrap()
                .get("228980")
                .unwrap()
                .as_str(),
            Some("0")
        );
    }

    #[test]
    fn escapes_and_unquoted() {
        let kv = parse_kv("key \"a\\\"b\\tc\\q\"\nbare value");
        assert_eq!(kv.get("key").unwrap().as_str(), Some("a\"b\tc\\q"));
        assert_eq!(kv.get("bare").unwrap().as_str(), Some("value"));
    }

    #[test]
    fn malformed_never_panics() {
        for input in [
            "}",
            "{",
            "\"a\" {",
            "\"a\"",
            "\"unterminated",
            "a { b { c } } }",
            "",
        ] {
            let _ = parse_kv(input);
        }
        let kv = parse_kv("} \"k\" \"v\"");
        assert_eq!(kv.get("k").unwrap().as_str(), Some("v"));
    }
}
