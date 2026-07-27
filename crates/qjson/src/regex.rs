//! A tiny backtracking regular-expression engine.
//!
//! It exists so that [`crate::schema`] can support the `pattern` keyword without
//! pulling in a dependency, but it is public so any crate in the workspace can
//! use it for lightweight matching.
//!
//! # Supported syntax
//!
//! | Construct | Meaning |
//! |---|---|
//! | `^` `$` | start / end of the subject (no multiline mode) |
//! | `.` | any character except `\n` |
//! | `a` | a literal character |
//! | `\d \D \w \W \s \S` | digit / word / whitespace classes and negations |
//! | `\. \\ \- \/ \+ \* \? \( \) \[ \] \{ \} \| \^ \$` | escaped literals |
//! | `\n \r \t \f \v \0` | control escapes |
//! | `[a-z0-9_-]`, `[^abc]` | character classes with ranges and negation |
//! | `*` `+` `?` `{m}` `{m,}` `{m,n}` | quantifiers, optionally lazy with `?` |
//! | `(...)` and `\|` | non-capturing grouping and alternation |
//!
//! A `{` that does not open a well-formed quantifier is treated as a literal.
//!
//! # Backtracking cap
//!
//! Matching is bounded at [`MAX_STEPS`] steps. A pattern/subject pair that would
//! backtrack catastrophically returns [`RegexError`] instead of hanging.

use std::cell::Cell;
use std::fmt;

/// Maximum number of match steps before a match attempt is abandoned.
pub const MAX_STEPS: u64 = 100_000;

/// Maximum nesting depth accepted by the pattern parser.
pub const MAX_GROUP_DEPTH: usize = 32;

/// Maximum recursion depth during matching, a stack-overflow guard.
const MAX_MATCH_DEPTH: u32 = 4_000;

/// An error from compiling or running a regular expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegexError {
    /// Human-readable explanation.
    pub message: String,
    /// `true` when the error is the backtracking step cap rather than a syntax problem.
    pub step_limit: bool,
}

impl RegexError {
    /// Builds a syntax error.
    fn syntax(message: impl Into<String>) -> Self {
        RegexError {
            message: message.into(),
            step_limit: false,
        }
    }

    /// Builds the step-cap error.
    fn overflow() -> Self {
        RegexError {
            message: format!("regex backtracking exceeded {MAX_STEPS} steps"),
            step_limit: true,
        }
    }
}

impl fmt::Display for RegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RegexError {}

/// One element of a character class.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ClassItem {
    /// A single literal character.
    Ch(char),
    /// An inclusive character range.
    Range(char, char),
    /// `\d` or, when negated, `\D`.
    Digit(bool),
    /// `\w` or, when negated, `\W`.
    Word(bool),
    /// `\s` or, when negated, `\S`.
    Space(bool),
}

impl ClassItem {
    /// Tests a single character against this item.
    fn matches(&self, c: char) -> bool {
        match self {
            ClassItem::Ch(x) => c == *x,
            ClassItem::Range(a, b) => c >= *a && c <= *b,
            ClassItem::Digit(pos) => c.is_ascii_digit() == *pos,
            ClassItem::Word(pos) => (c.is_alphanumeric() || c == '_') == *pos,
            ClassItem::Space(pos) => c.is_whitespace() == *pos,
        }
    }
}

/// A parsed character class.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CharClass {
    /// Whether the class was written as `[^...]`.
    negated: bool,
    /// Items forming the class.
    items: Vec<ClassItem>,
}

impl CharClass {
    /// Tests a single character against this class.
    fn matches(&self, c: char) -> bool {
        let hit = self.items.iter().any(|i| i.matches(c));
        hit != self.negated
    }
}

/// A node of the compiled pattern tree.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    /// A literal character.
    Char(char),
    /// `.`
    Any,
    /// A character class.
    Class(CharClass),
    /// `^`
    Start,
    /// `$`
    End,
    /// A group / alternation: a list of alternative sequences.
    Alt(Vec<Vec<Node>>),
    /// A quantified node.
    Repeat {
        /// The repeated node.
        node: Box<Node>,
        /// Minimum repetitions.
        min: u32,
        /// Maximum repetitions, `None` for unbounded.
        max: Option<u32>,
        /// Whether the quantifier is greedy.
        greedy: bool,
    },
}

/// A compiled regular expression.
#[derive(Clone, Debug)]
pub struct Regex {
    /// The original pattern text.
    pattern: String,
    /// Top-level alternation.
    root: Vec<Vec<Node>>,
    /// `true` when the pattern begins with `^` in every alternative.
    anchored_start: bool,
}

impl Regex {
    /// Compiles `pattern`.
    ///
    /// # Errors
    ///
    /// Returns [`RegexError`] when the pattern uses unsupported syntax, is
    /// malformed, or nests groups deeper than [`MAX_GROUP_DEPTH`].
    pub fn new(pattern: &str) -> Result<Regex, RegexError> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = PatternParser {
            chars: &chars,
            pos: 0,
            depth: 0,
        };
        let root = p.parse_alt()?;
        if p.pos != chars.len() {
            return Err(RegexError::syntax(format!(
                "unexpected '{}' at index {} in pattern",
                chars[p.pos], p.pos
            )));
        }
        let anchored_start = root
            .iter()
            .all(|branch| matches!(branch.first(), Some(Node::Start)));
        Ok(Regex {
            pattern: pattern.to_string(),
            root,
            anchored_start,
        })
    }

    /// Returns the pattern text this regex was compiled from.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Returns `true` when the pattern matches anywhere in `text`.
    ///
    /// # Errors
    ///
    /// Returns [`RegexError`] with `step_limit == true` if the backtracking cap
    /// is reached.
    pub fn is_match(&self, text: &str) -> Result<bool, RegexError> {
        Ok(self.find(text)?.is_some())
    }

    /// Returns the character index range of the leftmost match, if any.
    ///
    /// Indices count `char`s, not bytes.
    ///
    /// # Errors
    ///
    /// Returns [`RegexError`] with `step_limit == true` if the backtracking cap
    /// is reached.
    pub fn find(&self, text: &str) -> Result<Option<(usize, usize)>, RegexError> {
        let chars: Vec<char> = text.chars().collect();
        let m = Matcher {
            text: &chars,
            steps: Cell::new(0),
        };
        let last_start = if self.anchored_start { 0 } else { chars.len() };
        for start in 0..=last_start {
            let end = Cell::new(usize::MAX);
            let hit = {
                let mut cont = |p: usize| {
                    end.set(p);
                    Ok(true)
                };
                m.match_alt(&self.root, start, 0, &mut cont)?
            };
            if hit {
                return Ok(Some((start, end.get())));
            }
        }
        Ok(None)
    }

    /// Returns `true` when the pattern matches the whole of `text`.
    ///
    /// # Errors
    ///
    /// Returns [`RegexError`] with `step_limit == true` if the backtracking cap
    /// is reached.
    pub fn is_full_match(&self, text: &str) -> Result<bool, RegexError> {
        let chars: Vec<char> = text.chars().collect();
        let m = Matcher {
            text: &chars,
            steps: Cell::new(0),
        };
        let n = chars.len();
        let mut cont = |p: usize| Ok(p == n);
        m.match_alt(&self.root, 0, 0, &mut cont)
    }
}

/// Compiles `pattern` and tests it against `text` in one step.
///
/// # Errors
///
/// Returns [`RegexError`] for a malformed pattern or a backtracking overflow.
pub fn is_match(pattern: &str, text: &str) -> Result<bool, RegexError> {
    Regex::new(pattern)?.is_match(text)
}

/// Recursive-descent parser for the supported pattern syntax.
struct PatternParser<'a> {
    /// The pattern as characters.
    chars: &'a [char],
    /// Current read position.
    pos: usize,
    /// Current group nesting depth.
    depth: usize,
}

impl PatternParser<'_> {
    /// Returns the character at the cursor, if any.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Parses `branch ('|' branch)*`.
    fn parse_alt(&mut self) -> Result<Vec<Vec<Node>>, RegexError> {
        let mut branches = vec![self.parse_branch()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.parse_branch()?);
        }
        Ok(branches)
    }

    /// Parses a sequence of quantified atoms.
    fn parse_branch(&mut self) -> Result<Vec<Node>, RegexError> {
        let mut seq = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.parse_atom()?;
            seq.push(self.parse_quantifier(atom)?);
        }
        Ok(seq)
    }

    /// Parses one atom.
    fn parse_atom(&mut self) -> Result<Node, RegexError> {
        let c = self
            .peek()
            .ok_or_else(|| RegexError::syntax("unexpected end of pattern"))?;
        match c {
            '^' => {
                self.pos += 1;
                Ok(Node::Start)
            }
            '$' => {
                self.pos += 1;
                Ok(Node::End)
            }
            '.' => {
                self.pos += 1;
                Ok(Node::Any)
            }
            '(' => {
                if self.depth >= MAX_GROUP_DEPTH {
                    return Err(RegexError::syntax(format!(
                        "pattern nests groups deeper than {MAX_GROUP_DEPTH}"
                    )));
                }
                self.pos += 1;
                // Accept and ignore the non-capturing marker "?:".
                if self.chars.get(self.pos) == Some(&'?')
                    && self.chars.get(self.pos + 1) == Some(&':')
                {
                    self.pos += 2;
                }
                self.depth += 1;
                let alt = self.parse_alt()?;
                self.depth -= 1;
                if self.peek() != Some(')') {
                    return Err(RegexError::syntax("unclosed '(' in pattern"));
                }
                self.pos += 1;
                Ok(Node::Alt(alt))
            }
            ')' => Err(RegexError::syntax("unmatched ')' in pattern")),
            '[' => self.parse_class(),
            '\\' => self.parse_escape(),
            '*' | '+' | '?' => Err(RegexError::syntax(format!(
                "quantifier '{c}' at index {} has nothing to repeat",
                self.pos
            ))),
            _ => {
                self.pos += 1;
                Ok(Node::Char(c))
            }
        }
    }

    /// Parses a backslash escape outside a character class.
    fn parse_escape(&mut self) -> Result<Node, RegexError> {
        self.pos += 1;
        let c = self
            .peek()
            .ok_or_else(|| RegexError::syntax("pattern ends with a trailing '\\'"))?;
        self.pos += 1;
        Ok(match c {
            'd' => Node::Class(CharClass {
                negated: false,
                items: vec![ClassItem::Digit(true)],
            }),
            'D' => Node::Class(CharClass {
                negated: false,
                items: vec![ClassItem::Digit(false)],
            }),
            'w' => Node::Class(CharClass {
                negated: false,
                items: vec![ClassItem::Word(true)],
            }),
            'W' => Node::Class(CharClass {
                negated: false,
                items: vec![ClassItem::Word(false)],
            }),
            's' => Node::Class(CharClass {
                negated: false,
                items: vec![ClassItem::Space(true)],
            }),
            'S' => Node::Class(CharClass {
                negated: false,
                items: vec![ClassItem::Space(false)],
            }),
            other => Node::Char(escape_char(other)?),
        })
    }

    /// Parses a bracketed character class.
    fn parse_class(&mut self) -> Result<Node, RegexError> {
        self.pos += 1; // consume '['
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut items: Vec<ClassItem> = Vec::new();
        let mut first = true;
        loop {
            let c = self
                .peek()
                .ok_or_else(|| RegexError::syntax("unclosed '[' in pattern"))?;
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            first = false;

            // A class item is either a shorthand escape or a single character
            // that may start a range.
            let lo = if c == '\\' {
                self.pos += 1;
                let e = self
                    .peek()
                    .ok_or_else(|| RegexError::syntax("pattern ends with a trailing '\\'"))?;
                self.pos += 1;
                match e {
                    'd' => {
                        items.push(ClassItem::Digit(true));
                        continue;
                    }
                    'D' => {
                        items.push(ClassItem::Digit(false));
                        continue;
                    }
                    'w' => {
                        items.push(ClassItem::Word(true));
                        continue;
                    }
                    'W' => {
                        items.push(ClassItem::Word(false));
                        continue;
                    }
                    's' => {
                        items.push(ClassItem::Space(true));
                        continue;
                    }
                    'S' => {
                        items.push(ClassItem::Space(false));
                        continue;
                    }
                    other => escape_char(other)?,
                }
            } else {
                self.pos += 1;
                c
            };

            // Range, unless the '-' is the final character before ']'.
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|n| *n != ']') {
                self.pos += 1;
                let hc = self
                    .peek()
                    .ok_or_else(|| RegexError::syntax("unclosed '[' in pattern"))?;
                let hi = if hc == '\\' {
                    self.pos += 1;
                    let e = self
                        .peek()
                        .ok_or_else(|| RegexError::syntax("pattern ends with a trailing '\\'"))?;
                    self.pos += 1;
                    escape_char(e)?
                } else {
                    self.pos += 1;
                    hc
                };
                if hi < lo {
                    return Err(RegexError::syntax(format!(
                        "character range '{lo}-{hi}' is out of order"
                    )));
                }
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::Ch(lo));
            }
        }
        Ok(Node::Class(CharClass { negated, items }))
    }

    /// Applies a trailing quantifier to `atom`, if one is present.
    fn parse_quantifier(&mut self, atom: Node) -> Result<Node, RegexError> {
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
            Some('{') => match self.try_parse_braces()? {
                Some(bounds) => bounds,
                // Not a well-formed quantifier: leave the '{' to be read as a literal.
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        if matches!(atom, Node::Start | Node::End) {
            return Err(RegexError::syntax("anchors cannot be quantified"));
        }
        let greedy = if self.peek() == Some('?') {
            self.pos += 1;
            false
        } else {
            true
        };
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
        })
    }

    /// Tries to read `{m}`, `{m,}` or `{m,n}`; restores the cursor and returns
    /// `None` when the braces do not form a quantifier.
    #[allow(clippy::type_complexity)] // A plain Option<(u32, Option<u32>)>; the alias would not help.
    fn try_parse_braces(&mut self) -> Result<Option<(u32, Option<u32>)>, RegexError> {
        let start = self.pos;
        self.pos += 1; // consume '{'
        let min = match self.read_number() {
            Some(v) => v,
            None => {
                self.pos = start;
                return Ok(None);
            }
        };
        let max = match self.peek() {
            Some('}') => {
                self.pos += 1;
                Some(min)
            }
            Some(',') => {
                self.pos += 1;
                let hi = self.read_number();
                if self.peek() != Some('}') {
                    self.pos = start;
                    return Ok(None);
                }
                self.pos += 1;
                hi
            }
            _ => {
                self.pos = start;
                return Ok(None);
            }
        };
        if let Some(m) = max {
            if m < min {
                return Err(RegexError::syntax(format!(
                    "quantifier {{{min},{m}}} has max below min"
                )));
            }
        }
        Ok(Some((min, max)))
    }

    /// Reads a run of ASCII digits as a `u32`, or `None` if there is none.
    fn read_number(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut v: u32 = 0;
        while let Some(c) = self.peek() {
            if !c.is_ascii_digit() {
                break;
            }
            v = v.saturating_mul(10).saturating_add(c as u32 - '0' as u32);
            self.pos += 1;
        }
        if self.pos == start {
            None
        } else {
            Some(v.min(100_000))
        }
    }
}

/// Resolves a backslash escape to the character it denotes.
fn escape_char(c: char) -> Result<char, RegexError> {
    Ok(match c {
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'f' => '\u{0c}',
        'v' => '\u{0b}',
        '0' => '\0',
        '.' | '\\' | '-' | '/' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
        | '^' | '$' | '"' | '\'' | '#' | '@' | '!' | '&' | '~' | '=' | ':' | ',' | '<' | '>'
        | ';' | '%' | ' ' => c,
        other => {
            return Err(RegexError::syntax(format!(
                "unsupported escape '\\{other}' in pattern"
            )))
        }
    })
}

/// `true` when a node consumes exactly one character and has no sub-nodes.
fn is_width_one(node: &Node) -> bool {
    matches!(node, Node::Char(_) | Node::Any | Node::Class(_))
}

/// Backtracking matcher over a character slice.
struct Matcher<'a> {
    /// The subject, as characters.
    text: &'a [char],
    /// Steps consumed so far, capped at [`MAX_STEPS`].
    steps: Cell<u64>,
}

/// Continuation invoked with the position reached after a node matched.
type Cont<'c> = dyn FnMut(usize) -> Result<bool, RegexError> + 'c;

impl Matcher<'_> {
    /// Consumes one step of the backtracking budget.
    fn step(&self, depth: u32) -> Result<(), RegexError> {
        let n = self.steps.get() + 1;
        if n > MAX_STEPS || depth > MAX_MATCH_DEPTH {
            return Err(RegexError::overflow());
        }
        self.steps.set(n);
        Ok(())
    }

    /// Matches any branch of an alternation at `pos`.
    fn match_alt(
        &self,
        branches: &[Vec<Node>],
        pos: usize,
        depth: u32,
        cont: &mut Cont<'_>,
    ) -> Result<bool, RegexError> {
        self.step(depth)?;
        for branch in branches {
            if self.match_seq(branch, pos, depth + 1, cont)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Matches a sequence of nodes at `pos`, then the continuation.
    fn match_seq(
        &self,
        seq: &[Node],
        pos: usize,
        depth: u32,
        cont: &mut Cont<'_>,
    ) -> Result<bool, RegexError> {
        self.step(depth)?;
        match seq.split_first() {
            None => cont(pos),
            Some((first, rest)) => {
                let mut k = |p: usize| self.match_seq(rest, p, depth + 1, cont);
                self.match_node(first, pos, depth + 1, &mut k)
            }
        }
    }

    /// Matches a single node at `pos`, then the continuation.
    fn match_node(
        &self,
        node: &Node,
        pos: usize,
        depth: u32,
        cont: &mut Cont<'_>,
    ) -> Result<bool, RegexError> {
        self.step(depth)?;
        match node {
            Node::Char(c) => {
                if self.text.get(pos) == Some(c) {
                    cont(pos + 1)
                } else {
                    Ok(false)
                }
            }
            Node::Any => match self.text.get(pos) {
                Some(c) if *c != '\n' => cont(pos + 1),
                _ => Ok(false),
            },
            Node::Class(class) => match self.text.get(pos) {
                Some(c) if class.matches(*c) => cont(pos + 1),
                _ => Ok(false),
            },
            Node::Start => {
                if pos == 0 {
                    cont(pos)
                } else {
                    Ok(false)
                }
            }
            Node::End => {
                if pos == self.text.len() {
                    cont(pos)
                } else {
                    Ok(false)
                }
            }
            Node::Alt(branches) => self.match_alt(branches, pos, depth + 1, cont),
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => self.match_repeat(node, *min, *max, *greedy, 0, pos, depth + 1, cont),
        }
    }

    /// Tests a single-character node at `pos` without a continuation.
    fn match_one(&self, node: &Node, pos: usize) -> bool {
        match node {
            Node::Char(c) => self.text.get(pos) == Some(c),
            Node::Any => matches!(self.text.get(pos), Some(c) if *c != '\n'),
            Node::Class(class) => matches!(self.text.get(pos), Some(c) if class.matches(*c)),
            _ => false,
        }
    }

    /// Matches a quantified node, expanding greedily or lazily.
    #[allow(clippy::too_many_arguments)] // The repeat state is genuinely eight values.
    fn match_repeat(
        &self,
        node: &Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        count: u32,
        pos: usize,
        depth: u32,
        cont: &mut Cont<'_>,
    ) -> Result<bool, RegexError> {
        self.step(depth)?;

        // Fast path for the overwhelmingly common case of a quantified
        // single-character matcher (`a*`, `\d+`, `[a-z]{2,4}`, `.*`). Scanning
        // the run iteratively keeps recursion depth constant, so long subjects
        // never reach the stack guard.
        if count == 0 && is_width_one(node) {
            let cap = max.map_or(usize::MAX, |m| m as usize);
            let mut run = 0usize;
            while run < cap && self.match_one(node, pos + run) {
                self.step(depth)?;
                run += 1;
            }
            let lo = min as usize;
            if run < lo {
                return Ok(false);
            }
            if greedy {
                let mut k = run;
                loop {
                    self.step(depth)?;
                    if cont(pos + k)? {
                        return Ok(true);
                    }
                    if k == lo {
                        return Ok(false);
                    }
                    k -= 1;
                }
            }
            for k in lo..=run {
                self.step(depth)?;
                if cont(pos + k)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }

        if !greedy && count >= min && cont(pos)? {
            return Ok(true);
        }

        if max.is_none_or(|m| count < m) {
            let expanded = {
                let mut k = |p: usize| {
                    if p == pos {
                        // The body matched the empty string. Repeating it again
                        // can never consume input, so `min` is satisfiable here
                        // and we stop expanding to avoid looping forever.
                        return cont(p);
                    }
                    self.match_repeat(node, min, max, greedy, count + 1, p, depth + 1, cont)
                };
                self.match_node(node, pos, depth + 1, &mut k)?
            };
            if expanded {
                return Ok(true);
            }
        }

        if greedy && count >= min {
            return cont(pos);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, text: &str) -> bool {
        is_match(pattern, text).unwrap_or_else(|e| panic!("{pattern} / {text}: {e}"))
    }

    #[test]
    fn literals_match_anywhere_by_default() {
        assert!(m("abc", "abc"));
        assert!(m("abc", "xxabcxx"));
        assert!(!m("abc", "abd"));
        assert!(m("", "anything"));
        assert!(m("", ""));
    }

    #[test]
    fn anchors_restrict_position() {
        assert!(m("^abc", "abcdef"));
        assert!(!m("^abc", "xabcdef"));
        assert!(m("abc$", "xxabc"));
        assert!(!m("abc$", "abcx"));
        assert!(m("^abc$", "abc"));
        assert!(!m("^abc$", "abcd"));
        assert!(m("^$", ""));
        assert!(!m("^$", "x"));
    }

    #[test]
    fn dot_matches_any_char_but_newline() {
        assert!(m("^a.c$", "abc"));
        assert!(m("^a.c$", "a c"));
        assert!(m("^a.c$", "a\u{e9}c"));
        assert!(!m("^a.c$", "a\nc"));
        assert!(!m("^a.c$", "ac"));
    }

    #[test]
    fn shorthand_classes() {
        assert!(m("^\\d+$", "01234"));
        assert!(!m("^\\d+$", "12a"));
        assert!(m("^\\w+$", "a_Z9"));
        assert!(!m("^\\w+$", "a-b"));
        assert!(m("^\\s+$", " \t\n"));
        assert!(!m("^\\s+$", " x"));
        assert!(m("^\\D+$", "abc"));
        assert!(m("^\\W+$", "-.!"));
        assert!(m("^\\S+$", "abc"));
    }

    #[test]
    fn character_classes_with_ranges_and_negation() {
        assert!(m("^[a-z0-9_-]+$", "snake_case-9"));
        assert!(!m("^[a-z0-9_-]+$", "Upper"));
        assert!(m("^[^abc]+$", "xyz"));
        assert!(!m("^[^abc]+$", "xay"));
        assert!(m("^[-a]+$", "-a-"));
        assert!(m("^[a-]+$", "a--"));
        assert!(m("^[\\]]$", "]"));
        assert!(m("^[\\d\\-]+$", "1-2"));
        assert!(m("^[^\\d]+$", "abc"));
    }

    #[test]
    fn quantifiers_star_plus_question() {
        assert!(m("^ab*c$", "ac"));
        assert!(m("^ab*c$", "abbbc"));
        assert!(!m("^ab+c$", "ac"));
        assert!(m("^ab+c$", "abc"));
        assert!(m("^ab?c$", "ac"));
        assert!(m("^ab?c$", "abc"));
        assert!(!m("^ab?c$", "abbc"));
    }

    #[test]
    fn counted_quantifiers() {
        assert!(m("^a{3}$", "aaa"));
        assert!(!m("^a{3}$", "aa"));
        assert!(!m("^a{3}$", "aaaa"));
        assert!(m("^a{2,}$", "aa"));
        assert!(m("^a{2,}$", "aaaaaa"));
        assert!(!m("^a{2,}$", "a"));
        assert!(m("^a{2,4}$", "aa"));
        assert!(m("^a{2,4}$", "aaaa"));
        assert!(!m("^a{2,4}$", "aaaaa"));
        assert!(m("^\\d{4}-\\d{2}-\\d{2}$", "2026-07-26"));
        assert!(!m("^\\d{4}-\\d{2}-\\d{2}$", "226-07-26"));
    }

    #[test]
    fn lazy_quantifiers_are_supported() {
        let r = Regex::new("a+?").expect("compile");
        assert_eq!(r.find("aaa").expect("find"), Some((0, 1)));
        let g = Regex::new("a+").expect("compile");
        assert_eq!(g.find("aaa").expect("find"), Some((0, 3)));
    }

    #[test]
    fn groups_and_alternation() {
        assert!(m("^(cat|dog)$", "cat"));
        assert!(m("^(cat|dog)$", "dog"));
        assert!(!m("^(cat|dog)$", "cow"));
        assert!(m("^(ab)+$", "ababab"));
        assert!(!m("^(ab)+$", "aba"));
        assert!(m("^(a|b|c){2,3}$", "abc"));
        assert!(m("^x(?:yz)?$", "x"));
        assert!(m("^x(?:yz)?$", "xyz"));
        assert!(m("^((a|b)c)+$", "acbc"));
    }

    #[test]
    fn escapes_are_literal() {
        assert!(m("^a\\.c$", "a.c"));
        assert!(!m("^a\\.c$", "abc"));
        assert!(m("^a\\\\c$", "a\\c"));
        assert!(m("^a\\-c$", "a-c"));
        assert!(m("^\\{\\}$", "{}"));
        assert!(m("^\\^\\$$", "^$"));
        assert!(m("^a\\tb$", "a\tb"));
    }

    #[test]
    fn unmatched_brace_is_a_literal() {
        assert!(m("^a{$", "a{"));
        assert!(m("^a{x}$", "a{x}"));
        assert!(m("^a{,2}$", "a{,2}"));
        // A brace group that opens the pattern is a plain literal too.
        assert!(m("^\\{2\\}$", "{2}"));
    }

    #[test]
    fn find_reports_char_ranges() {
        let r = Regex::new("\\d+").expect("compile");
        assert_eq!(r.find("ab 123 cd").expect("find"), Some((3, 6)));
        assert_eq!(r.find("none").expect("find"), None);
        // Char indices, not bytes.
        let r2 = Regex::new("b").expect("compile");
        assert_eq!(r2.find("\u{e9}\u{e9}b").expect("find"), Some((2, 3)));
    }

    #[test]
    fn full_match_helper() {
        let r = Regex::new("a+").expect("compile");
        assert!(r.is_full_match("aaa").expect("match"));
        assert!(!r.is_full_match("aaab").expect("match"));
        assert!(r.is_match("aaab").expect("match"));
    }

    #[test]
    fn catastrophic_backtracking_is_capped() {
        let r = Regex::new("^(a+)+$").expect("compile");
        let subject = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab";
        let err = r.is_match(subject).expect_err("should hit the step cap");
        assert!(err.step_limit, "{err}");
        assert!(err.message.contains("100000"), "{}", err.message);
    }

    #[test]
    fn nested_optional_backtracking_is_capped() {
        let r = Regex::new("^(a|aa)+$").expect("compile");
        let subject = "a".repeat(40) + "b";
        assert!(r.is_match(&subject).expect_err("cap").step_limit);
    }

    #[test]
    fn empty_body_repeat_terminates() {
        assert!(m("^(a?)*$", ""));
        assert!(m("^(a?)*$", "aaa"));
        assert!(m("^(|a)*$", "aa"));
    }

    #[test]
    fn long_subjects_do_not_exhaust_the_recursion_guard() {
        let long = "a".repeat(20_000);
        assert!(m("^a+$", &long));
        assert!(m("^[a-z]*$", &long));
        assert!(m("^.*$", &long));
        assert!(!m("^a{5}$", &long));
        let mixed = format!("{}9", "a".repeat(10_000));
        assert!(m("^[a-z]+\\d$", &mixed));
        assert!(!m("^[a-z]+$", &mixed));
    }

    #[test]
    fn greedy_backtracking_still_works_on_the_fast_path() {
        let r = Regex::new("^a+ab$").expect("compile");
        assert!(r.is_match("aaaab").expect("match"));
        let lazy = Regex::new("^a+?ab$").expect("compile");
        assert!(lazy.is_match("aaaab").expect("match"));
        assert_eq!(
            Regex::new("a{2,4}").expect("c").find("aaaaa").expect("f"),
            Some((0, 4))
        );
        assert_eq!(
            Regex::new("a{2,4}?").expect("c").find("aaaaa").expect("f"),
            Some((0, 2))
        );
    }

    #[test]
    fn compile_errors_are_reported() {
        for bad in [
            "(", ")", "[abc", "a\\", "*a", "+", "?x", "[z-a]", "a{3,2}", "\\q", "^*",
        ] {
            let e = Regex::new(bad).expect_err(bad);
            assert!(!e.step_limit, "{bad}");
            assert!(!e.message.is_empty());
        }
    }

    #[test]
    fn group_depth_is_capped() {
        let deep = "(".repeat(MAX_GROUP_DEPTH + 2) + &")".repeat(MAX_GROUP_DEPTH + 2);
        assert!(Regex::new(&deep).is_err());
        let ok = "(".repeat(4) + "a" + &")".repeat(4);
        assert!(Regex::new(&ok).is_ok());
    }

    #[test]
    fn pattern_text_is_retained() {
        let r = Regex::new("^ab$").expect("compile");
        assert_eq!(r.pattern(), "^ab$");
    }

    #[test]
    fn realistic_id_patterns() {
        assert!(m("^[a-z][a-z0-9_]*$", "jazz_standard"));
        assert!(!m("^[a-z][a-z0-9_]*$", "9lives"));
        assert!(m(
            "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
            "0123456789abcdef0123456789abcdef"
                .chars()
                .collect::<String>()
                .chars()
                .enumerate()
                .fold(String::new(), |mut acc, (i, c)| {
                    if matches!(i, 8 | 12 | 16 | 20) {
                        acc.push('-');
                    }
                    acc.push(c);
                    acc
                })
                .as_str()
        ));
        assert!(m("^\\d+/\\d+$", "4/4"));
    }

    #[test]
    fn display_impl_shows_message() {
        let e = Regex::new("(").expect_err("compile");
        assert_eq!(format!("{e}"), e.message);
    }
}
