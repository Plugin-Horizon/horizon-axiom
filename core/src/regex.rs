// Written for this port (no crate): the world filters `whitelist-world-regex` and `blacklist-world-regex` of
// AxiomPaperPlugin's config.yml are matched with Java `String.matches`, which needs the whole text to match.
//
//! A small regular-expression matcher with Java `String.matches` semantics: the whole text must match the pattern.
//!
//! Supported: literals, `.` (not a line terminator), character classes `[...]` with ranges, negation `[^...]` and
//! `\d \w \s` inside, the escapes `\d \D \w \W \s \S \t \n \r \f` and escaped punctuation, groups `( )` and `(?: )`,
//! alternation `|` (empty branches included), the quantifiers `* + ? {n} {n,} {n,m}` (a lazy `?` suffix is accepted:
//! it does not change whether a whole text matches), and the anchors `^` and `$`.
//!
//! Everything else is a syntax error, so a filter that cannot be read refuses the world: possessive quantifiers,
//! backreferences, lookaround, inline flags, `\p{..}`, `\b`, nested classes and `&&`.
//!
//! Matching is a Pike VM over the compiled program: no backtracking, so the time is linear in the text length times
//! the program size, whatever the pattern.

use std::fmt;

/// The largest count a `{n}`, `{n,}` or `{n,m}` quantifier may name.
const MAX_REPEAT: u32 = 1000;
/// The largest program a pattern may compile to; past it the pattern is refused.
const MAX_PROGRAM: usize = 100_000;

/// A pattern that does not parse, with the position (in characters) where the problem was seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexError {
    pub index: usize,
    pub message: String,
}

impl fmt::Display for RegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at index {}", self.message, self.index)
    }
}

impl std::error::Error for RegexError {}

/// A compiled pattern.
#[derive(Debug, Clone)]
pub struct Regex {
    program: Vec<Inst>,
}

impl Regex {
    /// Compiles `pattern`; an error when it uses anything outside the supported syntax.
    pub fn new(pattern: &str) -> Result<Self, RegexError> {
        let node = parse(pattern)?;
        let mut program = Vec::new();
        compile(&node, &mut program)?;
        program.push(Inst::Match);
        Ok(Self { program })
    }

    /// Java `String.matches`: true when the whole of `text` matches.
    pub fn is_full_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let n = self.program.len();
        // `mark[pc] == stamp` means `pc` is already in the thread list of that position. Stamps are unique per list.
        let mut mark = vec![usize::MAX; n];
        let mut current = Vec::new();
        self.add_thread(0, 0, len, 0, &mut current, &mut mark);
        let mut next = Vec::new();
        for (i, &c) in chars.iter().enumerate() {
            next.clear();
            for &pc in &current {
                let accepts = match &self.program[pc] {
                    Inst::Char(expected) => *expected == c,
                    Inst::Any => !is_line_terminator(c),
                    Inst::Class(class) => class.matches(c),
                    _ => false,
                };
                if accepts {
                    self.add_thread(pc + 1, i + 1, len, i + 1, &mut next, &mut mark);
                }
            }
            std::mem::swap(&mut current, &mut next);
            if current.is_empty() {
                return false;
            }
        }
        current.iter().any(|&pc| matches!(self.program[pc], Inst::Match))
    }

    /// Adds `pc` and everything reachable from it without consuming a character, at text position `pos` of `len`.
    fn add_thread(
        &self,
        start: usize,
        pos: usize,
        len: usize,
        stamp: usize,
        list: &mut Vec<usize>,
        mark: &mut [usize],
    ) {
        let mut stack = vec![start];
        while let Some(pc) = stack.pop() {
            if mark[pc] == stamp {
                continue;
            }
            mark[pc] = stamp;
            match &self.program[pc] {
                Inst::Split(a, b) => {
                    stack.push(*b);
                    stack.push(*a);
                }
                Inst::Jmp(target) => stack.push(*target),
                Inst::Start => {
                    if pos == 0 {
                        stack.push(pc + 1);
                    }
                }
                Inst::End => {
                    if pos == len {
                        stack.push(pc + 1);
                    }
                }
                Inst::Char(_) | Inst::Any | Inst::Class(_) | Inst::Match => list.push(pc),
            }
        }
    }
}

/// Java `String.matches(pattern)` in one call: an error for a pattern that does not compile.
pub fn full_match(pattern: &str, text: &str) -> Result<bool, RegexError> {
    Ok(Regex::new(pattern)?.is_full_match(text))
}

/// Line terminators for `.`, as Java has them without UNIX_LINES.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    /// Inclusive range; a single character is `Range(c, c)`.
    Range(char, char),
    /// `\d` (or `\D` when negated).
    Digit(bool),
    /// `\w` (or `\W`).
    Word(bool),
    /// `\s` (or `\S`).
    Space(bool),
}

impl Item {
    fn matches(self, c: char) -> bool {
        match self {
            Item::Range(lo, hi) => lo <= c && c <= hi,
            Item::Digit(negated) => c.is_ascii_digit() != negated,
            Item::Word(negated) => (c.is_ascii_alphanumeric() || c == '_') != negated,
            Item::Space(negated) => is_space(c) != negated,
        }
    }
}

/// Java's `\s` without UNIX_LINES: space, tab, newline, vertical tab, form feed, carriage return.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Class {
    negated: bool,
    items: Vec<Item>,
}

impl Class {
    fn matches(&self, c: char) -> bool {
        self.items.iter().any(|item| item.matches(c)) != self.negated
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Empty,
    Literal(char),
    Any,
    Class(Class),
    Start,
    End,
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
}

/// One instruction of the compiled program. `Split` and `Jmp` name instruction indices.
#[derive(Debug, Clone)]
enum Inst {
    Char(char),
    Any,
    Class(Class),
    Start,
    End,
    Split(usize, usize),
    Jmp(usize),
    Match,
}

/// The result of an escape: a single character, or a class item such as `\d`.
enum Escaped {
    Char(char),
    Item(Item),
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

fn parse(pattern: &str) -> Result<Node, RegexError> {
    let mut parser = Parser {
        chars: pattern.chars().collect(),
        pos: 0,
    };
    let node = parser.alternation()?;
    if parser.pos < parser.chars.len() {
        // Only a `)` with no group open stops the top level early.
        return parser.error("unmatched closing parenthesis");
    }
    Ok(node)
}

impl Parser {
    fn error<T>(&self, message: &str) -> Result<T, RegexError> {
        Err(RegexError {
            index: self.pos,
            message: message.to_owned(),
        })
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn alternation(&mut self) -> Result<Node, RegexError> {
        let mut branches = vec![self.concatenation()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.concatenation()?);
        }
        if branches.len() == 1 {
            Ok(branches.remove(0))
        } else {
            Ok(Node::Alt(branches))
        }
    }

    fn concatenation(&mut self) -> Result<Node, RegexError> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom)?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.remove(0),
            _ => Node::Concat(items),
        })
    }

    fn atom(&mut self) -> Result<Node, RegexError> {
        let Some(c) = self.bump() else {
            return self.error("unexpected end of pattern");
        };
        match c {
            '(' => {
                if self.peek() == Some('?') {
                    self.pos += 1;
                    if self.bump() != Some(':') {
                        return self.error("unsupported group construct, only (?: ) is supported");
                    }
                }
                let inner = self.alternation()?;
                if self.bump() != Some(')') {
                    return self.error("unclosed group");
                }
                Ok(inner)
            }
            '[' => Ok(Node::Class(self.class()?)),
            '.' => Ok(Node::Any),
            '^' => Ok(Node::Start),
            '$' => Ok(Node::End),
            '*' | '+' | '?' => self.error("dangling metacharacter"),
            '{' => self.error("illegal repetition"),
            '\\' => Ok(match self.escape()? {
                Escaped::Char(c) => Node::Literal(c),
                Escaped::Item(item) => Node::Class(Class {
                    negated: false,
                    items: vec![item],
                }),
            }),
            c => Ok(Node::Literal(c)),
        }
    }

    /// The part after a backslash.
    fn escape(&mut self) -> Result<Escaped, RegexError> {
        let Some(c) = self.bump() else {
            return self.error("trailing backslash");
        };
        Ok(match c {
            'd' => Escaped::Item(Item::Digit(false)),
            'D' => Escaped::Item(Item::Digit(true)),
            'w' => Escaped::Item(Item::Word(false)),
            'W' => Escaped::Item(Item::Word(true)),
            's' => Escaped::Item(Item::Space(false)),
            'S' => Escaped::Item(Item::Space(true)),
            't' => Escaped::Char('\t'),
            'n' => Escaped::Char('\n'),
            'r' => Escaped::Char('\r'),
            'f' => Escaped::Char('\u{0C}'),
            c if c.is_ascii_alphanumeric() => return self.error("unsupported escape sequence"),
            c => Escaped::Char(c),
        })
    }

    /// The quantifier after an atom, if any, with its lazy or possessive suffix.
    fn quantified(&mut self, node: Node) -> Result<Node, RegexError> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') => {
                self.pos += 1;
                self.counted()?
            }
            _ => return Ok(node),
        };
        match self.peek() {
            Some('?') => self.pos += 1,
            Some('+') => return self.error("possessive quantifiers are not supported"),
            _ => {}
        }
        if matches!(node, Node::Start | Node::End) {
            return self.error("quantifier after an anchor");
        }
        if matches!(self.peek(), Some('*' | '+' | '?' | '{')) {
            return self.error("dangling metacharacter");
        }
        Ok(Node::Repeat {
            node: Box::new(node),
            min,
            max,
        })
    }

    /// The inside of `{n}`, `{n,}` or `{n,m}`, after the `{`.
    fn counted(&mut self) -> Result<(u32, Option<u32>), RegexError> {
        let Some(min) = self.number() else {
            return self.error("illegal repetition");
        };
        let max = if self.peek() == Some(',') {
            self.pos += 1;
            if self.peek() == Some('}') {
                None
            } else {
                let Some(max) = self.number() else {
                    return self.error("illegal repetition");
                };
                Some(max)
            }
        } else {
            Some(min)
        };
        if self.bump() != Some('}') {
            return self.error("illegal repetition");
        }
        if max.is_some_and(|m| m < min) {
            return self.error("illegal repetition range");
        }
        if min > MAX_REPEAT || max.is_some_and(|m| m > MAX_REPEAT) {
            return self.error("repetition count too large");
        }
        Ok((min, max))
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
            value = value.saturating_mul(10).saturating_add(digit);
            self.pos += 1;
        }
        if self.pos == start {
            None
        } else {
            Some(value)
        }
    }

    /// A character class, after its `[`.
    fn class(&mut self) -> Result<Class, RegexError> {
        let negated = if self.peek() == Some('^') {
            self.pos += 1;
            true
        } else {
            false
        };
        let mut items = Vec::new();
        loop {
            let Some(c) = self.bump() else {
                return self.error("unclosed character class");
            };
            if c == ']' {
                if items.is_empty() {
                    return self.error("empty character class");
                }
                break;
            }
            let lo = match c {
                '[' => return self.error("nested character classes are not supported"),
                '&' if self.peek() == Some('&') => {
                    return self.error("character class intersection is not supported");
                }
                '\\' => match self.escape()? {
                    Escaped::Item(item) => {
                        items.push(item);
                        continue;
                    }
                    Escaped::Char(c) => c,
                },
                c => c,
            };
            // `lo-hi` is a range; a `-` right before `]` is a literal.
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&next| next != ']') {
                self.pos += 1;
                let hi = match self.bump() {
                    Some('\\') => match self.escape()? {
                        Escaped::Char(c) => c,
                        Escaped::Item(_) => return self.error("a class escape cannot end a range"),
                    },
                    Some('[') => return self.error("nested character classes are not supported"),
                    Some(c) => c,
                    None => return self.error("unclosed character class"),
                };
                if hi < lo {
                    return self.error("illegal character range");
                }
                items.push(Item::Range(lo, hi));
            } else {
                items.push(Item::Range(lo, lo));
            }
        }
        Ok(Class { negated, items })
    }
}

fn compile(node: &Node, out: &mut Vec<Inst>) -> Result<(), RegexError> {
    if out.len() > MAX_PROGRAM {
        return Err(RegexError {
            index: 0,
            message: "pattern is too large".to_owned(),
        });
    }
    match node {
        Node::Empty => {}
        Node::Literal(c) => out.push(Inst::Char(*c)),
        Node::Any => out.push(Inst::Any),
        Node::Class(class) => out.push(Inst::Class(class.clone())),
        Node::Start => out.push(Inst::Start),
        Node::End => out.push(Inst::End),
        Node::Concat(items) => {
            for item in items {
                compile(item, out)?;
            }
        }
        Node::Alt(branches) => {
            let mut jumps = Vec::new();
            for (i, branch) in branches.iter().enumerate() {
                if i + 1 < branches.len() {
                    let split = out.len();
                    out.push(Inst::Split(0, 0));
                    compile(branch, out)?;
                    jumps.push(out.len());
                    out.push(Inst::Jmp(0));
                    let next = out.len();
                    out[split] = Inst::Split(split + 1, next);
                } else {
                    compile(branch, out)?;
                }
            }
            let end = out.len();
            for jump in jumps {
                out[jump] = Inst::Jmp(end);
            }
        }
        Node::Repeat { node, min, max } => {
            for _ in 0..*min {
                compile(node, out)?;
            }
            match max {
                None => {
                    let top = out.len();
                    out.push(Inst::Split(0, 0));
                    compile(node, out)?;
                    out.push(Inst::Jmp(top));
                    let end = out.len();
                    out[top] = Inst::Split(top + 1, end);
                }
                Some(max) => {
                    let mut splits = Vec::new();
                    for _ in *min..*max {
                        splits.push(out.len());
                        out.push(Inst::Split(0, 0));
                        compile(node, out)?;
                    }
                    let end = out.len();
                    for split in splits {
                        out[split] = Inst::Split(split + 1, end);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `text` fully matches `pattern`; panics when the pattern does not compile.
    fn m(pattern: &str, text: &str) -> bool {
        match full_match(pattern, text) {
            Ok(result) => result,
            Err(e) => panic!("pattern {pattern:?} should compile: {e}"),
        }
    }

    fn rejects(pattern: &str) {
        assert!(full_match(pattern, "").is_err(), "pattern {pattern:?} should be refused");
    }

    #[test]
    fn literal_is_a_whole_text_match() {
        assert!(m("lobby", "lobby"));
        assert!(!m("lobby", "lobby2"));
        assert!(!m("lobby", "xlobby"));
        assert!(!m("lobby", "lob"));
        assert!(!m("lobby", ""));
    }

    #[test]
    fn empty_pattern_matches_only_empty_text() {
        assert!(m("", ""));
        assert!(!m("", "a"));
    }

    #[test]
    fn dot_matches_one_char_but_not_line_terminators() {
        assert!(m("a.c", "abc"));
        assert!(m("a.c", "a c"));
        assert!(!m("a.c", "ac"));
        assert!(!m("a.c", "a\nc"));
        assert!(!m("a.c", "a\rc"));
        assert!(!m("a.c", "a\u{2028}c"));
        assert!(m(".", "é"));
    }

    #[test]
    fn character_classes_with_ranges() {
        assert!(m("[a-z0-9_]+", "world_01"));
        assert!(!m("[a-z0-9_]+", "World"));
        assert!(!m("[a-z0-9_]+", "a-b"));
        assert!(m("[abc]", "b"));
        assert!(!m("[abc]", "d"));
    }

    #[test]
    fn negated_class() {
        assert!(m("[^0-9]+", "abc"));
        assert!(!m("[^0-9]+", "ab3"));
        assert!(m("[^a]", "b"));
        assert!(!m("[^a]", "a"));
    }

    #[test]
    fn dash_at_class_edges_is_literal() {
        assert!(m("[a-]+", "a-a-"));
        assert!(m("[-a]+", "-a"));
        assert!(!m("[a-]", "b"));
    }

    #[test]
    fn caret_inside_class_is_literal_when_not_first() {
        assert!(m("[a^]", "^"));
        assert!(m("[a^]", "a"));
    }

    #[test]
    fn class_escapes_inside_brackets() {
        assert!(m("[\\d_]+", "12_3"));
        assert!(!m("[\\d_]+", "12a"));
        assert!(m("[\\w-]+", "ab-cd_9"));
        assert!(m("[\\s]", " "));
        assert!(m("[\\n]", "\n"));
        rejects("[a-\\n]");
    }

    #[test]
    fn digit_word_space_escapes() {
        assert!(m("\\d\\d", "42"));
        assert!(!m("\\d\\d", "4a"));
        assert!(m("\\D", "x"));
        assert!(!m("\\D", "5"));
        assert!(m("\\w+", "abc_123"));
        assert!(!m("\\w+", "abc-123"));
        assert!(m("\\W", "-"));
        assert!(!m("\\W", "a"));
        assert!(m("a\\sb", "a\tb"));
        assert!(m("a\\sb", "a\u{0B}b"));
        assert!(!m("a\\sb", "a\u{A0}b"));
        assert!(m("\\S+", "abc"));
        assert!(!m("\\S+", "a b"));
    }

    #[test]
    fn digits_and_word_are_ascii_only() {
        assert!(!m("\\d", "٣"));
        assert!(!m("\\w", "é"));
    }

    #[test]
    fn escaped_punctuation_is_literal() {
        assert!(m("a\\.b", "a.b"));
        assert!(!m("a\\.b", "axb"));
        assert!(m("\\(x\\)", "(x)"));
        assert!(m("\\[\\]", "[]"));
        assert!(m("\\*\\+\\?", "*+?"));
        assert!(m("\\|", "|"));
        assert!(m("\\\\", "\\"));
        assert!(m("\\^\\$", "^$"));
        assert!(m("a\\ b", "a b"));
    }

    #[test]
    fn control_escapes() {
        assert!(m("\\t", "\t"));
        assert!(m("\\n", "\n"));
        assert!(m("\\r\\f", "\r\u{0C}"));
    }

    #[test]
    fn star_plus_question() {
        assert!(m("a*", ""));
        assert!(m("a*", "aaaa"));
        assert!(!m("a*", "aab"));
        assert!(!m("a+", ""));
        assert!(m("a+", "a"));
        assert!(m("ab?c", "ac"));
        assert!(m("ab?c", "abc"));
        assert!(!m("ab?c", "abbc"));
    }

    #[test]
    fn counted_repetition() {
        assert!(m("a{3}", "aaa"));
        assert!(!m("a{3}", "aa"));
        assert!(!m("a{3}", "aaaa"));
        assert!(m("a{2,}", "aa"));
        assert!(m("a{2,}", "aaaaa"));
        assert!(!m("a{2,}", "a"));
        assert!(m("a{2,4}", "aa"));
        assert!(m("a{2,4}", "aaaa"));
        assert!(!m("a{2,4}", "a"));
        assert!(!m("a{2,4}", "aaaaa"));
        assert!(m("a{0}", ""));
        assert!(!m("a{0}", "a"));
        assert!(m("a{0,1}", ""));
        assert!(m("(ab){2}", "abab"));
        assert!(!m("(ab){2}", "ab"));
    }

    #[test]
    fn counted_repetition_on_classes_and_groups() {
        assert!(m("[0-9]{4}-[0-9]{2}", "2026-11"));
        assert!(!m("[0-9]{4}-[0-9]{2}", "2026-1"));
        assert!(m("(x|y){1,3}", "xyx"));
        assert!(!m("(x|y){1,3}", "xyxy"));
    }

    #[test]
    fn large_counts_still_compile() {
        let text = "a".repeat(1000);
        assert!(m("a{1000}", &text));
        assert!(!m("a{1000}", &text[1..]));
        assert!(m("a{0,1000}", &text));
    }

    #[test]
    fn counts_above_the_limit_are_refused() {
        rejects("a{1001}");
        rejects("a{1,1001}");
        rejects("a{1001,}");
    }

    #[test]
    fn alternation_and_groups() {
        assert!(m("lobby|hub", "hub"));
        assert!(m("lobby|hub", "lobby"));
        assert!(!m("lobby|hub", "lobbyhub"));
        assert!(m("(lobby|hub)_[0-9]+", "hub_12"));
        assert!(!m("(lobby|hub)_[0-9]+", "other_12"));
        assert!(m("(?:ab)+", "ababab"));
        assert!(m("a(b|c|)d", "ad"));
        assert!(m("a(b|c|)d", "acd"));
        assert!(!m("a(b|c|)d", "abcd"));
    }

    #[test]
    fn empty_alternative_branches() {
        assert!(m("a|", ""));
        assert!(m("a|", "a"));
        assert!(m("|a", ""));
        assert!(!m("|a", "b"));
        assert!(m("()", ""));
        assert!(m("(|)", ""));
    }

    #[test]
    fn nested_groups() {
        assert!(m("((a|b)c)+", "acbcac"));
        assert!(!m("((a|b)c)+", "acb"));
        assert!(m("(a(b(c)))", "abc"));
    }

    #[test]
    fn anchors_at_the_ends_of_a_full_match() {
        assert!(m("^lobby$", "lobby"));
        assert!(!m("^lobby$", "lobby\n"));
        assert!(m("^a|b$", "a"));
        assert!(m("^a|b$", "b"));
        assert!(!m("^a|b$", "ab"));
        assert!(!m("a^", ""));
        assert!(!m("a$b", "ab"));
    }

    #[test]
    fn anchors_are_not_quantified() {
        rejects("^*");
        rejects("$+");
    }

    #[test]
    fn lazy_suffix_does_not_change_full_match() {
        assert!(m("a*?", "aaa"));
        assert!(m("a+?b", "aaab"));
        assert!(m("a{2,3}?", "aa"));
        assert!(!m("a{2,3}?", "aaaa"));
        assert!(m("a??", ""));
        assert!(m("a??", "a"));
    }

    #[test]
    fn possessive_quantifiers_are_refused() {
        rejects("a*+");
        rejects("a++");
        rejects("a?+");
        rejects("a{2}+");
    }

    #[test]
    fn stacked_quantifiers_are_refused() {
        rejects("a**");
        rejects("a+*");
        rejects("a{2}{3}");
        rejects("a*{2}");
    }

    #[test]
    fn unsupported_constructs_are_refused() {
        rejects("(?i)lobby");
        rejects("(?=a)a");
        rejects("(?!a)a");
        rejects("(?<name>a)");
        rejects("\\1");
        rejects("\\b");
        rejects("\\p{L}");
        rejects("\\y");
        rejects("[[a-z]]");
        rejects("[a-z&&[^b]]");
        rejects("[a&&b]");
    }

    #[test]
    fn malformed_patterns_are_refused() {
        rejects("(");
        rejects("(a");
        rejects(")");
        rejects("a)");
        rejects("[");
        rejects("[a");
        rejects("[]");
        rejects("[^]");
        rejects("[z-a]");
        rejects("*a");
        rejects("+");
        rejects("?");
        rejects("a{");
        rejects("a{x}");
        rejects("a{,3}");
        rejects("a{3,2}");
        rejects("a{}");
        rejects("a{1");
        rejects("\\");
        rejects("(?:a");
        rejects("[a-\\d]");
    }

    #[test]
    fn a_lone_closing_brace_or_bracket_is_a_literal() {
        assert!(m("a}", "a}"));
        assert!(m("a]", "a]"));
    }

    #[test]
    fn realistic_world_filters() {
        let pattern = "^(world_a|world_b|other_[a-z]+)$";
        assert!(m(pattern, "world_a"));
        assert!(m(pattern, "world_b"));
        assert!(m(pattern, "other_x"));
        assert!(!m(pattern, "non_"));
        assert!(!m(pattern, "other_x2"));
        assert!(!m(pattern, "proxy"));
        assert!(m("stage|aether", "aether"));
        assert!(!m("stage|aether", "aether_nether"));
        assert!(m("world(_nether|_the_end)?", "world_the_end"));
        assert!(m("world(_nether|_the_end)?", "world"));
    }

    #[test]
    fn classes_do_not_match_the_empty_text() {
        assert!(!m("[a-z]", ""));
        assert!(!m("\\d", ""));
    }

    #[test]
    fn nested_optional_loops_do_not_hang() {
        // Epsilon loops: the thread list keeps one thread per instruction, so these terminate.
        assert!(m("(a*)*", ""));
        assert!(m("(a*)*", "aaaa"));
        assert!(m("(a?)*b", "aaab"));
        assert!(m("(|a)*", "aaa"));
        assert!(!m("(a*)*b", "aaaac"));
    }

    #[test]
    fn no_exponential_blowup() {
        // A backtracking matcher takes exponential time here; the Pike VM takes linear time.
        let text = "a".repeat(5000);
        assert!(!m("(a*)*b", &text));
        assert!(!m("(a|aa)*c", &text));
        assert!(m("(a|aa)*", &"a".repeat(200)));
    }

    #[test]
    fn unicode_text_is_matched_by_char() {
        assert!(m("[äöü]+", "äöü"));
        assert!(m("..", "éé"));
        assert!(!m("...", "éé"));
    }

    #[test]
    fn error_positions_point_into_the_pattern() {
        let err = Regex::new("ab[cd").unwrap_err();
        assert_eq!(err.index, 5);
        assert!(err.to_string().contains("unclosed character class"));
    }

    #[test]
    fn compiled_regex_can_be_reused() {
        let re = Regex::new("[a-c]+").expect("compiles");
        assert!(re.is_full_match("abc"));
        assert!(!re.is_full_match("abd"));
        assert!(re.is_full_match("cc"));
    }

    #[test]
    fn program_size_limit_refuses_nested_repetition() {
        // 1000 * 1000 * 1000 copies would not fit; the compile stops at the limit.
        rejects("((a{1000}){1000}){1000}");
    }
}
