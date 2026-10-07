//! `@streamparser/json` 0.0.21's tokenizer and token parser for upstream's configuration
//! (string input, `paths`, `keepStack: false`), fed as `parseJson` feeds them: `js::Stream`
//! chunks, a BOM `trimStart` on the first one. Tokenizer errors name a byte position within
//! the current chunk (F-30).

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
    /// Members in first-seen order; a repeated name replaces the value, as assignment does.
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
/// `@streamparser/json` selector syntax without the `$`: `&["a", "*"]` is `$.a.*`.
/// Values elsewhere are checked but never built.
pub fn parse(
    input: impl Read,
    paths: &[&[&str]],
    on_value: impl FnMut(Key, Value) -> ControlFlow<()>,
) -> io::Result<End> {
    let mut parser = Parser {
        paths,
        on_value,
        state: State::BomOrStart,
        string: String::new(),
        number: String::new(),
        unicode: 0,
        high_surrogate: None,
        expect: Expect::Value,
        container: None,
        key: None,
        value: None,
        stack: Vec::new(),
    };
    let mut stream = js::Stream::new(input, js::CHUNK);
    let mut text = String::new();
    let mut first = true;
    while stream.next(&mut text)? {
        let mut chunk = text.as_str();
        if std::mem::take(&mut first) && chunk.starts_with('\u{feff}') {
            chunk = js::trim_start(chunk); // upstream's removeBOM
        }
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
            members.push((name, value));
        }
        ControlFlow::Continue(())
    });
    assert!(
        matches!(end, Ok(End::Done)),
        "vendored schemas are JSON objects"
    );
    Value::Object(members)
}

/// `Err` ends parsing.
type Step = Result<(), End>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    BomOrStart,
    Start,
    True1,
    True2,
    True3,
    False1,
    False2,
    False3,
    False4,
    Null1,
    Null2,
    Null3,
    StringDefault,
    StringAfterBackslash,
    StringUnicodeDigit1,
    StringUnicodeDigit2,
    StringUnicodeDigit3,
    StringUnicodeDigit4,
    NumberAfterInitialMinus,
    NumberAfterInitialZero,
    NumberAfterInitialNonZero,
    NumberAfterFullStop,
    NumberAfterDecimal,
    NumberAfterE,
    NumberAfterEAndSign,
    NumberAfterEAndDigit,
}

impl State {
    fn name(self) -> &'static str {
        [
            "BOM_OR_START",
            "START",
            "TRUE1",
            "TRUE2",
            "TRUE3",
            "FALSE1",
            "FALSE2",
            "FALSE3",
            "FALSE4",
            "NULL1",
            "NULL2",
            "NULL3",
            "STRING_DEFAULT",
            "STRING_AFTER_BACKSLASH",
            "STRING_UNICODE_DIGIT_1",
            "STRING_UNICODE_DIGIT_2",
            "STRING_UNICODE_DIGIT_3",
            "STRING_UNICODE_DIGIT_4",
            "NUMBER_AFTER_INITIAL_MINUS",
            "NUMBER_AFTER_INITIAL_ZERO",
            "NUMBER_AFTER_INITIAL_NON_ZERO",
            "NUMBER_AFTER_FULL_STOP",
            "NUMBER_AFTER_DECIMAL",
            "NUMBER_AFTER_E",
            "NUMBER_AFTER_E_AND_SIGN",
            "NUMBER_AFTER_E_AND_DIGIT",
        ][self as usize]
    }
}

/// The token parser's state.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    Value,
    Key,
    Colon,
    Comma,
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

#[derive(Clone, Copy, PartialEq)]
enum Container {
    Object,
    Array,
}

/// An open container's parent, as `push` saves it.
struct Frame {
    key: Option<Key>,
    value: Option<Value>,
    container: Option<Container>,
    /// Whether the container this frame opened is emitted when it closes.
    emit: bool,
}

struct Parser<'p, F> {
    paths: &'p [&'p [&'p str]],
    on_value: F,
    // tokenizer
    state: State,
    string: String,
    number: String,
    unicode: u32,
    /// Survives the string it was read in, as upstream's does.
    high_surrogate: Option<u32>,
    // token parser
    expect: Expect,
    container: Option<Container>,
    key: Option<Key>,
    /// The open container, built only while it or an ancestor is emitted: elsewhere
    /// `keepStack: false` deletes each member once it is complete.
    value: Option<Value>,
    stack: Vec<Frame>,
}

impl<F: FnMut(Key, Value) -> ControlFlow<()>> Parser<'_, F> {
    /// The tokenizer's `write` loop over one chunk.
    fn write(&mut self, chunk: &str) -> Step {
        use State::*;
        let b = chunk.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let n = b[i];
            match self.state {
                BomOrStart | Start => match n {
                    b' ' | b'\n' | b'\r' | b'\t' => {}
                    b'{' => self.token(Token::LeftBrace)?,
                    b'}' => self.token(Token::RightBrace)?,
                    b'[' => self.token(Token::LeftBracket)?,
                    b']' => self.token(Token::RightBracket)?,
                    b':' => self.token(Token::Colon)?,
                    b',' => self.token(Token::Comma)?,
                    b't' => self.state = True1,
                    b'f' => self.state = False1,
                    b'n' => self.state = Null1,
                    b'"' => {
                        self.string.clear();
                        self.state = StringDefault;
                    }
                    b'0'..=b'9' | b'-' => {
                        self.number.clear();
                        self.number.push(n as char);
                        self.state = match n {
                            b'0' => NumberAfterInitialZero,
                            b'-' => NumberAfterInitialMinus,
                            _ => NumberAfterInitialNonZero,
                        };
                    }
                    _ => return Err(self.unexpected(n, i)),
                },
                StringDefault => match n {
                    b'"' => {
                        self.state = Start;
                        let s = std::mem::take(&mut self.string);
                        self.token(Token::Value(Value::String(s)))?;
                    }
                    b'\\' => self.state = StringAfterBackslash,
                    0x80.. => {
                        // The chunk is a JS string re-encoded, so every sequence is whole.
                        let c = chunk[i..].chars().next().unwrap();
                        self.string.push(c);
                        i += c.len_utf8() - 1;
                    }
                    0x20.. => self.string.push(n as char),
                    _ => return Err(self.unexpected(n, i)),
                },
                StringAfterBackslash => match n {
                    b'u' => {
                        self.unicode = 0;
                        self.state = StringUnicodeDigit1;
                    }
                    _ => {
                        let c = match n {
                            b'"' | b'\\' | b'/' => n as char,
                            b'b' => '\u{8}',
                            b'f' => '\u{c}',
                            b'n' => '\n',
                            b'r' => '\r',
                            b't' => '\t',
                            _ => return Err(self.unexpected(n, i)),
                        };
                        self.string.push(c);
                        self.state = StringDefault;
                    }
                },
                StringUnicodeDigit1 | StringUnicodeDigit2 | StringUnicodeDigit3
                | StringUnicodeDigit4 => {
                    let Some(digit) = (n as char).to_digit(16) else {
                        return Err(self.unexpected(n, i));
                    };
                    self.unicode = self.unicode * 16 + digit;
                    self.state = match self.state {
                        StringUnicodeDigit1 => StringUnicodeDigit2,
                        StringUnicodeDigit2 => StringUnicodeDigit3,
                        StringUnicodeDigit3 => StringUnicodeDigit4,
                        _ => {
                            self.push_unicode();
                            StringDefault
                        }
                    };
                }
                NumberAfterInitialMinus => match n {
                    b'0' => self.digit(n, NumberAfterInitialZero),
                    b'1'..=b'9' => self.digit(n, NumberAfterInitialNonZero),
                    _ => return Err(self.unexpected(n, i)),
                },
                NumberAfterFullStop => match n {
                    b'0'..=b'9' => self.digit(n, NumberAfterDecimal),
                    _ => return Err(self.unexpected(n, i)),
                },
                NumberAfterE | NumberAfterEAndSign => match n {
                    b'+' | b'-' if self.state == NumberAfterE => self.digit(n, NumberAfterEAndSign),
                    b'0'..=b'9' => self.digit(n, NumberAfterEAndDigit),
                    _ => return Err(self.unexpected(n, i)),
                },
                NumberAfterInitialZero
                | NumberAfterInitialNonZero
                | NumberAfterDecimal
                | NumberAfterEAndDigit => match n {
                    b'0'..=b'9' if self.state != NumberAfterInitialZero => {
                        self.number.push(n as char)
                    }
                    b'.' if matches!(
                        self.state,
                        NumberAfterInitialZero | NumberAfterInitialNonZero
                    ) =>
                    {
                        self.digit(n, NumberAfterFullStop)
                    }
                    b'e' | b'E' if self.state != NumberAfterEAndDigit => {
                        self.digit(n, NumberAfterE)
                    }
                    _ => {
                        // The number ended before this byte, which is read again in START.
                        self.state = Start;
                        self.emit_number()?;
                        continue;
                    }
                },
                True1 | True2 | True3 | False1 | False2 | False3 | False4 | Null1 | Null2
                | Null3 => {
                    let (expected, next) = match self.state {
                        True1 => (b'r', True2),
                        True2 => (b'u', True3),
                        False1 => (b'a', False2),
                        False2 => (b'l', False3),
                        False3 => (b's', False4),
                        Null1 => (b'u', Null2),
                        Null2 => (b'l', Null3),
                        _ => (if self.state == Null3 { b'l' } else { b'e' }, Start),
                    };
                    if n != expected {
                        return Err(self.unexpected(n, i));
                    }
                    let last = self.state;
                    self.state = next;
                    if next == Start {
                        let value = match last {
                            True3 => Value::Bool(true),
                            False4 => Value::Bool(false),
                            _ => Value::Null,
                        };
                        self.token(Token::Value(value))?;
                    }
                }
            }
            i += 1;
        }
        Ok(())
    }

    fn digit(&mut self, n: u8, next: State) {
        self.number.push(n as char);
        self.state = next;
    }

    fn unexpected(&self, n: u8, i: usize) -> End {
        // The byte read as Latin-1: `String.fromCharCode(n)`.
        let state = self.state.name();
        End::Invalid(format!(
            "Unexpected \"{}\" at position \"{i}\" in state {state}",
            n as char
        ))
    }

    /// A completed `\uXXXX`. Surrogates pair across escapes; a lone one becomes U+FFFD, and
    /// an escape that fails to complete a pending high surrogate is dropped with it.
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
        self.string.push(c.unwrap_or('\u{fffd}'));
    }

    fn emit_number(&mut self) -> Step {
        let n = self
            .number
            .parse()
            .expect("the tokenizer only builds JSON numbers");
        self.token(Token::Value(Value::Number(n)))
    }

    /// The tokenizer's `end`.
    fn end(&mut self) -> End {
        use State::*;
        let step = match self.state {
            NumberAfterInitialZero
            | NumberAfterInitialNonZero
            | NumberAfterDecimal
            | NumberAfterEAndDigit => self.emit_number().and_then(|()| self.end_tokens()),
            BomOrStart | Start => self.end_tokens(),
            state => Err(End::Invalid(format!(
                "Tokenizer ended in the middle of a token (state: {}). Either not all the data was received or the data was invalid.",
                state.name()
            ))),
        };
        step.expect_err("the end of input always ends parsing")
    }

    /// The token parser's `end`.
    fn end_tokens(&self) -> Step {
        if self.expect != Expect::Value || !self.stack.is_empty() {
            return Err(End::Invalid(format!(
                "Parser ended in mid-parsing (state: {}). Either not all the data was received or the data was invalid.",
                format!("{:?}", self.expect).to_uppercase()
            )));
        }
        Err(End::Done)
    }

    /// The token parser's `write`.
    fn token(&mut self, token: Token) -> Step {
        let array = self.container == Some(Container::Array);
        match (self.expect, token) {
            (Expect::Value, Token::Value(value)) => {
                if self.container.is_some() {
                    self.expect = Expect::Comma;
                }
                let emit = self.should_emit();
                self.emit(Some(value), emit)
            }
            (Expect::Value, token @ (Token::LeftBrace | Token::LeftBracket)) => {
                let emit = self.should_emit();
                let build = emit || self.value.is_some();
                self.stack.push(Frame {
                    key: self.key.take(),
                    value: self.value.take(),
                    container: self.container,
                    emit,
                });
                if matches!(token, Token::LeftBracket) {
                    self.container = Some(Container::Array);
                    self.key = Some(Key::Index(0));
                    self.value = build.then(|| Value::Array(Vec::new()));
                } else {
                    self.container = Some(Container::Object);
                    self.expect = Expect::Key;
                    self.value = build.then(|| Value::Object(Vec::new()));
                }
                Ok(())
            }
            // `this.value.length === 0`, which only holds right after `[`.
            (Expect::Value, Token::RightBracket) if array && self.key == Some(Key::Index(0)) => {
                self.pop()
            }
            (Expect::Key, Token::Value(Value::String(name))) => {
                self.key = Some(Key::Name(name));
                self.expect = Expect::Colon;
                Ok(())
            }
            // `Object.keys(this.value).length === 0`: members not kept count as gone.
            (Expect::Key, Token::RightBrace)
                if self
                    .value
                    .as_ref()
                    .is_none_or(|v| matches!(v, Value::Object(m) if m.is_empty())) =>
            {
                self.pop()
            }
            (Expect::Colon, Token::Colon) => {
                self.expect = Expect::Value;
                Ok(())
            }
            (Expect::Comma, Token::Comma) if self.container.is_some() => {
                if let Some(Key::Index(i)) = &mut self.key {
                    *i += 1;
                    self.expect = Expect::Value;
                } else {
                    self.expect = Expect::Key;
                }
                Ok(())
            }
            (Expect::Comma, Token::RightBrace) if self.container == Some(Container::Object) => {
                self.pop()
            }
            (Expect::Comma, Token::RightBracket) if array => self.pop(),
            (expect, token) => Err(End::Invalid(format!(
                "{} in state {}",
                token.describe(),
                format!("{expect:?}").to_uppercase()
            ))),
        }
    }

    fn pop(&mut self) -> Step {
        let value = self.value.take();
        let frame = self.stack.pop().expect("a container is open");
        self.key = frame.key;
        self.value = frame.value;
        self.container = frame.container;
        self.expect = if self.container.is_some() {
            Expect::Comma
        } else {
            Expect::Value
        };
        self.emit(value, frame.emit)
    }

    /// A completed value: handed to `on_value` if emitted, else kept in a built parent.
    /// With the paths used here an emitted value never has a built parent.
    fn emit(&mut self, value: Option<Value>, emit: bool) -> Step {
        let key = self.key.clone();
        if emit {
            let (Some(key), Some(value)) = (key, value) else {
                unreachable!("emitted values are built and have keys")
            };
            if (self.on_value)(key, value).is_break() {
                return Err(End::Stopped);
            }
        } else if let (Some(parent), Some(value)) = (&mut self.value, value) {
            match (parent, key) {
                (Value::Array(items), _) => items.push(value),
                (Value::Object(members), Some(Key::Name(name))) => {
                    match members.iter_mut().find(|(k, _)| *k == name) {
                        Some(member) => member.1 = value,
                        None => members.push((name, value)),
                    }
                }
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
        self.paths.iter().any(|path| {
            let Some((last, parents)) = path.split_last() else {
                return false;
            };
            path.len() == self.stack.len()
                && (parents.iter().zip(&self.stack[1..]))
                    .all(|(s, f)| *s == "*" || matches!(&f.key, Some(Key::Name(k)) if k == s))
                && (*last == "*"
                    || match &self.key {
                        Some(Key::Name(k)) => k == last,
                        Some(Key::Index(i)) => i.to_string() == *last,
                        None => false,
                    })
        })
    }
}
