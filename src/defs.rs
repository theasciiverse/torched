//! Xscorch's `.def` data format, shared by weapons, accessories and
//! scorings:
//!
//! ```text
//! ; a comment
//! Ident = some_class {
//!    key = "text";
//!    other = 42;
//! };
//! ```

use std::collections::HashMap;

/// One `Ident = class { ... };` block.
#[derive(Debug)]
pub struct Block {
    pub ident: String,
    pub line: usize,
    pub fields: HashMap<String, String>,
}

impl Block {
    pub fn text(&self, k: &str) -> Option<&str> {
        self.fields.get(k).map(String::as_str)
    }

    pub fn num(&self, k: &str) -> Result<f64, String> {
        self.fields
            .get(k)
            .map_or(Ok(0.0), |v| v.parse::<f64>().map_err(|_| format!("{}: {k} is not a number", self.ident)))
    }

    pub fn flag(&self, k: &str) -> bool {
        self.fields.get(k).is_some_and(|v| v == "true")
    }

    /// A comma-separated list, lower-cased.
    pub fn list(&self, k: &str) -> Vec<String> {
        self.fields
            .get(k)
            .map(|v| v.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    }

    /// A name, and whether a leading "!" hides it.
    pub fn name(&self, k: &str) -> Result<(String, bool), String> {
        let raw = self.text(k).ok_or_else(|| format!("{}: no {k}", self.ident))?;
        Ok(match raw.strip_prefix('!') {
            Some(n) => (n.to_string(), true),
            None => (raw.to_string(), false),
        })
    }
}

/// Every block of the given class, in order.
pub fn parse(src: &str, class: &str) -> Result<Vec<Block>, String> {
    let mut blocks = Vec::new();
    let mut cur: Option<Block> = None;
    for (n, raw) in src.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let at = || format!("line {}", n + 1);
        if let Some(head) = line.strip_suffix('{') {
            let head = head.trim();
            let Some(ident) = head.strip_suffix(class).map(|h| h.trim().trim_end_matches('=').trim()) else {
                return Err(format!("{}: expected '... = {class} {{'", at()));
            };
            if cur.is_some() {
                return Err(format!("{}: {ident} starts inside another block", at()));
            }
            cur = Some(Block { ident: ident.to_string(), line: n + 1, fields: HashMap::new() });
        } else if line.starts_with('}') {
            blocks.push(cur.take().ok_or_else(|| format!("{}: '}}' with no block open", at()))?);
        } else if let Some((k, v)) = line.split_once('=') {
            let b = cur.as_mut().ok_or_else(|| format!("{}: field outside a block", at()))?;
            let v = v.trim().trim_end_matches(';').trim().trim_matches('"');
            b.fields.insert(k.trim().to_string(), v.to_string());
        } else {
            return Err(format!("{}: can't read '{line}'", at()));
        }
    }
    if cur.is_some() {
        return Err("file ends inside a block".into());
    }
    Ok(blocks)
}

fn strip_comment(line: &str) -> &str {
    // ';' starts a comment unless inside quotes.
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_blocks_and_comments() {
        let src = "; top\nA = thing_class {\n  name = \"Semi; colon\"; ; trailing\n  n = 3;\n};\nB = thing_class {\n};\n";
        let b = parse(src, "thing_class").unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].text("name"), Some("Semi; colon"));
        assert_eq!(b[0].num("n").unwrap(), 3.0);
        assert!(parse("A = other_class {\n};\n", "thing_class").is_err());
    }
}
