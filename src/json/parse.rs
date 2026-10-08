//! `@streamparser/json` 0.0.21's tokenizer and token parser for upstream's configuration
//! (string input, `paths`, `keepStack: false`), fed as `parseJson` feeds them: `js::Stream`
//! chunks, `removeBOM` on the first one. Tokenizer errors name a byte position within the
//! current chunk.

use crate::js;
use std::io::{self, Read};
use std::ops::ControlFlow;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    /// Members in first-seen order (see `set`).
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Assigns an object member: a repeated name replaces the value in place, as JS does.
pub fn set(members: &mut Vec<(String, Value)>, name: String, value: Value) {
    match members.iter_mut().find(|(k, _)| *k == name) {
        Some(member) => member.1 = value,
        None => members.push((name, value)),
    }
}

/// Where a value sits in its parent.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Name(String),
    Index(usize),
}

/// How parsing finished.
pub enum End {
    /// The top-level value is complete (`onEnd`); the rest of the input is never read.
    Done,
    /// `on_value` asked to stop.
    Stopped,
    /// The parser's error message (`onError`).
    Invalid(String),
}

/// Parses `input`, handing `on_value` each value whose path matches one of `paths`, in
/// `@streamparser/json` selector syntax without the `$` (`&["a", "*"]` is `$.a.*`), with
/// names or `*` as selectors. Values elsewhere are checked but never built.
pub fn parse(
    input: impl Read,
    paths: &[&[&str]],
    on_value: impl FnMut(Key, &Value) -> ControlFlow<()>,
) -> io::Result<End> {
    let mut parser = Parser {
        paths,
        on_value,
        state: State::BOM_OR_START,
        buffer: String::new(),
        unicode: 0,
        high_surrogate: None,
        expect: Expect::VALUE,
        key: None,
        value: None,
        stack: Vec::new(),
        spare: Spare::default(),
    };
    let mut stream = js::Stream::new(input, js::CHUNK);
    let mut text = String::new();
    let mut first = true;
    while stream.next(&mut text)? {
        let chunk = if std::mem::take(&mut first) { crate::remove_bom(&text) } else { &text };
        if let Err(end) = parser.write(chunk) {
            return Ok(end);
        }
        text.clear();
    }
    Ok(parser.end())
}

/// A whole document, for the vendored schemas: an object read as its `$.*` members.
pub fn document(text: &str) -> Value {
    let mut members = Vec::new();
    let end = parse(text.as_bytes(), &[&["*"]], |key, value| {
        if let Key::Name(name) = key {
            set(&mut members, name, value.clone());
        }
        ControlFlow::Continue(())
    });
    assert!(matches!(end, Ok(End::Done)), "vendored schemas are JSON objects");
    Value::Object(members)
}

/// `Err` ends parsing.
type Step = Result<(), End>;

/// The tokenizer's state, named as upstream's are in its error texts.
#[allow(
    non_camel_case_types,
    clippy::upper_case_acronyms,
    reason = "upstream's names, printed in its error texts"
)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    BOM_OR_START,
    START,
    /// `true`, `false` or `null` with this many bytes read: upstream's `TRUE1` to `NULL3`.
    LITERAL(&'static str, usize),
    STRING_DEFAULT,
    STRING_AFTER_BACKSLASH,
    /// Expecting this hex digit, 1 to 4, of a `\u` escape.
    STRING_UNICODE_DIGIT(u32),
    NUMBER_AFTER_INITIAL_MINUS,
    NUMBER_AFTER_INITIAL_ZERO,
    NUMBER_AFTER_INITIAL_NON_ZERO,
    NUMBER_AFTER_FULL_STOP,
    NUMBER_AFTER_DECIMAL,
    NUMBER_AFTER_E,
    NUMBER_AFTER_E_AND_SIGN,
    NUMBER_AFTER_E_AND_DIGIT,
}

impl State {
    fn name(self) -> String {
        match self {
            State::LITERAL(word, read) => format!("{}{read}", word.to_ascii_uppercase()),
            State::STRING_UNICODE_DIGIT(n) => format!("STRING_UNICODE_DIGIT_{n}"),
            state => format!("{state:?}"),
        }
    }
}

/// The token parser's state.
#[allow(
    non_camel_case_types,
    clippy::upper_case_acronyms,
    reason = "upstream's names, printed in its error texts"
)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    VALUE,
    KEY,
    COLON,
    COMMA,
}

enum Token {
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Colon,
    Comma,
    Value(Value),
}

impl Token {
    /// `Unexpected ${TokenType[token]} (${JSON.stringify(value)})`.
    fn describe(&self) -> String {
        let (name, json) = match self {
            Token::LeftBrace => ("LEFT_BRACE", "\"{\"".into()),
            Token::RightBrace => ("RIGHT_BRACE", "\"}\"".into()),
            Token::LeftBracket => ("LEFT_BRACKET", "\"[\"".into()),
            Token::RightBracket => ("RIGHT_BRACKET", "\"]\"".into()),
            Token::Colon => ("COLON", "\":\"".into()),
            Token::Comma => ("COMMA", "\",\"".into()),
            Token::Value(Value::Bool(true)) => ("TRUE", "true".into()),
            Token::Value(Value::Bool(false)) => ("FALSE", "false".into()),
            Token::Value(Value::String(s)) => {
                let mut json = String::new();
                crate::push_json_str(&mut json, s);
                ("STRING", json)
            }
            Token::Value(Value::Number(n)) if n.is_finite() => ("NUMBER", js::number_string(*n)),
            Token::Value(Value::Number(_)) => ("NUMBER", "null".into()),
            Token::Value(_) => ("NULL", "null".into()),
        };
        format!("Unexpected {name} ({json})")
    }
}

/// The buffers of values `on_value` is done with, for the values built next: without them
/// every string, array and object of every charge item is allocated and freed. Strings are
/// emptied when the tokenizer takes them.
#[derive(Default)]
struct Spare {
    strings: Vec<String>,
    arrays: Vec<Vec<Value>>,
    objects: Vec<Vec<(String, Value)>>,
}

impl Spare {
    fn keep(&mut self, value: Value) {
        match value {
            Value::String(s) => self.strings.push(s),
            Value::Array(mut items) => {
                items.drain(..).for_each(|v| self.keep(v));
                self.arrays.push(items);
            }
            Value::Object(mut members) => {
                for (name, v) in members.drain(..) {
                    self.strings.push(name);
                    self.keep(v);
                }
                self.objects.push(members);
            }
            _ => {}
        }
    }
}

/// An open container's parent, as `push` saves it.
struct Frame {
    key: Option<Key>,
    value: Option<Value>,
    /// Whether the container this frame opened is emitted when it closes.
    emit: bool,
}

struct Parser<'p, F> {
    paths: &'p [&'p [&'p str]],
    on_value: F,
    // tokenizer
    state: State,
    /// The string or number being read.
    buffer: String,
    unicode: u32,
    /// Survives the string it was read in, as upstream's does.
    high_surrogate: Option<u32>,
    // token parser
    expect: Expect,
    /// An array element's index, an object member's name, or none: what the open container
    /// is, too.
    key: Option<Key>,
    /// The open container, built only while it or an ancestor is emitted: elsewhere
    /// `keepStack: false` deletes each member once it is complete.
    value: Option<Value>,
    stack: Vec<Frame>,
    spare: Spare,
}

impl<F: FnMut(Key, &Value) -> ControlFlow<()>> Parser<'_, F> {
    /// The tokenizer's `write` loop over one chunk.
    #[expect(clippy::too_many_lines, reason = "one state machine, as upstream's")]
    fn write(&mut self, chunk: &str) -> Step {
        #[allow(clippy::enum_glob_use, reason = "states read as upstream names them")]
        use State::*;
        let b = chunk.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let n = b[i];
            match self.state {
                BOM_OR_START | START => match n {
                    b' ' | b'\n' | b'\r' | b'\t' => {}
                    b'{' => self.token(Token::LeftBrace)?,
                    b'}' => self.token(Token::RightBrace)?,
                    b'[' => self.token(Token::LeftBracket)?,
                    b']' => self.token(Token::RightBracket)?,
                    b':' => self.token(Token::Colon)?,
                    b',' => self.token(Token::Comma)?,
                    b't' => self.state = LITERAL("true", 1),
                    b'f' => self.state = LITERAL("false", 1),
                    b'n' => self.state = LITERAL("null", 1),
                    b'"' => {
                        self.buffer.clear();
                        self.state = STRING_DEFAULT;
                    }
                    b'0'..=b'9' | b'-' => {
                        self.buffer.clear();
                        self.buffer.push(n as char);
                        self.state = match n {
                            b'0' => NUMBER_AFTER_INITIAL_ZERO,
                            b'-' => NUMBER_AFTER_INITIAL_MINUS,
                            _ => NUMBER_AFTER_INITIAL_NON_ZERO,
                        };
                    }
                    _ => return Err(self.unexpected(n, i)),
                },
                LITERAL(word, read) => {
                    if n != word.as_bytes()[read] {
                        return Err(self.unexpected(n, i));
                    }
                    if read + 1 < word.len() {
                        self.state = LITERAL(word, read + 1);
                    } else {
                        self.state = START;
                        let value = match word {
                            "true" => Value::Bool(true),
                            "false" => Value::Bool(false),
                            _ => Value::Null,
                        };
                        self.token(Token::Value(value))?;
                    }
                }
                STRING_DEFAULT => match n {
                    b'"' => {
                        self.state = START;
                        let spare = self.spare.strings.pop().unwrap_or_default();
                        let s = Value::String(std::mem::replace(&mut self.buffer, spare));
                        self.token(Token::Value(s))?;
                    }
                    b'\\' => self.state = STRING_AFTER_BACKSLASH,
                    0x20.. => {
                        // A run of plain bytes, which ends at ASCII, so at a char boundary.
                        let run = (b[i..].iter())
                            .position(|&c| c == b'"' || c == b'\\' || c < 0x20)
                            .unwrap_or(b.len() - i);
                        self.buffer.push_str(&chunk[i..i + run]);
                        i += run;
                        continue;
                    }
                    _ => return Err(self.unexpected(n, i)),
                },
                STRING_AFTER_BACKSLASH if n == b'u' => {
                    self.unicode = 0;
                    self.state = STRING_UNICODE_DIGIT(1);
                }
                STRING_AFTER_BACKSLASH => {
                    let c = match n {
                        b'"' | b'\\' | b'/' => n as char,
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        _ => return Err(self.unexpected(n, i)),
                    };
                    self.buffer.push(c);
                    self.state = STRING_DEFAULT;
                }
                STRING_UNICODE_DIGIT(k) => {
                    let Some(digit) = (n as char).to_digit(16) else {
                        return Err(self.unexpected(n, i));
                    };
                    self.unicode = self.unicode * 16 + digit;
                    self.state = if k < 4 {
                        STRING_UNICODE_DIGIT(k + 1)
                    } else {
                        self.push_unicode();
                        STRING_DEFAULT
                    };
                }
                NUMBER_AFTER_INITIAL_MINUS => match n {
                    b'0' => self.digit(n, NUMBER_AFTER_INITIAL_ZERO),
                    b'1'..=b'9' => self.digit(n, NUMBER_AFTER_INITIAL_NON_ZERO),
                    _ => return Err(self.unexpected(n, i)),
                },
                NUMBER_AFTER_FULL_STOP => match n {
                    b'0'..=b'9' => self.digit(n, NUMBER_AFTER_DECIMAL),
                    _ => return Err(self.unexpected(n, i)),
                },
                NUMBER_AFTER_E | NUMBER_AFTER_E_AND_SIGN => match n {
                    b'+' | b'-' if self.state == NUMBER_AFTER_E => {
                        self.digit(n, NUMBER_AFTER_E_AND_SIGN);
                    }
                    b'0'..=b'9' => self.digit(n, NUMBER_AFTER_E_AND_DIGIT),
                    _ => return Err(self.unexpected(n, i)),
                },
                NUMBER_AFTER_INITIAL_ZERO
                | NUMBER_AFTER_INITIAL_NON_ZERO
                | NUMBER_AFTER_DECIMAL
                | NUMBER_AFTER_E_AND_DIGIT => match n {
                    b'0'..=b'9' if self.state != NUMBER_AFTER_INITIAL_ZERO => {
                        self.buffer.push(n as char);
                    }
                    b'.' if matches!(
                        self.state,
                        NUMBER_AFTER_INITIAL_ZERO | NUMBER_AFTER_INITIAL_NON_ZERO
                    ) =>
                    {
                        self.digit(n, NUMBER_AFTER_FULL_STOP);
                    }
                    b'e' | b'E' if self.state != NUMBER_AFTER_E_AND_DIGIT => {
                        self.digit(n, NUMBER_AFTER_E);
                    }
                    _ => {
                        // The number ended before this byte, which is read again in START.
                        self.state = START;
                        self.emit_number()?;
                        continue;
                    }
                },
            }
            i += 1;
        }
        Ok(())
    }

    fn digit(&mut self, n: u8, next: State) {
        self.buffer.push(n as char);
        self.state = next;
    }

    fn unexpected(&self, n: u8, i: usize) -> End {
        // The byte read as Latin-1: `String.fromCharCode(n)`.
        let state = self.state.name();
        End::Invalid(format!("Unexpected \"{}\" at position \"{i}\" in state {state}", n as char))
    }

    /// A completed `\u` escape. Surrogates pair across escapes; a lone one becomes U+FFFD,
    /// and an escape that fails to complete a pending high surrogate is dropped with it.
    fn push_unicode(&mut self) {
        let code = self.unicode;
        let c = match self.high_surrogate.take() {
            None if (0xd800..=0xdbff).contains(&code) => {
                self.high_surrogate = Some(code);
                return;
            }
            None => char::from_u32(code),
            Some(high) if (0xdc00..=0xdfff).contains(&code) => {
                char::from_u32(0x10000 + ((high - 0xd800) << 10) + (code - 0xdc00))
            }
            Some(_) => None,
        };
        self.buffer.push(c.unwrap_or('\u{fffd}'));
    }

    fn emit_number(&mut self) -> Step {
        let n = (self.buffer.parse()).expect("the tokenizer only builds JSON numbers");
        self.token(Token::Value(Value::Number(n)))
    }

    /// The tokenizer's `end`.
    fn end(&mut self) -> End {
        #[allow(clippy::enum_glob_use, reason = "states read as upstream names them")]
        use State::*;
        match self.state {
            NUMBER_AFTER_INITIAL_ZERO
            | NUMBER_AFTER_INITIAL_NON_ZERO
            | NUMBER_AFTER_DECIMAL
            | NUMBER_AFTER_E_AND_DIGIT => {
                (self.emit_number().err()).unwrap_or_else(|| self.end_tokens())
            }
            BOM_OR_START | START => self.end_tokens(),
            state => End::Invalid(format!(
                "Tokenizer ended in the middle of a token (state: {}). Either not all the data was received or the data was invalid.",
                state.name()
            )),
        }
    }

    /// The token parser's `end`.
    fn end_tokens(&self) -> End {
        if self.expect != Expect::VALUE || !self.stack.is_empty() {
            return End::Invalid(format!(
                "Parser ended in mid-parsing (state: {:?}). Either not all the data was received or the data was invalid.",
                self.expect
            ));
        }
        End::Done
    }

    /// The token parser's `write`. At the top level a value ends parsing before `expect`
    /// is read again, so it is set as inside a container.
    fn token(&mut self, token: Token) -> Step {
        let array = matches!(self.key, Some(Key::Index(_)));
        match (self.expect, token) {
            (Expect::VALUE, Token::Value(value)) => {
                self.expect = Expect::COMMA;
                let emit = self.should_emit();
                self.emit(Some(value), emit)
            }
            (Expect::VALUE, token @ (Token::LeftBrace | Token::LeftBracket)) => {
                let emit = self.should_emit();
                let build = emit || self.value.is_some();
                self.stack.push(Frame { key: self.key.take(), value: self.value.take(), emit });
                if matches!(token, Token::LeftBracket) {
                    self.key = Some(Key::Index(0));
                    let spare = &mut self.spare.arrays;
                    self.value = build.then(|| Value::Array(spare.pop().unwrap_or_default()));
                } else {
                    self.expect = Expect::KEY;
                    let spare = &mut self.spare.objects;
                    self.value = build.then(|| Value::Object(spare.pop().unwrap_or_default()));
                }
                Ok(())
            }
            // `this.value.length === 0`, which only holds right after `[`.
            (Expect::VALUE, Token::RightBracket) if self.key == Some(Key::Index(0)) => self.pop(),
            (Expect::KEY, Token::Value(Value::String(name))) => {
                self.key = Some(Key::Name(name));
                self.expect = Expect::COLON;
                Ok(())
            }
            // `Object.keys(this.value).length === 0`: members not kept count as gone.
            (Expect::KEY, Token::RightBrace)
                if self
                    .value
                    .as_ref()
                    .is_none_or(|v| matches!(v, Value::Object(m) if m.is_empty())) =>
            {
                self.pop()
            }
            (Expect::COLON, Token::Colon) => {
                self.expect = Expect::VALUE;
                Ok(())
            }
            (Expect::COMMA, Token::Comma) => {
                if let Some(Key::Index(i)) = &mut self.key {
                    *i += 1;
                    self.expect = Expect::VALUE;
                } else {
                    self.expect = Expect::KEY;
                }
                Ok(())
            }
            (Expect::COMMA, Token::RightBrace) if !array => self.pop(),
            (Expect::COMMA, Token::RightBracket) if array => self.pop(),
            (expect, token) => {
                Err(End::Invalid(format!("{} in state {expect:?}", token.describe())))
            }
        }
    }

    fn pop(&mut self) -> Step {
        let value = self.value.take();
        let frame = self.stack.pop().expect("a container is open");
        self.key = frame.key;
        self.value = frame.value;
        self.expect = Expect::COMMA;
        self.emit(value, frame.emit)
    }

    /// A completed value: handed to `on_value` if emitted, else kept in a built parent.
    /// With the paths used here an emitted value never has a built parent.
    fn emit(&mut self, value: Option<Value>, emit: bool) -> Step {
        // An array keeps its index for the next element; a member's name is not read again.
        let key = match self.key {
            Some(Key::Index(i)) => Some(Key::Index(i)),
            _ => self.key.take(),
        };
        if emit {
            let (Some(key), Some(value)) = (key, value) else {
                unreachable!("emitted values are built and have keys")
            };
            if (self.on_value)(key, &value).is_break() {
                return Err(End::Stopped);
            }
            self.spare.keep(value);
        } else if let (Some(parent), Some(value)) = (&mut self.value, value) {
            match (parent, key) {
                (Value::Array(items), _) => items.push(value),
                (Value::Object(members), Some(Key::Name(name))) => set(members, name, value),
                _ => unreachable!("object members have names"),
            }
        }
        if self.stack.is_empty() {
            return Err(End::Done);
        }
        Ok(())
    }

    /// Whether the value at the current key matches a path.
    fn should_emit(&self) -> bool {
        let keys = || (self.stack.iter().skip(1).map(|f| &f.key)).chain([&self.key]);
        self.paths.iter().any(|path| {
            path.len() == self.stack.len()
                && (path.iter().zip(keys()))
                    .all(|(s, k)| *s == "*" || matches!(k, Some(Key::Name(n)) if n == s))
        })
    }
}
