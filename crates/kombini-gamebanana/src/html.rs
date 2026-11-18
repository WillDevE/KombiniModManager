//! Minimal, safe HTML → plain text for mod descriptions. No markup survives,
//! so nothing from GameBanana can inject UI.

/// Converts description HTML to readable text: block tags become line
/// breaks, list items get bullets, entities are decoded, and runs of blank
/// lines collapse.
pub fn to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.char_indices().peekable();
    let mut skip_until: Option<&'static str> = None;
    while let Some((i, c)) = chars.next() {
        if c == '<' {
            let rest = &html[i + 1..];
            let end = rest.find('>').map(|e| i + 1 + e);
            let Some(end) = end else {
                out.push('<');
                continue;
            };
            let tag = html[i + 1..end].trim().to_ascii_lowercase();
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let closing = tag.starts_with('/');
            if let Some(until) = skip_until {
                if closing && name == until {
                    skip_until = None;
                }
            } else {
                match name.as_str() {
                    "script" | "style" if !closing => {
                        skip_until = Some(if name == "script" { "script" } else { "style" })
                    }
                    "br" | "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr"
                    | "blockquote" | "ul" | "ol" | "hr" => out.push('\n'),
                    "li" if !closing => out.push_str("\n• "),
                    "td" | "th" if !closing => out.push(' '),
                    _ => {}
                }
            }
            while chars.peek().is_some_and(|(j, _)| *j <= end) {
                chars.next();
            }
            continue;
        }
        if skip_until.is_some() {
            continue;
        }
        if c == '&' {
            let rest = &html[i..];
            if let Some(semi) = rest[..rest.len().min(12)].find(';') {
                let entity = &rest[1..semi];
                if let Some(decoded) = decode_entity(entity) {
                    out.push(decoded);
                    while chars.peek().is_some_and(|(j, _)| *j <= i + semi) {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        out.push(if c == '\r' || c == '\t' { ' ' } else { c });
    }
    tidy(&out)
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        "ndash" => Some('–'),
        "mdash" => Some('—'),
        "hellip" => Some('…'),
        _ => {
            let num = entity.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code).filter(|c| !c.is_control() || *c == '\n')
        }
    }
}

fn tidy(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut blank = 0;
    for line in text.lines() {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            blank += 1;
            if blank > 1 || lines.is_empty() {
                continue;
            }
        } else {
            blank = 0;
        }
        lines.push(collapsed);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::to_text;

    #[test]
    fn converts() {
        let html = "<h2>Install</h2><p>Put the <b>file</b> in &quot;addons&quot;&nbsp;&amp; go.</p>\
                    <ul><li>One</li><li>Two &#8211; 2</li></ul><script>alert(1)</script><br><br><br>End";
        assert_eq!(
            to_text(html),
            "Install\n\nPut the file in \"addons\" & go.\n\n• One\n• Two – 2\n\nEnd"
        );
        assert_eq!(to_text("a < b"), "a < b");
        assert_eq!(to_text("&bogus; &#0;"), "&bogus; &#0;");
    }
}
