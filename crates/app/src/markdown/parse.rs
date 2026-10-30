//! Discord-flavoured markdown parser.
//!
//! Supports the syntax the official client renders: bold, italic, underline,
//! strikethrough, spoilers, inline code, code blocks, block quotes (`>`/`>>>`),
//! headings (`#`, `##`, `###`), subtext (`-#`), lists, masked links, autolinks,
//! mentions, custom emoji and `<t:…>` timestamps.

use ferrocord_core::model::Snowflake;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    /// Index of the enclosing spoiler within the message.
    pub spoiler: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String, Style),
    Link { text: String, url: String, style: Style },
    User(Snowflake, Style),
    Role(Snowflake, Style),
    Channel(Snowflake, Style),
    /// `@everyone` / `@here`.
    Everyone(String, Style),
    Emoji { id: Snowflake, name: String, animated: bool, style: Style },
    Timestamp { unix: i64, format: char, style: Style },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading(u8, Vec<Inline>),
    Subtext(Vec<Inline>),
    ListItem { depth: u8, marker: String, content: Vec<Inline> },
    Code { lang: String, code: String },
    Quote(Vec<Block>),
}

pub fn parse(src: &str) -> Vec<Block> {
    let mut spoilers = 0usize;
    parse_blocks(src, &mut spoilers, true)
}

fn parse_blocks(src: &str, spoilers: &mut usize, allow_quote: bool) -> Vec<Block> {
    let mut out = vec![];
    let mut rest = src;
    let mut quote_lines: Vec<&str> = vec![];

    let flush_quote = |out: &mut Vec<Block>, lines: &mut Vec<&str>, spoilers: &mut usize| {
        if !lines.is_empty() {
            let inner = lines.join("\n");
            out.push(Block::Quote(parse_blocks(&inner, spoilers, false)));
            lines.clear();
        }
    };

    while !rest.is_empty() {
        // Fenced code block.
        if let Some(after) = rest.strip_prefix("```")
            && let Some(end) = after.find("```")
        {
            flush_quote(&mut out, &mut quote_lines, spoilers);
            let body = &after[..end];
            let (lang, code) = match body.split_once('\n') {
                Some((first, code)) if !first.contains(' ') && first.len() <= 20 => (first.trim().to_string(), code),
                _ => (String::new(), body),
            };
            out.push(Block::Code { lang, code: code.trim_end_matches('\n').to_string() });
            rest = &after[end + 3..];
            rest = rest.strip_prefix('\n').unwrap_or(rest);
            continue;
        }

        let (line, next) = match rest.find('\n') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        };

        // A code fence may start mid-paragraph line: split there.
        if let Some(i) = line.find("```")
            && i > 0
            && rest[i + 3..].contains("```")
        {
            flush_quote(&mut out, &mut quote_lines, spoilers);
            out.push(Block::Paragraph(parse_inline(&line[..i], Style::default(), spoilers)));
            rest = &rest[i..];
            continue;
        }

        if allow_quote {
            if let Some(q) = line.strip_prefix(">>> ") {
                flush_quote(&mut out, &mut quote_lines, spoilers);
                let inner = if next.is_empty() { q.to_string() } else { format!("{q}\n{next}") };
                out.push(Block::Quote(parse_blocks(&inner, spoilers, false)));
                break;
            }
            if let Some(q) = line.strip_prefix("> ").or_else(|| (line == ">").then_some("")) {
                quote_lines.push(q);
                rest = next;
                continue;
            }
        }
        flush_quote(&mut out, &mut quote_lines, spoilers);

        let block = if let Some(h) = line.strip_prefix("### ") {
            Block::Heading(3, parse_inline(h, Style::default(), spoilers))
        } else if let Some(h) = line.strip_prefix("## ") {
            Block::Heading(2, parse_inline(h, Style::default(), spoilers))
        } else if let Some(h) = line.strip_prefix("# ") {
            Block::Heading(1, parse_inline(h, Style::default(), spoilers))
        } else if let Some(s) = line.strip_prefix("-# ") {
            Block::Subtext(parse_inline(s, Style::default(), spoilers))
        } else if let Some((depth, marker, content)) = list_item(line) {
            Block::ListItem { depth, marker, content: parse_inline(content, Style::default(), spoilers) }
        } else {
            Block::Paragraph(parse_inline(line, Style::default(), spoilers))
        };
        out.push(block);
        rest = next;
    }
    flush_quote(&mut out, &mut quote_lines, spoilers);
    out
}

fn list_item(line: &str) -> Option<(u8, String, &str)> {
    let trimmed = line.trim_start_matches(' ');
    let depth = ((line.len() - trimmed.len()) / 2).min(3) as u8;
    if let Some(c) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        return Some((depth, "•".into(), c));
    }
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && digits.len() <= 9 {
        let after = &trimmed[digits.len()..];
        if let Some(c) = after.strip_prefix(". ") {
            return Some((depth, format!("{digits}."), c));
        }
    }
    None
}

/// Parses inline markup.
pub fn parse_inline(src: &str, style: Style, spoilers: &mut usize) -> Vec<Inline> {
    let mut out: Vec<Inline> = vec![];
    let mut text = String::new();
    let b = src.as_bytes();
    let mut i = 0;

    macro_rules! flush {
        () => {
            if !text.is_empty() {
                push_text(&mut out, std::mem::take(&mut text), style);
            }
        };
    }

    while i < b.len() {
        let rest = &src[i..];

        // Escapes.
        if b[i] == b'\\' && i + 1 < b.len() && !b[i + 1].is_ascii_alphanumeric() && b[i + 1].is_ascii() {
            text.push(b[i + 1] as char);
            i += 2;
            continue;
        }

        // Inline code: `..` or ``..``.
        if b[i] == b'`' {
            let ticks = if rest.starts_with("``") { 2 } else { 1 };
            let delim = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(delim) {
                let code = &rest[ticks..ticks + end];
                if !code.is_empty() {
                    flush!();
                    let s = Style { code: true, ..style };
                    out.push(Inline::Text(code.trim_matches(' ').to_string(), s));
                    i += ticks * 2 + end;
                    continue;
                }
            }
        }

        // Angle-bracket tokens: mentions, emoji, timestamps, suppressed links.
        if b[i] == b'<'
            && let Some(end) = rest.find('>')
            && let Some(tok) = angle_token(&rest[1..end], style)
        {
            flush!();
            out.push(tok);
            i += end + 1;
            continue;
        }

        // Masked link [text](url).
        if b[i] == b'['
            && let Some((label, url, len)) = masked_link(rest)
        {
            flush!();
            out.push(Inline::Link { text: label, url, style });
            i += len;
            continue;
        }

        // Autolinks.
        if (rest.starts_with("https://") || rest.starts_with("http://")) && !style.code {
            let end = rest.find(|c: char| c.is_whitespace() || c == '<').unwrap_or(rest.len());
            let mut url = &rest[..end];
            // Trailing punctuation isn't part of the URL (balanced parens are).
            while url.ends_with(['.', ',', ':', ';', '!', '?', '"', '\'']) || (url.ends_with(')') && url.matches('(').count() < url.matches(')').count()) {
                url = &url[..url.len() - 1];
            }
            flush!();
            out.push(Inline::Link { text: url.to_string(), url: url.to_string(), style });
            i += url.len();
            continue;
        }

        // @everyone / @here.
        if b[i] == b'@'
            && let Some(word) = ["@everyone", "@here"].into_iter().find(|w| rest.starts_with(w))
        {
            flush!();
            out.push(Inline::Everyone(word.into(), style));
            i += word.len();
            continue;
        }

        // Paired delimiters.
        let delims: [(&str, fn(Style) -> Style); 6] = [
            ("||", |s| s),
            ("***", |s| Style { bold: true, italic: true, ..s }),
            ("**", |s| Style { bold: true, ..s }),
            ("__", |s| Style { underline: true, ..s }),
            ("~~", |s| Style { strike: true, ..s }),
            ("*", |s| Style { italic: true, ..s }),
        ];
        let mut matched = false;
        for (d, f) in delims {
            if rest.starts_with(d)
                && let Some(end) = find_closing(&rest[d.len()..], d)
                && end > 0
            {
                flush!();
                let inner = &rest[d.len()..d.len() + end];
                let new_style = if d == "||" {
                    let idx = *spoilers;
                    *spoilers += 1;
                    Style { spoiler: Some(idx), ..style }
                } else {
                    f(style)
                };
                out.extend(parse_inline(inner, new_style, spoilers));
                i += d.len() * 2 + end;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        // Underscore italics need word boundaries (snake_case stays literal).
        if b[i] == b'_'
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && let Some(end) = find_closing(&rest[1..], "_")
            && end > 0
            && rest.as_bytes().get(end + 2).is_none_or(|c| !c.is_ascii_alphanumeric())
        {
            flush!();
            out.extend(parse_inline(&rest[1..1 + end], Style { italic: true, ..style }, spoilers));
            i += end + 2;
            continue;
        }

        let ch = rest.chars().next().unwrap_or(' ');
        text.push(ch);
        i += ch.len_utf8();
    }
    flush!();
    out
}

fn push_text(out: &mut Vec<Inline>, t: String, style: Style) {
    if let Some(Inline::Text(prev, s)) = out.last_mut()
        && *s == style
    {
        prev.push_str(&t);
        return;
    }
    out.push(Inline::Text(t, style));
}

/// Finds the closing delimiter, skipping escaped characters and code spans.
fn find_closing(s: &str, delim: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == b'`'
            && let Some(e) = s[i + 1..].find('`')
        {
            i += e + 2;
            continue;
        }
        if s[i..].starts_with(delim) {
            // `**` must not be read as the end of `*` italics.
            if delim == "*" && s[i..].starts_with("**") {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += s[i..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    None
}

fn masked_link(s: &str) -> Option<(String, String, usize)> {
    let close = s.find("](")?;
    let label = &s[1..close];
    if label.is_empty() || label.contains('\n') {
        return None;
    }
    let after = &s[close + 2..];
    let end = after.find(')')?;
    let mut url = after[..end].trim();
    // Optional title: [x](url "title")
    if let Some(sp) = url.find(' ') {
        url = &url[..sp];
    }
    let url = url.trim_start_matches('<').trim_end_matches('>');
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    Some((label.to_string(), url.to_string(), close + 2 + end + 1))
}

fn angle_token(inner: &str, style: Style) -> Option<Inline> {
    let id = |s: &str| s.parse::<u64>().ok().map(Snowflake);
    if let Some(r) = inner.strip_prefix("@&") {
        return id(r).map(|i| Inline::Role(i, style));
    }
    if let Some(r) = inner.strip_prefix("@!").or_else(|| inner.strip_prefix('@')) {
        return id(r).map(|i| Inline::User(i, style));
    }
    if let Some(r) = inner.strip_prefix('#') {
        return id(r).map(|i| Inline::Channel(i, style));
    }
    if let Some(r) = inner.strip_prefix("t:") {
        let (ts, fmt) = r.split_once(':').unwrap_or((r, "f"));
        let unix = ts.parse().ok()?;
        return Some(Inline::Timestamp { unix, format: fmt.chars().next().unwrap_or('f'), style });
    }
    let (animated, r) = match inner.strip_prefix("a:") {
        Some(r) => (true, r),
        None => (false, inner.strip_prefix(':')?),
    };
    let (name, eid) = r.rsplit_once(':')?;
    Some(Inline::Emoji { id: id(eid)?, name: name.to_string(), animated, style })
}

/// Angle-bracketed URLs (`<https://…>`) are links with embeds suppressed.
pub fn strip_suppressed_links(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains("<http") {
        return s.into();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<http") {
        out.push_str(&rest[..i]);
        match rest[i..].find('>') {
            Some(e) if !rest[i..i + e].contains(char::is_whitespace) => {
                out.push_str(&rest[i + 1..i + e]);
                rest = &rest[i + e + 1..];
            }
            _ => {
                out.push('<');
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inl(s: &str) -> Vec<Inline> {
        parse_inline(s, Style::default(), &mut 0)
    }

    #[test]
    fn emphasis() {
        let v = inl("a **b** *c* __d__ ~~e~~");
        assert!(v.contains(&Inline::Text("b".into(), Style { bold: true, ..Default::default() })));
        assert!(v.contains(&Inline::Text("c".into(), Style { italic: true, ..Default::default() })));
        assert!(v.contains(&Inline::Text("d".into(), Style { underline: true, ..Default::default() })));
        assert!(v.contains(&Inline::Text("e".into(), Style { strike: true, ..Default::default() })));
    }

    #[test]
    fn snake_case_literal() {
        assert_eq!(inl("my_var_name"), vec![Inline::Text("my_var_name".into(), Style::default())]);
    }

    #[test]
    fn mentions_and_emoji() {
        let v = inl("hi <@123> <@&4> <#5> <:pog:6> <a:dance:7> <t:1700000000:R>");
        assert!(v.contains(&Inline::User(Snowflake(123), Style::default())));
        assert!(v.contains(&Inline::Role(Snowflake(4), Style::default())));
        assert!(v.contains(&Inline::Channel(Snowflake(5), Style::default())));
        assert!(v.iter().any(|i| matches!(i, Inline::Emoji { animated: true, .. })));
        assert!(v.iter().any(|i| matches!(i, Inline::Timestamp { format: 'R', .. })));
    }

    #[test]
    fn links() {
        let v = inl("see https://example.com/a_(b). and [docs](https://x.y/z)");
        assert!(v.contains(&Inline::Link { text: "https://example.com/a_(b)".into(), url: "https://example.com/a_(b)".into(), style: Style::default() }));
        assert!(v.contains(&Inline::Link { text: "docs".into(), url: "https://x.y/z".into(), style: Style::default() }));
    }

    #[test]
    fn spoilers_and_code() {
        let v = inl("||secret|| `**not bold**`");
        assert!(matches!(&v[0], Inline::Text(t, s) if t == "secret" && s.spoiler == Some(0)));
        assert!(v.iter().any(|i| matches!(i, Inline::Text(t, s) if t == "**not bold**" && s.code)));
    }

    #[test]
    fn blocks() {
        let b = parse("# Title\n> quoted\n> more\n```rs\nfn main() {}\n```\n- item\n-# small");
        assert!(matches!(b[0], Block::Heading(1, _)));
        assert!(matches!(&b[1], Block::Quote(q) if q.len() == 2));
        assert!(matches!(&b[2], Block::Code { lang, .. } if lang == "rs"));
        assert!(matches!(b[3], Block::ListItem { .. }));
        assert!(matches!(b[4], Block::Subtext(_)));
    }

    #[test]
    fn escape() {
        assert_eq!(inl(r"\*not italic\*"), vec![Inline::Text("*not italic*".into(), Style::default())]);
    }
}
