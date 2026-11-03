//! Small deterministic helpers. No randomness, no clocks: the same repo must lift to the same bytes.

/// FNV-1a, 64 bit. Stable across machines and Rust versions, which is all a fingerprint needs.
pub fn fnv64(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "root".into()
    } else {
        out
    }
}

pub fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub fn dirname(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[..i],
        None => "",
    }
}

/// Compress sorted 0-based line indices into a human 1-based range list: "3-9, 12".
pub fn ranges(lines: &[usize]) -> String {
    let mut v: Vec<usize> = lines.to_vec();
    v.sort_unstable();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        out.push(if start == end { format!("{}", start + 1) } else { format!("{}-{}", start + 1, end + 1) });
        i += 1;
    }
    out.join(", ")
}

/// YAML double-quoted scalar. Everything we emit is quoted, so no value can turn into a comment or a type.
pub fn q(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\x{:02x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn qlist(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| q(s)).collect::<Vec<_>>().join(", "))
}

/// tsconfig.json is JSONC: strip comments and trailing commas so serde_json can read it.
pub fn strip_jsonc(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut in_str, mut esc) = (0, false, false);
    while i < c.len() {
        let ch = c[i];
        if in_str {
            out.push(ch);
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            in_str = true;
            out.push(ch);
            i += 1;
        } else if ch == '/' && i + 1 < c.len() && c[i + 1] == '/' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && i + 1 < c.len() && c[i + 1] == '*' {
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if ch == ',' {
            let mut j = i + 1;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if j < c.len() && (c[j] == '}' || c[j] == ']') {
                i += 1;
            } else {
                out.push(ch);
                i += 1;
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

pub fn short(s: &str, n: usize) -> String {
    let first = s.lines().next().unwrap_or("").trim();
    if first.chars().count() > n {
        format!("{}…", first.chars().take(n).collect::<String>())
    } else if s.lines().count() > 1 {
        format!("{first} …")
    } else {
        first.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers() {
        assert_eq!(slug("POST /api/users/[id]"), "post-api-users-id");
        assert_eq!(normalize("app/api/../../lib/./x"), "lib/x");
        assert_eq!(ranges(&[0, 1, 2, 5, 7, 8]), "1-3, 6, 8-9");
        assert_eq!(q("a\"b"), "\"a\\\"b\"");
        let j = strip_jsonc("{ // c\n \"a\": \"//x\", /* y */ \"b\": [1,],\n}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&j).unwrap()["a"], "//x");
    }
}

/// "requestBooking" -> "Request booking", "BillingPage" -> "Billing page". Acronyms stay.
pub fn human(n: &str) -> String {
    let mut words: Vec<String> = vec![];
    let mut cur = String::new();
    let chars: Vec<char> = n.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' || c == ' ' || c == '.' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = if i > 0 { chars[i - 1] } else { ' ' };
        let next = chars.get(i + 1).copied().unwrap_or(' ');
        let boundary = c.is_uppercase() && !cur.is_empty() && (prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next.is_lowercase()));
        if boundary {
            words.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let s = words
        .iter()
        .map(|w| if w.len() > 1 && w.chars().all(|c| c.is_uppercase() || c.is_ascii_digit()) { w.clone() } else { w.to_lowercase() })
        .collect::<Vec<_>>()
        .join(" ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

#[cfg(test)]
mod human_tests {
    #[test]
    fn human_names() {
        assert_eq!(super::human("requestBooking"), "Request booking");
        assert_eq!(super::human("BillingPage"), "Billing page");
        assert_eq!(super::human("POST"), "POST");
    }
}
