// Vendored from horizon-yaml (MIT), do not edit here.
//! The YAML that Bukkit plugins read and write, with no dependencies.
//!
//! Bukkit's `YamlConfiguration` (SnakeYAML) writes block style: two-space indent, `- ` list items at the
//! parent key's indent, single-quoted strings where a plain one would be read as something else. Plugin
//! authors write the same by hand, with comments, block scalars (`|-`) and the odd flow list. This crate
//! reads all of that into an ordered [`Node`] tree that remembers each scalar's quoting style and the
//! comments above each key, and writes it back in the same layout, so a file the plugin itself saved comes
//! out byte for byte as it went in.
//!
//! Scalars keep their text; [`Node::tag`] resolves a plain scalar the way SnakeYAML's YAML 1.1 resolver does
//! (`yes`/`on` are booleans, `0x1F` and `0777` are integers, `2026-11-11` is a timestamp, `~` is null), so a
//! port reads exactly the type the Java plugin got. Not supported (no Bukkit plugin file here uses them):
//! anchors and aliases, tags (`!!str`), complex keys (`? `), several documents, tabs for indentation.
//!
//! License: MIT (see `LICENSE`). Vendored as one file (`src/lib.rs` → `yaml.rs`) into ports under any licence.

use std::fmt;

/// How a scalar was written. Plain scalars are type-resolved ([`Node::tag`]); all other styles are strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    Single,
    Double,
    Literal,
    Folded,
}

/// One node of the tree. Maps keep their order. A missing value (`key:` with nothing after it) is an empty
/// plain scalar, which resolves to null.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Scalar { value: String, style: Style },
    Seq { items: Vec<Node>, flow: Flow },
    Map { entries: Vec<Entry>, flow: Flow },
}

/// How a list or map is written: block style, inline (`[a, b]`, `{}`), or an empty flow collection that
/// SnakeYAML split over two lines (`{` and an indented `}` on the next line, as Citizens' `saves.yml` has).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Block,
    Inline,
    Split,
}

/// A key and its value, with the comment and blank lines written above it (`""` = a blank line) and a
/// comment at the end of its line.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub key_style: Style,
    pub value: Node,
    pub comments: Vec<String>,
    pub inline_comment: Option<String>,
}

/// A whole file: the tree plus comment and blank lines after the last node.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// Comment and blank lines above a root that is not a block map (a map keeps them on its first key).
    pub leading: Vec<String>,
    pub root: Node,
    pub trailing: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// 1-based line number.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------------------------------
// Tree access

impl Node {
    pub fn null() -> Node {
        Node::Scalar {
            value: String::new(),
            style: Style::Plain,
        }
    }

    /// A string scalar, plain if SnakeYAML would read the plain text back as the same string, otherwise
    /// single-quoted, or double-quoted when it holds a line break or a control character.
    pub fn str(s: &str) -> Node {
        let style = if plain_is_safe(s) {
            Style::Plain
        } else if s
            .chars()
            .any(|c| c == '\n' || (c.is_control() && c != '\t'))
        {
            Style::Double
        } else {
            Style::Single
        };
        Node::Scalar {
            value: s.to_string(),
            style,
        }
    }

    pub fn int(i: i64) -> Node {
        Node::Scalar {
            value: i.to_string(),
            style: Style::Plain,
        }
    }

    /// A float in Java's `Double.toString` form for ordinary values (`1.0`, `0.25`, `-3.5`).
    pub fn float(f: f64) -> Node {
        Node::Scalar {
            value: java_double(f),
            style: Style::Plain,
        }
    }

    pub fn bool(b: bool) -> Node {
        Node::Scalar {
            value: if b { "true" } else { "false" }.into(),
            style: Style::Plain,
        }
    }

    pub fn map() -> Node {
        Node::Map {
            entries: Vec::new(),
            flow: Flow::Block,
        }
    }

    pub fn seq(items: Vec<Node>) -> Node {
        Node::Seq {
            items,
            flow: Flow::Block,
        }
    }

    /// SnakeYAML's tag for this node: `str`, `int`, `float`, `bool`, `null`, `timestamp`, `seq` or `map`.
    pub fn tag(&self) -> &'static str {
        match self {
            Node::Scalar {
                value,
                style: Style::Plain,
            } => resolve_plain(value),
            Node::Scalar { .. } => "str",
            Node::Seq { .. } => "seq",
            Node::Map { .. } => "map",
        }
    }

    pub fn is_null(&self) -> bool {
        self.tag() == "null"
    }

    /// The scalar's text (also for numbers and booleans), `None` for null, lists and maps.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Scalar { value, .. } if !self.is_null() => Some(value),
            _ => None,
        }
    }

    /// An integer scalar, as SnakeYAML constructs it (decimal, `0x`, `0b`, leading-zero octal, `_`
    /// separators, sexagesimal `1:30`). A float or a string gives `None`, like Bukkit's `isInt`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Node::Scalar { value, .. } if self.tag() == "int" => parse_int(value),
            _ => None,
        }
    }

    /// A float or integer scalar as `f64` (Bukkit's `getDouble` accepts both).
    pub fn as_f64(&self) -> Option<f64> {
        match self.tag() {
            "int" => self.as_i64().map(|i| i as f64),
            "float" => match self {
                Node::Scalar { value, .. } => parse_float(value),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Node::Scalar { value, .. } if self.tag() == "bool" => Some(matches!(
                value.to_ascii_lowercase().as_str(),
                "yes" | "true" | "on"
            )),
            _ => None,
        }
    }

    pub fn items(&self) -> &[Node] {
        match self {
            Node::Seq { items, .. } => items,
            _ => &[],
        }
    }

    pub fn entries(&self) -> &[Entry] {
        match self {
            Node::Map { entries, .. } => entries,
            _ => &[],
        }
    }

    pub fn is_map(&self) -> bool {
        matches!(self, Node::Map { .. })
    }

    pub fn is_seq(&self) -> bool {
        matches!(self, Node::Seq { .. })
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries().iter().map(|e| e.key.as_str())
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entries()
            .iter()
            .find(|e| e.key == key)
            .map(|e| &e.value)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Node> {
        match self {
            Node::Map { entries, .. } => entries
                .iter_mut()
                .find(|e| e.key == key)
                .map(|e| &mut e.value),
            _ => None,
        }
    }

    /// `a.b.c` lookup, as Bukkit's `ConfigurationSection.get("a.b.c")`.
    pub fn path(&self, path: &str) -> Option<&Node> {
        path.split('.').try_fold(self, |node, key| node.get(key))
    }

    /// A string list, as Bukkit's `getStringList` (scalars only, others skipped).
    pub fn str_list(&self) -> Vec<String> {
        self.items()
            .iter()
            .filter_map(|n| n.as_str().map(str::to_string))
            .collect()
    }

    /// Set `key` (keeps the entry's place and comments when it exists, appends otherwise). Turns a
    /// non-map node into an empty map first.
    pub fn set(&mut self, key: &str, value: Node) {
        if !self.is_map() {
            *self = Node::map();
        }
        if let Node::Map { entries, .. } = self {
            if let Some(e) = entries.iter_mut().find(|e| e.key == key) {
                e.value = value;
            } else {
                entries.push(Entry {
                    key: key.to_string(),
                    key_style: key_style(key),
                    value,
                    comments: Vec::new(),
                    inline_comment: None,
                });
            }
        }
    }

    /// Set `a.b.c`, creating the maps on the way.
    pub fn set_path(&mut self, path: &str, value: Node) {
        let mut parts = path.split('.').peekable();
        let mut node = self;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                node.set(part, value);
                return;
            }
            if node.get(part).is_none_or(|n| !n.is_map()) {
                node.set(part, Node::map());
            }
            node = node.get_mut(part).expect("just set");
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Node> {
        match self {
            Node::Map { entries, .. } => {
                let i = entries.iter().position(|e| e.key == key)?;
                Some(entries.remove(i).value)
            }
            _ => None,
        }
    }

    /// The node in the oracle's JSON form (`{"t":tag,"v":len:hash}`, keys `{"t":tag,"k":key}`, `{"s":[...]}`, `{"m":[[k,v],...]}`), used
    /// by the tests that compare this parser with SnakeYAML.
    pub fn oracle_json(&self) -> String {
        let mut out = String::new();
        oracle(self, &mut out);
        out
    }
}

fn oracle(n: &Node, out: &mut String) {
    match n {
        Node::Scalar { value, .. } => {
            let tag = n.tag();
            // SnakeYAML gives an empty plain scalar the text "" and a `~`/`null` one its own text.
            out.push_str("{\"t\":");
            json_str(tag, out);
            out.push_str(",\"v\":");
            json_str(&redact(value), out);
            out.push('}');
        }
        Node::Seq { items, .. } => {
            out.push_str("{\"s\":[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                oracle(item, out);
            }
            out.push_str("]}");
        }
        Node::Map { entries, .. } => {
            out.push_str("{\"m\":[");
            for (i, e) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str("[{\"t\":");
                json_str(
                    Node::Scalar {
                        value: e.key.clone(),
                        style: e.key_style,
                    }
                    .tag(),
                    out,
                );
                out.push_str(",\"k\":");
                json_str(&e.key, out);
                out.push('}');
                out.push(',');
                oracle(&e.value, out);
                out.push(']');
            }
            out.push_str("]}");
        }
    }
}

/// Length in UTF-16 units and Java's `String.hashCode`, so the oracle comparison never prints a value.
fn redact(v: &str) -> String {
    let mut h: i32 = 0;
    let mut n = 0;
    for u in v.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u as i32);
        n += 1;
    }
    format!("{n}:{:x}", h as u32)
}

fn json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------------------------------
// SnakeYAML's implicit resolver (YAML 1.1 types)

fn resolve_plain(v: &str) -> &'static str {
    if v.is_empty() || matches!(v, "~" | "null" | "Null" | "NULL") {
        return "null";
    }
    if matches!(
        v,
        "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    ) {
        return "bool";
    }
    if is_int(v) {
        return "int";
    }
    if is_float(v) {
        return "float";
    }
    if is_timestamp(v) {
        return "timestamp";
    }
    "str"
}

fn sign(v: &str) -> &str {
    v.strip_prefix(['-', '+']).unwrap_or(v)
}

fn all(s: &str, f: impl Fn(char) -> bool) -> bool {
    s.chars().all(f)
}

/// `[-+]?0b_*[0-1]+[0-1_]* | [-+]?0_*[0-7]+[0-7_]* | [-+]?(0|[1-9][0-9_]*) | [-+]?0x_*[0-9a-fA-F]+[0-9a-fA-F_]*
///  | [-+]?[1-9][0-9_]*(:[0-5]?[0-9])+`
fn is_int(v: &str) -> bool {
    let s = sign(v);
    if let Some(b) = s.strip_prefix("0b") {
        let t = b.trim_start_matches('_');
        return t.starts_with(['0', '1']) && all(t, |c| matches!(c, '0' | '1' | '_'));
    }
    if let Some(h) = s.strip_prefix("0x") {
        let t = h.trim_start_matches('_');
        return t.chars().next().is_some_and(|c| c.is_ascii_hexdigit())
            && all(t, |c| c.is_ascii_hexdigit() || c == '_');
    }
    if s == "0" {
        return true;
    }
    if let Some(o) = s.strip_prefix('0') {
        let t = o.trim_start_matches('_');
        return t.chars().next().is_some_and(|c| ('0'..='7').contains(&c))
            && all(t, |c| ('0'..='7').contains(&c) || c == '_');
    }
    if !s.starts_with(|c: char| ('1'..='9').contains(&c)) {
        return false;
    }
    let mut parts = s.split(':');
    let head = parts.next().unwrap_or("");
    if !all(head, |c| c.is_ascii_digit() || c == '_') {
        return false;
    }
    parts.all(|p| sexagesimal_part(p))
}

fn sexagesimal_part(p: &str) -> bool {
    let b = p.as_bytes();
    match b.len() {
        1 => b[0].is_ascii_digit(),
        2 => (b'0'..=b'5').contains(&b[0]) && b[1].is_ascii_digit(),
        _ => false,
    }
}

/// `[-+]?(\.[0-9]+|[0-9_]+(\.[0-9_]*)?)([eE][-+]?[0-9]+)? | [-+]?[0-9][0-9_]*(:[0-5]?[0-9])+\.[0-9_]*
///  | [-+]?\.(inf|Inf|INF) | \.(nan|NaN|NAN)`
fn is_float(v: &str) -> bool {
    if matches!(v, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let s = sign(v);
    if matches!(s, ".inf" | ".Inf" | ".INF") {
        return true;
    }
    // sexagesimal float
    if let Some((head, frac)) = s.rsplit_once('.')
        && head.contains(':')
    {
        let mut parts = head.split(':');
        let first = parts.next().unwrap_or("");
        return first.starts_with(|c: char| c.is_ascii_digit())
            && all(first, |c| c.is_ascii_digit() || c == '_')
            && parts.all(sexagesimal_part)
            && all(frac, |c| c.is_ascii_digit() || c == '_');
    }
    let (mantissa, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    if let Some(e) = exp {
        let e = sign(e);
        if e.is_empty() || !all(e, |c| c.is_ascii_digit()) {
            return false;
        }
    }
    if let Some(frac) = mantissa.strip_prefix('.') {
        return !frac.is_empty() && all(frac, |c| c.is_ascii_digit());
    }
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    !int.is_empty()
        && all(int, |c| c.is_ascii_digit() || c == '_')
        && frac.is_none_or(|f| all(f, |c| c.is_ascii_digit() || c == '_'))
}

/// `YYYY-MM-DD` or `YYYY-M-D(T| +)H:MM:SS(.frac)?( *(Z|±H(:MM)?))?`
fn is_timestamp(v: &str) -> bool {
    let b = v.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| r.clone().all(|i| b.get(i).is_some_and(u8::is_ascii_digit));
    if b.len() == 10
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
    {
        return true;
    }
    // long form
    let mut i = 0;
    let take = |i: &mut usize, min: usize, max: usize| {
        let start = *i;
        while *i < b.len() && *i - start < max && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start >= min
    };
    if !take(&mut i, 4, 4) || b.get(i) != Some(&b'-') {
        return false;
    }
    i += 1;
    if !take(&mut i, 1, 2) || b.get(i) != Some(&b'-') {
        return false;
    }
    i += 1;
    if !take(&mut i, 1, 2) {
        return false;
    }
    match b.get(i) {
        Some(b'T' | b't') => i += 1,
        Some(b' ' | b'\t') => {
            while matches!(b.get(i), Some(b' ' | b'\t')) {
                i += 1;
            }
        }
        _ => return false,
    }
    if !take(&mut i, 1, 2) || b.get(i) != Some(&b':') {
        return false;
    }
    i += 1;
    if !take(&mut i, 2, 2) || b.get(i) != Some(&b':') {
        return false;
    }
    i += 1;
    if !take(&mut i, 2, 2) {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    while matches!(b.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    match b.get(i) {
        None => true,
        Some(b'Z') => i + 1 == b.len(),
        Some(b'-' | b'+') => {
            i += 1;
            if !take(&mut i, 1, 2) {
                return false;
            }
            if b.get(i) == Some(&b':') {
                i += 1;
                if !take(&mut i, 2, 2) {
                    return false;
                }
            }
            i == b.len()
        }
        _ => false,
    }
}

fn parse_int(v: &str) -> Option<i64> {
    let neg = v.starts_with('-');
    let s: String = sign(v).chars().filter(|&c| c != '_').collect();
    let n = if let Some(b) = s.strip_prefix("0b") {
        i64::from_str_radix(b, 2).ok()?
    } else if let Some(h) = s.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()?
    } else if s.contains(':') {
        s.split(':')
            .try_fold(0i64, |acc, p| Some(acc * 60 + p.parse::<i64>().ok()?))?
    } else if s.len() > 1 && s.starts_with('0') {
        i64::from_str_radix(&s[1..], 8).ok()?
    } else {
        s.parse().ok()?
    };
    Some(if neg { -n } else { n })
}

fn parse_float(v: &str) -> Option<f64> {
    let s: String = v.chars().filter(|&c| c != '_').collect();
    match s.as_str() {
        ".nan" | ".NaN" | ".NAN" => return Some(f64::NAN),
        _ => {}
    }
    let neg = s.starts_with('-');
    let body = sign(&s);
    if matches!(body, ".inf" | ".Inf" | ".INF") {
        return Some(if neg {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    if body.contains(':') {
        let (head, frac) = body.rsplit_once('.')?;
        let mut acc = 0f64;
        for p in head.split(':') {
            acc = acc * 60.0 + p.parse::<f64>().ok()?;
        }
        let f: f64 = format!("0.{frac}").parse().unwrap_or(0.0);
        let r = acc + f;
        return Some(if neg { -r } else { r });
    }
    s.parse().ok()
}

/// Java's `Double.toString` for the values configs hold (always a `.`; exponent form outside 1e-3..1e7).
fn java_double(f: f64) -> String {
    if f.is_nan() {
        return ".NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            ".inf".into()
        } else {
            "-.inf".into()
        };
    }
    let a = f.abs();
    if a != 0.0 && !(1e-3..1e7).contains(&a) {
        let s = format!("{f:E}");
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let m = if m.contains('.') {
            m.to_string()
        } else {
            format!("{m}.0")
        };
        return format!("{m}E{e}");
    }
    let s = format!("{f}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

/// Whether `s` written plain reads back as the same string.
fn plain_is_safe(s: &str) -> bool {
    if s.is_empty() || resolve_plain(s) != "str" {
        return false;
    }
    let first = s.chars().next().unwrap();
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
        // `-x` and `:x` are allowed by YAML, but SnakeYAML quotes them; so do we.
        return false;
    }
    if s.starts_with(' ') || s.ends_with(' ') || s.ends_with(':') {
        return false;
    }
    if s.contains(": ") || s.contains(" #") || s.contains(['\n', '\r', '\t']) {
        return false;
    }
    !s.chars().any(|c| c.is_control())
}

fn key_style(k: &str) -> Style {
    match Node::str(k) {
        Node::Scalar { style, .. } => style,
        _ => Style::Single,
    }
}

// ---------------------------------------------------------------------------------------------------
// Parser

/// Parse a whole file.
pub fn parse_document(text: &str) -> Result<Document, Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    let mut p = Parser { lines, i: 0 };
    for (n, l) in p.lines.iter().enumerate() {
        let lead: String = l.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        if lead.contains('\t') && !is_comment_or_blank(l) {
            return Err(Error {
                line: n + 1,
                message: "tab used for indentation".into(),
            });
        }
    }
    let start = p.i;
    let comments = p.take_comments();
    let root = match p.peek_content() {
        None => {
            p.i = start;
            Node::null()
        }
        Some(j) => {
            p.i = start;
            let n = indent(&p.lines[j]);
            p.parse_block(n)?
        }
    };
    let trailing = p.take_comments();
    if let Some(j) = p.peek_content() {
        return Err(Error {
            line: j + 1,
            message: "unexpected content (bad indentation?)".into(),
        });
    }
    let leading = match &root {
        Node::Map {
            entries,
            flow: Flow::Block,
        } if !entries.is_empty() => Vec::new(),
        _ => comments,
    };
    Ok(Document {
        leading,
        root,
        trailing: trim_final_blank(trailing),
    })
}

/// Parse a whole file, keep only the tree.
pub fn parse(text: &str) -> Result<Node, Error> {
    parse_document(text).map(|d| d.root)
}

fn trim_final_blank(mut v: Vec<String>) -> Vec<String> {
    // The file's final newline shows up as one empty "line" after it.
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

struct Parser {
    lines: Vec<String>,
    i: usize,
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start_matches(' ').len()
}

fn is_comment_or_blank(l: &str) -> bool {
    let t = l.trim_start_matches(' ');
    t.is_empty() || t.starts_with('#')
}

/// Strip a trailing ` # comment` that is outside quotes; returns (content, comment).
fn split_comment(s: &str) -> (&str, Option<&str>) {
    let b = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if q == b'"' && c == b'\\' {
                    i += 2;
                    continue;
                }
                if c == q {
                    if q == b'\'' && b.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    quote = None;
                }
            }
            None => {
                if (c == b'"' || c == b'\'')
                    && (i == 0 || matches!(b[i - 1], b' ' | b'[' | b'{' | b',' | b':'))
                {
                    quote = Some(c);
                } else if c == b'#' && (i == 0 || b[i - 1] == b' ') {
                    return (s[..i].trim_end_matches(' '), Some(&s[i..]));
                }
            }
        }
        i += 1;
    }
    (s.trim_end_matches(' '), None)
}

/// Where the `:` of a block mapping key is (index into `s`), if `s` starts a `key: value` / `key:` line.
fn map_colon(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() || matches!(b[0], b'[' | b'{' | b'#' | b'|' | b'>') {
        return None;
    }
    if b[0] == b'-' && (b.len() == 1 || b[1] == b' ') {
        return None;
    }
    let colon_ok = |i: usize| b.get(i) == Some(&b':') && (i + 1 == b.len() || b[i + 1] == b' ');
    if b[0] == b'"' || b[0] == b'\'' {
        let q = b[0];
        let mut i = 1;
        while i < b.len() {
            if q == b'"' && b[i] == b'\\' {
                i += 2;
                continue;
            }
            if b[i] == q {
                if q == b'\'' && b.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                while b.get(j) == Some(&b' ') {
                    j += 1;
                }
                return colon_ok(j).then_some(j);
            }
            i += 1;
        }
        return None;
    }
    for i in 0..b.len() {
        if b[i] == b'#' && i > 0 && b[i - 1] == b' ' {
            return None;
        }
        if colon_ok(i) {
            return Some(i);
        }
    }
    None
}

impl Parser {
    fn err<T>(&self, line: usize, msg: &str) -> Result<T, Error> {
        Err(Error {
            line: line + 1,
            message: msg.into(),
        })
    }

    /// Index of the next line with content (not blank, not a comment), without moving.
    fn peek_content(&self) -> Option<usize> {
        (self.i..self.lines.len()).find(|&j| !is_comment_or_blank(&self.lines[j]))
    }

    /// Consume blank and comment lines, returning them trimmed (`""` for blank).
    fn take_comments(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while self.i < self.lines.len() && is_comment_or_blank(&self.lines[self.i]) {
            out.push(self.lines[self.i].trim().to_string());
            self.i += 1;
        }
        out
    }

    /// A block node whose first line is the next content line, at indent `n`.
    fn parse_block(&mut self, n: usize) -> Result<Node, Error> {
        let j = self.peek_content().expect("caller checked");
        let t = &self.lines[j][n..];
        if t == "-" || t.starts_with("- ") {
            self.parse_seq(n)
        } else if map_colon(t).is_some() {
            self.parse_map(n)
        } else {
            // a lone scalar (document root or more-indented value)
            self.take_comments();
            let i = self.i;
            let (node, _) = self.inline_value(i, n, n.saturating_sub(1))?;
            Ok(node)
        }
    }

    fn parse_map(&mut self, n: usize) -> Result<Node, Error> {
        let mut entries = Vec::new();
        loop {
            let save = self.i;
            let comments = self.take_comments();
            let Some(j) = self.peek_content() else {
                self.i = save;
                break;
            };
            let ind = indent(&self.lines[j]);
            if ind < n {
                self.i = save;
                break;
            }
            if ind > n {
                return self.err(j, "bad indentation of a mapping entry");
            }
            let line = self.lines[j].clone();
            let t = &line[n..];
            if t == "-" || t.starts_with("- ") {
                self.i = save;
                break;
            }
            let Some(c) = map_colon(t) else {
                return self.err(j, "expected `key: value`");
            };
            let (key, key_style) = parse_key(t[..c].trim_end(), j)?;
            let rest_col = n + c + 1;
            let (rest, inline) = split_comment(&line[rest_col..]);
            let mut inline_comment = inline.map(str::to_string);
            let value = if rest.trim().is_empty() {
                self.i = j + 1;
                match self.peek_content() {
                    Some(k) if indent(&self.lines[k]) > n => {
                        let ni = indent(&self.lines[k]);
                        self.parse_block(ni)?
                    }
                    Some(k)
                        if indent(&self.lines[k]) == n && {
                            let tt = &self.lines[k][n..];
                            tt == "-" || tt.starts_with("- ")
                        } =>
                    {
                        self.parse_seq(n)?
                    }
                    _ => Node::null(),
                }
            } else {
                let col = rest_col
                    + (line[rest_col..].len() - line[rest_col..].trim_start_matches(' ').len());
                let (v, c2) = self.inline_value(j, col, n)?;
                if c2.is_some() {
                    inline_comment = c2;
                }
                v
            };
            if let Some(dup) = entries.iter().position(|e: &Entry| e.key == key) {
                // SnakeYAML keeps the last value of a duplicate key.
                entries.remove(dup);
            }
            entries.push(Entry {
                key,
                key_style,
                value,
                comments,
                inline_comment,
            });
        }
        Ok(Node::Map {
            entries,
            flow: Flow::Block,
        })
    }

    fn parse_seq(&mut self, n: usize) -> Result<Node, Error> {
        let mut items = Vec::new();
        loop {
            let save = self.i;
            self.take_comments();
            let Some(j) = self.peek_content() else {
                self.i = save;
                break;
            };
            let ind = indent(&self.lines[j]);
            let line = self.lines[j].clone();
            let t = &line[ind..];
            if ind != n || !(t == "-" || t.starts_with("- ")) {
                if ind > n {
                    return self.err(j, "bad indentation of a list item");
                }
                self.i = save;
                break;
            }
            let after = &line[n + 1..];
            let (rest, _) = split_comment(after);
            if rest.trim().is_empty() {
                self.i = j + 1;
                let v = match self.peek_content() {
                    Some(k) if indent(&self.lines[k]) > n => {
                        let ni = indent(&self.lines[k]);
                        self.parse_block(ni)?
                    }
                    _ => Node::null(),
                };
                items.push(v);
                continue;
            }
            let col = n + 1 + (after.len() - after.trim_start_matches(' ').len());
            let content = &line[col..];
            let nested_seq = content == "-" || content.starts_with("- ");
            if nested_seq || map_colon(content).is_some() {
                // Turn "- key: v" into "  key: v" and parse the block at that column.
                self.lines[j] = format!("{}{}", " ".repeat(col), content);
                self.i = j;
                let v = if nested_seq {
                    self.parse_seq(col)?
                } else {
                    self.parse_map(col)?
                };
                items.push(v);
            } else {
                let (v, _) = self.inline_value(j, col, n)?;
                items.push(v);
            }
        }
        Ok(Node::Seq {
            items,
            flow: Flow::Block,
        })
    }

    /// A value starting at `lines[i][col..]`; `parent` is the indent of the key or `-` that owns it.
    /// Leaves `self.i` on the line after the value. Returns the node and a comment at the end of its line.
    fn inline_value(
        &mut self,
        i: usize,
        col: usize,
        parent: usize,
    ) -> Result<(Node, Option<String>), Error> {
        let line = self.lines[i].clone();
        let s = &line[col..];
        let first = s.as_bytes()[0];
        match first {
            b'|' | b'>' => self.block_scalar(i, col, parent),
            b'"' | b'\'' => self.quoted(i, col),
            b'[' | b'{' => self.flow(i, col),
            b'&' | b'*' | b'!' => self.err(i, "anchors, aliases and tags are not supported"),
            _ => {
                let (text, comment) = split_comment(s);
                let mut value = text.trim().to_string();
                self.i = i + 1;
                // continuation lines of a multi-line plain scalar
                let mut blanks = 0;
                let mut k = self.i;
                while k < self.lines.len() {
                    let l = &self.lines[k];
                    if l.trim().is_empty() {
                        blanks += 1;
                        k += 1;
                        continue;
                    }
                    if indent(l) <= parent || l.trim_start().starts_with('#') {
                        break;
                    }
                    let (more, _) = split_comment(l.trim());
                    if blanks > 0 {
                        value.push_str(&"\n".repeat(blanks));
                    } else {
                        value.push(' ');
                    }
                    value.push_str(more.trim());
                    blanks = 0;
                    k += 1;
                    self.i = k;
                }
                Ok((
                    Node::Scalar {
                        value,
                        style: Style::Plain,
                    },
                    comment.map(str::to_string),
                ))
            }
        }
    }

    fn block_scalar(
        &mut self,
        i: usize,
        col: usize,
        parent: usize,
    ) -> Result<(Node, Option<String>), Error> {
        let line = self.lines[i].clone();
        let (head, comment) = split_comment(&line[col..]);
        let literal = head.starts_with('|');
        let mut chomp = 'c';
        let mut explicit: Option<usize> = None;
        for c in head[1..].chars() {
            match c {
                '-' | '+' => chomp = c,
                '1'..='9' => explicit = Some(c as usize - '0' as usize),
                ' ' => {}
                _ => return self.err(i, "bad block scalar header"),
            }
        }
        let mut k = i + 1;
        let content_indent = match explicit {
            Some(d) => parent + d,
            None => {
                let mut m = None;
                let mut kk = k;
                while kk < self.lines.len() {
                    if !self.lines[kk].trim().is_empty() {
                        m = Some(indent(&self.lines[kk]));
                        break;
                    }
                    kk += 1;
                }
                match m {
                    Some(m) if m > parent => m,
                    _ => parent + 1,
                }
            }
        };
        let mut body: Vec<String> = Vec::new();
        while k < self.lines.len() {
            let l = &self.lines[k];
            if l.trim().is_empty() {
                body.push(String::new());
                k += 1;
                continue;
            }
            if indent(l) < content_indent {
                break;
            }
            body.push(l[content_indent..].to_string());
            k += 1;
        }
        // trailing blank lines: count them for chomping, then give them back to the outer level
        let mut trailing_blank = 0;
        while body.last().is_some_and(String::is_empty) {
            body.pop();
            trailing_blank += 1;
        }
        let consumed_to = k - trailing_blank;
        let mut value = if literal {
            body.join("\n")
        } else {
            fold(&body)
        };
        let has_content = !body.is_empty();
        match chomp {
            '-' => {}
            '+' => {
                if has_content {
                    value.push('\n');
                }
                value.push_str(&"\n".repeat(trailing_blank));
            }
            _ => {
                if has_content {
                    value.push('\n');
                }
            }
        }
        self.i = if chomp == '+' { k } else { consumed_to };
        let style = if literal {
            Style::Literal
        } else {
            Style::Folded
        };
        Ok((Node::Scalar { value, style }, comment.map(str::to_string)))
    }

    /// A quoted scalar from `lines[i][col]`, possibly spanning lines.
    fn quoted(&mut self, i: usize, col: usize) -> Result<(Node, Option<String>), Error> {
        let (value, end_line, end_col, double) = self.scan_quoted(i, col)?;
        let line = self.lines[end_line].clone();
        let (rest, comment) = split_comment(&line[end_col..]);
        if !rest.trim().is_empty() {
            return self.err(end_line, "text after a quoted scalar");
        }
        self.i = end_line + 1;
        let style = if double { Style::Double } else { Style::Single };
        Ok((Node::Scalar { value, style }, comment.map(str::to_string)))
    }

    /// Returns (value, line of the closing quote, column after it, double-quoted?).
    fn scan_quoted(&self, i: usize, col: usize) -> Result<(String, usize, usize, bool), Error> {
        let q = self.lines[i].as_bytes()[col];
        let double = q == b'"';
        let mut out = String::new();
        let mut li = i;
        let mut chars: Vec<char> = self.lines[li][col + 1..].chars().collect();
        let mut base = col + 1; // byte offset of chars[0] in the line
        let mut ci = 0;
        loop {
            if ci >= chars.len() {
                // line break inside the scalar: fold
                let mut trimmed = out.trim_end_matches([' ', '\t']).len();
                // A trailing unescaped backslash was handled in the escape branch (sets the marker below).
                if out.ends_with('\u{1}') {
                    out.pop();
                    trimmed = out.len();
                    out.truncate(trimmed);
                    li += 1;
                    if li >= self.lines.len() {
                        return self.err(i, "unterminated quoted scalar");
                    }
                    let l = &self.lines[li];
                    let lead = l.len() - l.trim_start_matches([' ', '\t']).len();
                    chars = l[lead..].chars().collect();
                    base = lead;
                    ci = 0;
                    continue;
                }
                out.truncate(trimmed);
                let mut empties = 0;
                loop {
                    li += 1;
                    if li >= self.lines.len() {
                        return self.err(i, "unterminated quoted scalar");
                    }
                    if self.lines[li].trim().is_empty() {
                        empties += 1;
                    } else {
                        break;
                    }
                }
                if empties > 0 {
                    out.push_str(&"\n".repeat(empties));
                } else {
                    out.push(' ');
                }
                let l = &self.lines[li];
                let lead = l.len() - l.trim_start_matches([' ', '\t']).len();
                chars = l[lead..].chars().collect();
                base = lead;
                ci = 0;
                continue;
            }
            let c = chars[ci];
            if !double {
                if c == '\'' {
                    if chars.get(ci + 1) == Some(&'\'') {
                        out.push('\'');
                        ci += 2;
                        continue;
                    }
                    let end = base + chars[..=ci].iter().map(|c| c.len_utf8()).sum::<usize>();
                    return Ok((out, li, end, false));
                }
                out.push(c);
                ci += 1;
                continue;
            }
            match c {
                '"' => {
                    let end = base + chars[..=ci].iter().map(|c| c.len_utf8()).sum::<usize>();
                    return Ok((out, li, end, true));
                }
                '\\' => {
                    let Some(&e) = chars.get(ci + 1) else {
                        // escaped line break: join without a space
                        out.push('\u{1}');
                        ci += 1;
                        continue;
                    };
                    ci += 2;
                    let hex = |n: usize, ci: &mut usize| -> Option<char> {
                        let s: String = chars.get(*ci..*ci + n)?.iter().collect();
                        *ci += n;
                        char::from_u32(u32::from_str_radix(&s, 16).ok()?)
                    };
                    let ch = match e {
                        '0' => '\0',
                        'a' => '\u{7}',
                        'b' => '\u{8}',
                        't' | '\t' => '\t',
                        'n' => '\n',
                        'v' => '\u{b}',
                        'f' => '\u{c}',
                        'r' => '\r',
                        'e' => '\u{1b}',
                        ' ' => ' ',
                        '"' => '"',
                        '/' => '/',
                        '\\' => '\\',
                        'N' => '\u{85}',
                        '_' => '\u{a0}',
                        'L' => '\u{2028}',
                        'P' => '\u{2029}',
                        'x' => hex(2, &mut ci).ok_or_else(|| Error {
                            line: li + 1,
                            message: "bad \\x escape".into(),
                        })?,
                        'u' => hex(4, &mut ci).ok_or_else(|| Error {
                            line: li + 1,
                            message: "bad \\u escape".into(),
                        })?,
                        'U' => hex(8, &mut ci).ok_or_else(|| Error {
                            line: li + 1,
                            message: "bad \\U escape".into(),
                        })?,
                        _ => return self.err(li, "unknown escape in a double-quoted scalar"),
                    };
                    out.push(ch);
                }
                c => {
                    out.push(c);
                    ci += 1;
                }
            }
        }
    }

    /// A flow collection from `lines[i][col]`, possibly spanning lines.
    fn flow(&mut self, i: usize, col: usize) -> Result<(Node, Option<String>), Error> {
        // Gather text until the brackets balance (outside quotes), lines joined by a space.
        let mut text = String::new();
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut li = i;
        let mut start = col;
        let end_line;
        let end_col;
        'outer: loop {
            let line: Vec<char> = self.lines[li][start..].chars().collect();
            let mut k = 0;
            let mut byte = start;
            while k < line.len() {
                let c = line[k];
                text.push(c);
                byte += c.len_utf8();
                match quote {
                    Some('"') if c == '\\' => {
                        if let Some(&n) = line.get(k + 1) {
                            text.push(n);
                            byte += n.len_utf8();
                            k += 1;
                        }
                    }
                    Some(q) if c == q => {
                        if q == '\'' && line.get(k + 1) == Some(&'\'') {
                            text.push('\'');
                            byte += 1;
                            k += 1;
                        } else {
                            quote = None;
                        }
                    }
                    Some(_) => {}
                    None => match c {
                        '"' | '\'' => quote = Some(c),
                        '[' | '{' => depth += 1,
                        ']' | '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end_line = li;
                                end_col = byte;
                                break 'outer;
                            }
                        }
                        '#' if text.len() >= 2 && text[..text.len() - 1].ends_with(' ') => {
                            // comment inside a multi-line flow collection: drop the rest of the line
                            text.pop();
                            break;
                        }
                        _ => {}
                    },
                }
                k += 1;
            }
            li += 1;
            if li >= self.lines.len() {
                return self.err(i, "unterminated flow collection");
            }
            text.push(' ');
            start = indent(&self.lines[li]);
        }
        let line = self.lines[end_line].clone();
        let (rest, comment) = split_comment(&line[end_col..]);
        if !rest.trim().is_empty() {
            return self.err(end_line, "text after a flow collection");
        }
        self.i = end_line + 1;
        let chars: Vec<char> = text.chars().collect();
        let mut fp = FlowParser {
            c: &chars,
            i: 0,
            line: i,
        };
        let mut node = fp.node()?;
        if end_line > i {
            match &mut node {
                Node::Map { entries, flow } if entries.is_empty() => *flow = Flow::Split,
                Node::Seq { items, flow } if items.is_empty() => *flow = Flow::Split,
                _ => {}
            }
        }
        Ok((node, comment.map(str::to_string)))
    }
}

fn fold(lines: &[String]) -> String {
    let mut out = String::new();
    let mut prev_more = false;
    let mut pending_breaks = 0usize;
    let mut first = true;
    for l in lines {
        if l.is_empty() {
            pending_breaks += 1;
            continue;
        }
        let more = l.starts_with(' ') || l.starts_with('\t');
        if !first {
            if pending_breaks > 0 {
                out.push_str(&"\n".repeat(if more || prev_more {
                    pending_breaks + 1
                } else {
                    pending_breaks
                }));
            } else if more || prev_more {
                out.push('\n');
            } else {
                out.push(' ');
            }
        } else if pending_breaks > 0 {
            out.push_str(&"\n".repeat(pending_breaks));
        }
        pending_breaks = 0;
        out.push_str(l);
        prev_more = more;
        first = false;
    }
    out
}

fn parse_key(k: &str, line: usize) -> Result<(String, Style), Error> {
    if k.starts_with('"') || k.starts_with('\'') {
        let p = Parser {
            lines: vec![k.to_string()],
            i: 0,
        };
        let (v, _, end, double) = p.scan_quoted(0, 0).map_err(|e| Error {
            line: line + 1,
            message: e.message,
        })?;
        if end != k.len() {
            return Err(Error {
                line: line + 1,
                message: "text after a quoted key".into(),
            });
        }
        Ok((v, if double { Style::Double } else { Style::Single }))
    } else {
        Ok((k.to_string(), Style::Plain))
    }
}

struct FlowParser<'a> {
    c: &'a [char],
    i: usize,
    line: usize,
}

impl FlowParser<'_> {
    fn err<T>(&self, m: &str) -> Result<T, Error> {
        Err(Error {
            line: self.line + 1,
            message: m.into(),
        })
    }
    fn ws(&mut self) {
        while self.c.get(self.i).is_some_and(|c| c.is_whitespace()) {
            self.i += 1;
        }
    }
    fn node(&mut self) -> Result<Node, Error> {
        self.ws();
        match self.c.get(self.i) {
            Some('[') => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    match self.c.get(self.i) {
                        Some(']') => {
                            self.i += 1;
                            break;
                        }
                        None => return self.err("unterminated flow sequence"),
                        _ => {}
                    }
                    let item = self.node()?;
                    // `[a: b]` single-pair map
                    self.ws();
                    if self.c.get(self.i) == Some(&':') {
                        self.i += 1;
                        let v = self.node()?;
                        let (key, key_style) = scalar_key(item);
                        items.push(Node::Map {
                            entries: vec![Entry {
                                key,
                                key_style,
                                value: v,
                                comments: vec![],
                                inline_comment: None,
                            }],
                            flow: Flow::Inline,
                        });
                    } else {
                        items.push(item);
                    }
                    self.ws();
                    match self.c.get(self.i) {
                        Some(',') => self.i += 1,
                        Some(']') => {}
                        _ => return self.err("expected `,` or `]`"),
                    }
                }
                Ok(Node::Seq {
                    items,
                    flow: Flow::Inline,
                })
            }
            Some('{') => {
                self.i += 1;
                let mut entries: Vec<Entry> = Vec::new();
                loop {
                    self.ws();
                    match self.c.get(self.i) {
                        Some('}') => {
                            self.i += 1;
                            break;
                        }
                        None => return self.err("unterminated flow mapping"),
                        _ => {}
                    }
                    let k = self.node()?;
                    self.ws();
                    let v = if self.c.get(self.i) == Some(&':') {
                        self.i += 1;
                        self.ws();
                        if matches!(self.c.get(self.i), Some(',') | Some('}')) {
                            Node::null()
                        } else {
                            self.node()?
                        }
                    } else {
                        Node::null()
                    };
                    let (key, key_style) = scalar_key(k);
                    entries.retain(|e| e.key != key);
                    entries.push(Entry {
                        key,
                        key_style,
                        value: v,
                        comments: vec![],
                        inline_comment: None,
                    });
                    self.ws();
                    match self.c.get(self.i) {
                        Some(',') => self.i += 1,
                        Some('}') => {}
                        _ => return self.err("expected `,` or `}`"),
                    }
                }
                Ok(Node::Map {
                    entries,
                    flow: Flow::Inline,
                })
            }
            Some('"') | Some('\'') => {
                let q = self.c[self.i];
                self.i += 1;
                let mut s = String::new();
                loop {
                    let Some(&c) = self.c.get(self.i) else {
                        return self.err("unterminated quoted scalar");
                    };
                    self.i += 1;
                    if c == q {
                        if q == '\'' && self.c.get(self.i) == Some(&'\'') {
                            s.push('\'');
                            self.i += 1;
                            continue;
                        }
                        break;
                    }
                    if q == '"' && c == '\\' {
                        let Some(&e) = self.c.get(self.i) else {
                            return self.err("bad escape");
                        };
                        self.i += 1;
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '0' => '\0',
                            '"' => '"',
                            '\\' => '\\',
                            '/' => '/',
                            ' ' => ' ',
                            'u' | 'x' | 'U' => {
                                let n = match e {
                                    'x' => 2,
                                    'u' => 4,
                                    _ => 8,
                                };
                                let h: String = self
                                    .c
                                    .get(self.i..self.i + n)
                                    .unwrap_or(&[])
                                    .iter()
                                    .collect();
                                self.i += n;
                                u32::from_str_radix(&h, 16)
                                    .ok()
                                    .and_then(char::from_u32)
                                    .ok_or(Error {
                                        line: self.line + 1,
                                        message: "bad escape".into(),
                                    })?
                            }
                            _ => return self.err("unknown escape"),
                        });
                        continue;
                    }
                    s.push(c);
                }
                let style = if q == '"' {
                    Style::Double
                } else {
                    Style::Single
                };
                Ok(Node::Scalar { value: s, style })
            }
            _ => {
                let start = self.i;
                while let Some(&c) = self.c.get(self.i) {
                    if matches!(c, ',' | '[' | ']' | '{' | '}') {
                        break;
                    }
                    if c == ':'
                        && self
                            .c
                            .get(self.i + 1)
                            .is_none_or(|n| n.is_whitespace() || matches!(n, ',' | ']' | '}'))
                    {
                        break;
                    }
                    self.i += 1;
                }
                let v: String = self.c[start..self.i].iter().collect();
                let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
                Ok(Node::Scalar {
                    value: v,
                    style: Style::Plain,
                })
            }
        }
    }
}

fn scalar_key(n: Node) -> (String, Style) {
    match n {
        Node::Scalar { value, style } => (value, style),
        other => (format!("{other:?}"), Style::Plain),
    }
}

// ---------------------------------------------------------------------------------------------------
// Writer

/// Write a document in Bukkit's layout (two-space indent, list items at the parent key's indent).
pub fn write_document(doc: &Document) -> String {
    let mut out = String::new();
    for c in &doc.leading {
        out.push_str(c);
        out.push('\n');
    }
    match &doc.root {
        Node::Map {
            entries,
            flow: Flow::Block,
        } if !entries.is_empty() => write_map(entries, 0, &mut out),
        Node::Seq {
            items,
            flow: Flow::Block,
        } if !items.is_empty() => write_seq(items, 0, &mut out),
        n if n.is_null() && doc.trailing.is_empty() => {}
        n => {
            out.push_str(&inline_repr(n, 0));
            out.push('\n');
        }
    }
    for c in &doc.trailing {
        out.push_str(c);
        out.push('\n');
    }
    out
}

/// Write a tree with no trailing comments.
pub fn write(node: &Node) -> String {
    write_document(&Document {
        leading: Vec::new(),
        root: node.clone(),
        trailing: Vec::new(),
    })
}

fn pad(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

fn write_map(entries: &[Entry], ind: usize, out: &mut String) {
    for e in entries {
        for c in &e.comments {
            if !c.is_empty() {
                pad(out, ind);
                out.push_str(c);
            }
            out.push('\n');
        }
        pad(out, ind);
        out.push_str(&scalar_repr(&e.key, e.key_style, ind, true));
        out.push(':');
        let tail = |out: &mut String| {
            if let Some(c) = &e.inline_comment {
                out.push(' ');
                out.push_str(c);
            }
            out.push('\n');
        };
        match &e.value {
            Node::Map {
                entries: sub,
                flow: Flow::Block,
            } if !sub.is_empty() => {
                tail(out);
                write_map(sub, ind + 2, out);
            }
            Node::Seq {
                items,
                flow: Flow::Block,
            } if !items.is_empty() => {
                tail(out);
                write_seq(items, ind, out);
            }
            Node::Scalar {
                value,
                style: Style::Plain,
            } if value.is_empty() => tail(out),
            Node::Map {
                entries: sub,
                flow: Flow::Split,
            } if sub.is_empty() => {
                out.push_str(" {\n");
                pad(out, ind + 2);
                out.push('}');
                tail(out);
            }
            Node::Seq {
                items,
                flow: Flow::Split,
            } if items.is_empty() => {
                out.push_str(" [\n");
                pad(out, ind + 2);
                out.push(']');
                tail(out);
            }
            Node::Scalar {
                value,
                style: style @ (Style::Literal | Style::Folded),
            } => {
                let _ = style;
                write_block_scalar(value, ind, out, &e.inline_comment);
            }
            v => {
                out.push(' ');
                out.push_str(&inline_repr(v, ind));
                tail(out);
            }
        }
    }
}

fn write_seq(items: &[Node], ind: usize, out: &mut String) {
    for item in items {
        match item {
            Node::Map {
                entries,
                flow: Flow::Block,
            } if !entries.is_empty() => {
                let mut sub = String::new();
                write_map(entries, ind + 2, &mut sub);
                out.push_str(&dash_first_line(&sub, ind));
            }
            Node::Seq {
                items: sub_items,
                flow: Flow::Block,
            } if !sub_items.is_empty() => {
                let mut sub = String::new();
                write_seq(sub_items, ind + 2, &mut sub);
                out.push_str(&dash_first_line(&sub, ind));
            }
            Node::Scalar {
                value,
                style: Style::Plain,
            } if value.is_empty() => {
                pad(out, ind);
                out.push_str("-\n");
            }
            Node::Scalar {
                value,
                style: Style::Literal | Style::Folded,
            } => {
                pad(out, ind);
                out.push('-');
                write_block_scalar(value, ind, out, &None);
            }
            v => {
                pad(out, ind);
                out.push_str("- ");
                out.push_str(&inline_repr(v, ind + 2));
                out.push('\n');
            }
        }
    }
}

/// Replace the first line's leading `ind + 2` spaces with `ind` spaces and `- `.
fn dash_first_line(sub: &str, ind: usize) -> String {
    let mut s = String::with_capacity(sub.len() + 2);
    pad(&mut s, ind);
    s.push_str("- ");
    s.push_str(&sub[ind + 2..]);
    s
}

fn write_block_scalar(value: &str, ind: usize, out: &mut String, comment: &Option<String>) {
    let trailing = value.len() - value.trim_end_matches('\n').len();
    let body = value.trim_end_matches('\n');
    out.push_str(" |");
    if body.starts_with(' ') {
        out.push('2');
    }
    match trailing {
        0 => out.push('-'),
        1 => {}
        _ => out.push('+'),
    }
    if let Some(c) = comment {
        out.push(' ');
        out.push_str(c);
    }
    out.push('\n');
    if !body.is_empty() || trailing > 0 {
        for l in body.split('\n') {
            if !l.is_empty() {
                pad(out, ind + 2);
                out.push_str(l);
            }
            out.push('\n');
        }
    }
    for _ in 1..trailing.max(1) {
        if trailing > 1 {
            out.push('\n');
        }
    }
}

fn inline_repr(n: &Node, ind: usize) -> String {
    match n {
        Node::Scalar { value, style } => scalar_repr(value, *style, ind, false),
        Node::Map {
            entries,
            flow: Flow::Split,
        } if entries.is_empty() => format!("{{\n{}}}", " ".repeat(ind + 2)),
        Node::Seq {
            items,
            flow: Flow::Split,
        } if items.is_empty() => format!("[\n{}]", " ".repeat(ind + 2)),
        Node::Seq { items, .. } => {
            let parts: Vec<String> = items.iter().map(|i| inline_repr(i, ind)).collect();
            format!("[{}]", parts.join(", "))
        }
        Node::Map { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|e| {
                    format!(
                        "{}: {}",
                        scalar_repr(&e.key, e.key_style, ind, true),
                        inline_repr(&e.value, ind)
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn scalar_repr(value: &str, style: Style, _ind: usize, _key: bool) -> String {
    match style {
        Style::Plain => value.to_string(),
        Style::Single => format!("'{}'", value.replace('\'', "''")),
        Style::Double | Style::Literal | Style::Folded => {
            let mut s = String::from("\"");
            for c in value.chars() {
                match c {
                    '"' => s.push_str("\\\""),
                    '\\' => s.push_str("\\\\"),
                    '\n' => s.push_str("\\n"),
                    '\t' => s.push_str("\\t"),
                    '\r' => s.push_str("\\r"),
                    '\0' => s.push_str("\\0"),
                    c if c.is_control() => s.push_str(&format!("\\u{:04X}", c as u32)),
                    c => s.push(c),
                }
            }
            s.push('"');
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_matches_snakeyaml_types() {
        for (v, t) in [
            ("", "null"),
            ("~", "null"),
            ("null", "null"),
            ("yes", "bool"),
            ("Off", "bool"),
            ("true", "bool"),
            ("0", "int"),
            ("12", "int"),
            ("-3", "int"),
            ("0x1F", "int"),
            ("0777", "int"),
            ("1_000", "int"),
            ("1:30", "int"),
            ("08", "float"),
            ("1.5", "float"),
            (".5", "float"),
            ("1e5", "float"),
            ("-.inf", "float"),
            ("2026-11-11", "timestamp"),
            ("2026-11-11 10:00:00", "timestamp"),
            ("abc", "str"),
            ("1.2.3", "str"),
            ("minecraft:stone", "str"),
            ("1s", "str"),
            ("y", "str"),
            ("n", "str"),
        ] {
            assert_eq!(resolve_plain(v), t, "{v:?}");
        }
        assert_eq!(parse_int("0777"), Some(511));
        assert_eq!(parse_int("0x1F"), Some(31));
        assert_eq!(parse_int("1:30"), Some(90));
        assert_eq!(parse_int("-1_000"), Some(-1000));
    }

    #[test]
    fn block_layout_round_trips() {
        let src = "# header\n\nroot:\n  a: 1\n  b: 'x: y'\n  list:\n  - one\n  - k: v\n    k2: w\n  empty: []\n  m: {}\nnull-value:\n";
        let doc = parse_document(src).unwrap();
        assert_eq!(doc.root.path("root.a").unwrap().as_i64(), Some(1));
        assert_eq!(doc.root.path("root.b").unwrap().as_str(), Some("x: y"));
        assert_eq!(
            doc.root.path("root.list").unwrap().items()[1]
                .get("k2")
                .unwrap()
                .as_str(),
            Some("w")
        );
        assert!(doc.root.get("null-value").unwrap().is_null());
        assert_eq!(write_document(&doc), src);
    }

    #[test]
    fn block_scalars_and_quotes() {
        let src = "a: |-\n  line1\n  line2\nb: >\n  folded\n  text\n\nc: \"esc \\\"q\\\" \\u00e9\"\nd: 'it''s'\n";
        let n = parse(src).unwrap();
        assert_eq!(n.get("a").unwrap().as_str(), Some("line1\nline2"));
        assert_eq!(n.get("b").unwrap().as_str(), Some("folded text\n"));
        assert_eq!(n.get("c").unwrap().as_str(), Some("esc \"q\" é"));
        assert_eq!(n.get("d").unwrap().as_str(), Some("it's"));
    }

    #[test]
    fn set_path_and_new_strings() {
        let mut n = Node::map();
        n.set_path("a.b", Node::str("&4[Core]"));
        n.set("c", Node::str("plain"));
        n.set("d", Node::str("yes"));
        assert_eq!(write(&n), "a:\n  b: '&4[Core]'\nc: plain\nd: 'yes'\n");
    }
}
