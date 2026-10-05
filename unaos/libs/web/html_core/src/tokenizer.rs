//! The WHATWG HTML tokenizer — §13.2.5 "Tokenization", every state, written from the specification.
//!
//! Input is a `&str` that has already been through §13.2.3.5 "Preprocessing the input stream" (CR and CRLF
//! normalised to LF — [`preprocess`]). The tokenizer is pulled one token at a time ([`Tokenizer::next_token`]);
//! the tree builder flips [`Tokenizer::state`] (RCDATA/RAWTEXT/script data/PLAINTEXT) after a start tag and keeps
//! [`Tokenizer::allow_cdata`] equal to "there is an adjusted current node and it is not an element in the HTML
//! namespace" (§13.2.5.42).
//!
//! Parse errors are recognised where the spec names them but are not reported (the tree does not depend on
//! them); `errors` counts them.
//!
//! Processing instructions (the 2026 spec's §13.2.5.80–84) are behind [`TokenizerOpts::processing_instructions`],
//! off by default: shipping Chromium and the pinned tree-construction corpus still treat `<?` as a bogus comment.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use crate::entities::ENTITIES;

/// A start or end tag attribute as the tokenizer produced it (name lowercased, value decoded).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    pub name: String,
    pub value: String,
}

/// A start or end tag token.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Tag {
    pub name: String,
    pub attrs: Vec<Attr>,
    pub self_closing: bool,
}

impl Tag {
    pub fn new(name: &str) -> Tag {
        Tag { name: String::from(name), attrs: Vec::new(), self_closing: false }
    }
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.name == name).map(|a| a.value.as_str())
    }
}

/// A DOCTYPE token. `None` is the spec's "missing", distinct from the empty string.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Doctype {
    pub name: Option<String>,
    pub public_id: Option<String>,
    pub system_id: Option<String>,
    pub force_quirks: bool,
}

/// The tokenizer's output (§13.2.5 intro).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Doctype(Doctype),
    StartTag(Tag),
    EndTag(Tag),
    Comment(String),
    ProcessingInstruction { target: String, data: String },
    Character(char),
    Eof,
}

/// The tokenizer states (§13.2.5.1–13.2.5.80). `NumericCharRefEnd` is folded into a function because it
/// consumes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Data,
    Rcdata,
    Rawtext,
    ScriptData,
    Plaintext,
    TagOpen,
    EndTagOpen,
    TagName,
    RcdataLessThan,
    RcdataEndTagOpen,
    RcdataEndTagName,
    RawtextLessThan,
    RawtextEndTagOpen,
    RawtextEndTagName,
    ScriptDataLessThan,
    ScriptDataEndTagOpen,
    ScriptDataEndTagName,
    ScriptDataEscapeStart,
    ScriptDataEscapeStartDash,
    ScriptDataEscaped,
    ScriptDataEscapedDash,
    ScriptDataEscapedDashDash,
    ScriptDataEscapedLessThan,
    ScriptDataEscapedEndTagOpen,
    ScriptDataEscapedEndTagName,
    ScriptDataDoubleEscapeStart,
    ScriptDataDoubleEscaped,
    ScriptDataDoubleEscapedDash,
    ScriptDataDoubleEscapedDashDash,
    ScriptDataDoubleEscapedLessThan,
    ScriptDataDoubleEscapeEnd,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    BogusComment,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentLessThan,
    CommentLessThanBang,
    CommentLessThanBangDash,
    CommentLessThanBangDashDash,
    CommentEndDash,
    CommentEnd,
    CommentEndBang,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
    AfterDoctypeName,
    AfterDoctypePublicKeyword,
    BeforeDoctypePublicIdentifier,
    DoctypePublicIdentifierDoubleQuoted,
    DoctypePublicIdentifierSingleQuoted,
    AfterDoctypePublicIdentifier,
    BetweenDoctypePublicAndSystemIdentifiers,
    AfterDoctypeSystemKeyword,
    BeforeDoctypeSystemIdentifier,
    DoctypeSystemIdentifierDoubleQuoted,
    DoctypeSystemIdentifierSingleQuoted,
    AfterDoctypeSystemIdentifier,
    BogusDoctype,
    CdataSection,
    CdataSectionBracket,
    CdataSectionEnd,
    ProcessingInstructionOpen,
    ProcessingInstructionTarget,
    AfterProcessingInstructionTarget,
    ProcessingInstructionData,
    ProcessingInstructionQuestionable,
    CharacterReference,
    NamedCharacterReference,
    AmbiguousAmpersand,
    NumericCharacterReference,
    HexadecimalCharacterReferenceStart,
    HexadecimalCharacterReference,
    DecimalCharacterReference,
}

/// Tokenizer options.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokenizerOpts {
    /// Tokenize `<?target data>` as a processing instruction (WHATWG HTML 2026, §13.2.5.80–84) instead of a
    /// bogus comment.
    pub processing_instructions: bool,
}

/// §13.2.3.5: normalise newlines (CRLF and lone CR become LF).
pub fn preprocess(input: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(input.len());
    let mut it = input.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\r' {
            if it.peek() == Some(&'\n') {
                it.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

#[inline]
fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | ' ')
}

/// The tokenizer state machine.
pub struct Tokenizer {
    input: Vec<char>,
    pos: usize,
    last_was_eof: bool,
    /// The current state. The tree builder sets it after `<title>`, `<script>`, `<plaintext>`, ….
    pub state: State,
    return_state: State,
    /// §13.2.5.42: whether `<![CDATA[` opens a CDATA section (adjusted current node not HTML).
    pub allow_cdata: bool,
    /// Tag name of the last start tag emitted (for "appropriate end tag token").
    pub last_start_tag: Option<String>,
    opts: TokenizerOpts,
    queue: VecDeque<Token>,
    tag: Tag,
    tag_is_end: bool,
    attr_name: String,
    attr_value: String,
    attr_active: bool,
    comment: String,
    doctype: Doctype,
    temp: String,
    char_ref_code: u32,
    pi_target: String,
    pi_data: String,
    done: bool,
    /// Number of parse errors seen (codes are not recorded).
    pub errors: usize,
}

impl Tokenizer {
    /// A tokenizer over already-preprocessed input.
    pub fn new(input: Vec<char>, opts: TokenizerOpts) -> Tokenizer {
        Tokenizer {
            input,
            pos: 0,
            last_was_eof: false,
            state: State::Data,
            return_state: State::Data,
            allow_cdata: false,
            last_start_tag: None,
            opts,
            queue: VecDeque::new(),
            tag: Tag::default(),
            tag_is_end: false,
            attr_name: String::new(),
            attr_value: String::new(),
            attr_active: false,
            comment: String::new(),
            doctype: Doctype::default(),
            temp: String::new(),
            char_ref_code: 0,
            pi_target: String::new(),
            pi_data: String::new(),
            done: false,
            errors: 0,
        }
    }

    /// A tokenizer over a `&str` (preprocessed here).
    pub fn from_str(input: &str, opts: TokenizerOpts) -> Tokenizer {
        Tokenizer::new(preprocess(input), opts)
    }

    /// The next token. After `Token::Eof` every call returns `Token::Eof`.
    pub fn next_token(&mut self) -> Token {
        loop {
            if let Some(t) = self.queue.pop_front() {
                return t;
            }
            if self.done {
                return Token::Eof;
            }
            self.step();
        }
    }

    // ---- input ---------------------------------------------------------------------------------------------

    #[inline]
    fn consume(&mut self) -> Option<char> {
        if self.pos < self.input.len() {
            let c = self.input[self.pos];
            self.pos += 1;
            self.last_was_eof = false;
            Some(c)
        } else {
            self.last_was_eof = true;
            None
        }
    }

    #[inline]
    fn reconsume(&mut self, s: State) {
        if !self.last_was_eof {
            self.pos -= 1;
        }
        self.state = s;
    }

    /// Does the input starting at `at` match `s` (ASCII case-insensitively if `ci`)?
    fn lookahead(&self, at: usize, s: &str, ci: bool) -> bool {
        let n = s.chars().count();
        if at + n > self.input.len() {
            return false;
        }
        self.input[at..at + n].iter().zip(s.chars()).all(|(&a, b)| {
            if ci {
                a.to_ascii_lowercase() == b.to_ascii_lowercase()
            } else {
                a == b
            }
        })
    }

    // ---- emission ------------------------------------------------------------------------------------------

    #[inline]
    fn emit_char(&mut self, c: char) {
        self.queue.push_back(Token::Character(c));
    }

    fn emit_str(&mut self, s: &str) {
        for c in s.chars() {
            self.emit_char(c);
        }
    }

    fn emit_eof(&mut self) {
        self.queue.push_back(Token::Eof);
        self.done = true;
    }

    fn err(&mut self) {
        self.errors += 1;
    }

    fn new_tag(&mut self, end: bool) {
        self.tag = Tag::default();
        self.tag_is_end = end;
        self.attr_active = false;
    }

    fn start_attr(&mut self) {
        self.finish_attr();
        self.attr_name.clear();
        self.attr_value.clear();
        self.attr_active = true;
    }

    fn finish_attr(&mut self) {
        if !self.attr_active {
            return;
        }
        self.attr_active = false;
        let name = core::mem::take(&mut self.attr_name);
        let value = core::mem::take(&mut self.attr_value);
        if self.tag.attrs.iter().any(|a| a.name == name) {
            self.err(); // duplicate-attribute: the later one is dropped
        } else {
            self.tag.attrs.push(Attr { name, value });
        }
    }

    fn emit_tag(&mut self) {
        self.finish_attr();
        let tag = core::mem::take(&mut self.tag);
        if self.tag_is_end {
            if !tag.attrs.is_empty() || tag.self_closing {
                self.err();
            }
            self.queue.push_back(Token::EndTag(tag));
        } else {
            self.last_start_tag = Some(tag.name.clone());
            self.queue.push_back(Token::StartTag(tag));
        }
    }

    fn emit_comment(&mut self) {
        let c = core::mem::take(&mut self.comment);
        self.queue.push_back(Token::Comment(c));
    }

    fn emit_doctype(&mut self) {
        let d = core::mem::take(&mut self.doctype);
        self.queue.push_back(Token::Doctype(d));
    }

    fn appropriate_end_tag(&self) -> bool {
        self.tag_is_end && self.last_start_tag.as_deref() == Some(self.tag.name.as_str())
    }

    fn char_ref_in_attr(&self) -> bool {
        matches!(
            self.return_state,
            State::AttributeValueDoubleQuoted | State::AttributeValueSingleQuoted | State::AttributeValueUnquoted
        )
    }

    fn flush_char_ref(&mut self) {
        let t = core::mem::take(&mut self.temp);
        if self.char_ref_in_attr() {
            self.attr_value.push_str(&t);
        } else {
            self.emit_str(&t);
        }
    }

    // ---- the state machine ---------------------------------------------------------------------------------

    fn step(&mut self) {
        use State as S;
        match self.state {
            // §13.2.5.1
            S::Data => match self.consume() {
                Some('&') => {
                    self.return_state = S::Data;
                    self.state = S::CharacterReference;
                }
                Some('<') => self.state = S::TagOpen,
                Some('\0') => {
                    self.err();
                    self.emit_char('\0');
                }
                None => self.emit_eof(),
                Some(c) => self.emit_char(c),
            },
            // §13.2.5.2
            S::Rcdata => match self.consume() {
                Some('&') => {
                    self.return_state = S::Rcdata;
                    self.state = S::CharacterReference;
                }
                Some('<') => self.state = S::RcdataLessThan,
                Some('\0') => {
                    self.err();
                    self.emit_char('\u{FFFD}');
                }
                None => self.emit_eof(),
                Some(c) => self.emit_char(c),
            },
            // §13.2.5.3
            S::Rawtext => match self.consume() {
                Some('<') => self.state = S::RawtextLessThan,
                Some('\0') => {
                    self.err();
                    self.emit_char('\u{FFFD}');
                }
                None => self.emit_eof(),
                Some(c) => self.emit_char(c),
            },
            // §13.2.5.4
            S::ScriptData => match self.consume() {
                Some('<') => self.state = S::ScriptDataLessThan,
                Some('\0') => {
                    self.err();
                    self.emit_char('\u{FFFD}');
                }
                None => self.emit_eof(),
                Some(c) => self.emit_char(c),
            },
            // §13.2.5.5
            S::Plaintext => match self.consume() {
                Some('\0') => {
                    self.err();
                    self.emit_char('\u{FFFD}');
                }
                None => self.emit_eof(),
                Some(c) => self.emit_char(c),
            },
            // §13.2.5.6
            S::TagOpen => match self.consume() {
                Some('!') => self.state = S::MarkupDeclarationOpen,
                Some('/') => self.state = S::EndTagOpen,
                Some(c) if c.is_ascii_alphabetic() => {
                    self.new_tag(false);
                    self.reconsume(S::TagName);
                }
                Some('?') if self.opts.processing_instructions => {
                    self.temp.clear();
                    self.state = S::ProcessingInstructionOpen;
                }
                Some('?') => {
                    self.err(); // unexpected-question-mark-instead-of-tag-name
                    self.comment.clear();
                    self.reconsume(S::BogusComment);
                }
                None => {
                    self.err();
                    self.emit_char('<');
                    self.emit_eof();
                }
                Some(_) => {
                    self.err();
                    self.emit_char('<');
                    self.reconsume(S::Data);
                }
            },
            // §13.2.5.7
            S::EndTagOpen => match self.consume() {
                Some(c) if c.is_ascii_alphabetic() => {
                    self.new_tag(true);
                    self.reconsume(S::TagName);
                }
                Some('>') => {
                    self.err();
                    self.state = S::Data;
                }
                None => {
                    self.err();
                    self.emit_str("</");
                    self.emit_eof();
                }
                Some(_) => {
                    self.err();
                    self.comment.clear();
                    self.reconsume(S::BogusComment);
                }
            },
            // §13.2.5.8
            S::TagName => match self.consume() {
                Some(c) if is_ws(c) => self.state = S::BeforeAttributeName,
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                Some('\0') => {
                    self.err();
                    self.tag.name.push('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => self.tag.name.push(c.to_ascii_lowercase()),
            },
            // §13.2.5.9–11
            S::RcdataLessThan => self.text_less_than(S::Rcdata, S::RcdataEndTagOpen),
            S::RcdataEndTagOpen => self.text_end_tag_open(S::Rcdata, S::RcdataEndTagName),
            S::RcdataEndTagName => self.text_end_tag_name(S::Rcdata),
            // §13.2.5.12–14
            S::RawtextLessThan => self.text_less_than(S::Rawtext, S::RawtextEndTagOpen),
            S::RawtextEndTagOpen => self.text_end_tag_open(S::Rawtext, S::RawtextEndTagName),
            S::RawtextEndTagName => self.text_end_tag_name(S::Rawtext),
            // §13.2.5.15
            S::ScriptDataLessThan => match self.consume() {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataEndTagOpen;
                }
                Some('!') => {
                    self.state = S::ScriptDataEscapeStart;
                    self.emit_str("<!");
                }
                _ => {
                    self.emit_char('<');
                    self.reconsume(S::ScriptData);
                }
            },
            // §13.2.5.16–17
            S::ScriptDataEndTagOpen => self.text_end_tag_open(S::ScriptData, S::ScriptDataEndTagName),
            S::ScriptDataEndTagName => self.text_end_tag_name(S::ScriptData),
            // §13.2.5.18
            S::ScriptDataEscapeStart => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataEscapeStartDash;
                    self.emit_char('-');
                }
                _ => self.reconsume(S::ScriptData),
            },
            // §13.2.5.19
            S::ScriptDataEscapeStartDash => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                _ => self.reconsume(S::ScriptData),
            },
            // §13.2.5.20
            S::ScriptDataEscaped => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = S::ScriptDataEscapedLessThan,
                Some('\0') => {
                    self.err();
                    self.emit_char('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => self.emit_char(c),
            },
            // §13.2.5.21
            S::ScriptDataEscapedDash => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = S::ScriptDataEscapedLessThan,
                Some('\0') => {
                    self.err();
                    self.state = S::ScriptDataEscaped;
                    self.emit_char('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => {
                    self.state = S::ScriptDataEscaped;
                    self.emit_char(c);
                }
            },
            // §13.2.5.22
            S::ScriptDataEscapedDashDash => match self.consume() {
                Some('-') => self.emit_char('-'),
                Some('<') => self.state = S::ScriptDataEscapedLessThan,
                Some('>') => {
                    self.state = S::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.err();
                    self.state = S::ScriptDataEscaped;
                    self.emit_char('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => {
                    self.state = S::ScriptDataEscaped;
                    self.emit_char(c);
                }
            },
            // §13.2.5.23
            S::ScriptDataEscapedLessThan => match self.consume() {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataEscapedEndTagOpen;
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.temp.clear();
                    self.emit_char('<');
                    self.reconsume(S::ScriptDataDoubleEscapeStart);
                }
                _ => {
                    self.emit_char('<');
                    self.reconsume(S::ScriptDataEscaped);
                }
            },
            // §13.2.5.24–25
            S::ScriptDataEscapedEndTagOpen => {
                self.text_end_tag_open(S::ScriptDataEscaped, S::ScriptDataEscapedEndTagName)
            }
            S::ScriptDataEscapedEndTagName => self.text_end_tag_name(S::ScriptDataEscaped),
            // §13.2.5.26
            S::ScriptDataDoubleEscapeStart => match self.consume() {
                Some(c) if is_ws(c) || c == '/' || c == '>' => {
                    self.state =
                        if self.temp == "script" { S::ScriptDataDoubleEscaped } else { S::ScriptDataEscaped };
                    self.emit_char(c);
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.temp.push(c.to_ascii_lowercase());
                    self.emit_char(c);
                }
                _ => self.reconsume(S::ScriptDataEscaped),
            },
            // §13.2.5.27
            S::ScriptDataDoubleEscaped => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataDoubleEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThan;
                    self.emit_char('<');
                }
                Some('\0') => {
                    self.err();
                    self.emit_char('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => self.emit_char(c),
            },
            // §13.2.5.28
            S::ScriptDataDoubleEscapedDash => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataDoubleEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThan;
                    self.emit_char('<');
                }
                Some('\0') => {
                    self.err();
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => {
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char(c);
                }
            },
            // §13.2.5.29
            S::ScriptDataDoubleEscapedDashDash => match self.consume() {
                Some('-') => self.emit_char('-'),
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThan;
                    self.emit_char('<');
                }
                Some('>') => {
                    self.state = S::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.err();
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => {
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char(c);
                }
            },
            // §13.2.5.30
            S::ScriptDataDoubleEscapedLessThan => match self.consume() {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataDoubleEscapeEnd;
                    self.emit_char('/');
                }
                _ => self.reconsume(S::ScriptDataDoubleEscaped),
            },
            // §13.2.5.31
            S::ScriptDataDoubleEscapeEnd => match self.consume() {
                Some(c) if is_ws(c) || c == '/' || c == '>' => {
                    self.state =
                        if self.temp == "script" { S::ScriptDataEscaped } else { S::ScriptDataDoubleEscaped };
                    self.emit_char(c);
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.temp.push(c.to_ascii_lowercase());
                    self.emit_char(c);
                }
                _ => self.reconsume(S::ScriptDataDoubleEscaped),
            },
            // §13.2.5.32
            S::BeforeAttributeName => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('/') | Some('>') | None => self.reconsume(S::AfterAttributeName),
                Some('=') => {
                    self.err();
                    self.start_attr();
                    self.attr_name.push('=');
                    self.state = S::AttributeName;
                }
                Some(_) => {
                    self.start_attr();
                    self.reconsume(S::AttributeName);
                }
            },
            // §13.2.5.33
            S::AttributeName => match self.consume() {
                Some(c) if is_ws(c) || c == '/' || c == '>' => self.reconsume(S::AfterAttributeName),
                None => self.reconsume(S::AfterAttributeName),
                Some('=') => self.state = S::BeforeAttributeValue,
                Some('\0') => {
                    self.err();
                    self.attr_name.push('\u{FFFD}');
                }
                Some(c) => {
                    if c == '"' || c == '\'' || c == '<' {
                        self.err();
                    }
                    self.attr_name.push(c.to_ascii_lowercase());
                }
            },
            // §13.2.5.34
            S::AfterAttributeName => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('=') => self.state = S::BeforeAttributeValue,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(_) => {
                    self.start_attr();
                    self.reconsume(S::AttributeName);
                }
            },
            // §13.2.5.35
            S::BeforeAttributeValue => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('"') => self.state = S::AttributeValueDoubleQuoted,
                Some('\'') => self.state = S::AttributeValueSingleQuoted,
                Some('>') => {
                    self.err();
                    self.state = S::Data;
                    self.emit_tag();
                }
                _ => self.reconsume(S::AttributeValueUnquoted),
            },
            // §13.2.5.36–37
            S::AttributeValueDoubleQuoted => self.attr_value_quoted('"', S::AttributeValueDoubleQuoted),
            S::AttributeValueSingleQuoted => self.attr_value_quoted('\'', S::AttributeValueSingleQuoted),
            // §13.2.5.38
            S::AttributeValueUnquoted => match self.consume() {
                Some(c) if is_ws(c) => self.state = S::BeforeAttributeName,
                Some('&') => {
                    self.return_state = S::AttributeValueUnquoted;
                    self.state = S::CharacterReference;
                }
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                Some('\0') => {
                    self.err();
                    self.attr_value.push('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => {
                    if matches!(c, '"' | '\'' | '<' | '=' | '`') {
                        self.err();
                    }
                    self.attr_value.push(c);
                }
            },
            // §13.2.5.39
            S::AfterAttributeValueQuoted => match self.consume() {
                Some(c) if is_ws(c) => self.state = S::BeforeAttributeName,
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(_) => {
                    self.err();
                    self.reconsume(S::BeforeAttributeName);
                }
            },
            // §13.2.5.40
            S::SelfClosingStartTag => match self.consume() {
                Some('>') => {
                    self.tag.self_closing = true;
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(_) => {
                    self.err();
                    self.reconsume(S::BeforeAttributeName);
                }
            },
            // §13.2.5.41
            S::BogusComment => match self.consume() {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                Some('\0') => {
                    self.err();
                    self.comment.push('\u{FFFD}');
                }
                Some(c) => self.comment.push(c),
            },
            // §13.2.5.42
            S::MarkupDeclarationOpen => {
                if self.lookahead(self.pos, "--", false) {
                    self.pos += 2;
                    self.comment.clear();
                    self.state = S::CommentStart;
                } else if self.lookahead(self.pos, "doctype", true) {
                    self.pos += 7;
                    self.state = S::Doctype;
                } else if self.lookahead(self.pos, "[CDATA[", false) {
                    self.pos += 7;
                    if self.allow_cdata {
                        self.state = S::CdataSection;
                    } else {
                        self.err(); // cdata-in-html-content
                        self.comment.clear();
                        self.comment.push_str("[CDATA[");
                        self.state = S::BogusComment;
                    }
                } else {
                    self.err(); // incorrectly-opened-comment
                    self.comment.clear();
                    self.state = S::BogusComment;
                }
            }
            // §13.2.5.43
            S::CommentStart => match self.consume() {
                Some('-') => self.state = S::CommentStartDash,
                Some('>') => {
                    self.err();
                    self.state = S::Data;
                    self.emit_comment();
                }
                _ => self.reconsume(S::Comment),
            },
            // §13.2.5.44
            S::CommentStartDash => match self.consume() {
                Some('-') => self.state = S::CommentEnd,
                Some('>') => {
                    self.err();
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.err();
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push('-');
                    self.reconsume(S::Comment);
                }
            },
            // §13.2.5.45
            S::Comment => match self.consume() {
                Some('<') => {
                    self.comment.push('<');
                    self.state = S::CommentLessThan;
                }
                Some('-') => self.state = S::CommentEndDash,
                Some('\0') => {
                    self.err();
                    self.comment.push('\u{FFFD}');
                }
                None => {
                    self.err();
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(c) => self.comment.push(c),
            },
            // §13.2.5.46
            S::CommentLessThan => match self.consume() {
                Some('!') => {
                    self.comment.push('!');
                    self.state = S::CommentLessThanBang;
                }
                Some('<') => self.comment.push('<'),
                _ => self.reconsume(S::Comment),
            },
            // §13.2.5.47
            S::CommentLessThanBang => match self.consume() {
                Some('-') => self.state = S::CommentLessThanBangDash,
                _ => self.reconsume(S::Comment),
            },
            // §13.2.5.48
            S::CommentLessThanBangDash => match self.consume() {
                Some('-') => self.state = S::CommentLessThanBangDashDash,
                _ => self.reconsume(S::CommentEndDash),
            },
            // §13.2.5.49
            S::CommentLessThanBangDashDash => match self.consume() {
                Some('>') | None => self.reconsume(S::CommentEnd),
                Some(_) => {
                    self.err(); // nested-comment
                    self.reconsume(S::CommentEnd);
                }
            },
            // §13.2.5.50
            S::CommentEndDash => match self.consume() {
                Some('-') => self.state = S::CommentEnd,
                None => {
                    self.err();
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push('-');
                    self.reconsume(S::Comment);
                }
            },
            // §13.2.5.51
            S::CommentEnd => match self.consume() {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                Some('!') => self.state = S::CommentEndBang,
                Some('-') => self.comment.push('-'),
                None => {
                    self.err();
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push_str("--");
                    self.reconsume(S::Comment);
                }
            },
            // §13.2.5.52
            S::CommentEndBang => match self.consume() {
                Some('-') => {
                    self.comment.push_str("--!");
                    self.state = S::CommentEndDash;
                }
                Some('>') => {
                    self.err();
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.err();
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push_str("--!");
                    self.reconsume(S::Comment);
                }
            },
            // §13.2.5.53
            S::Doctype => match self.consume() {
                Some(c) if is_ws(c) => self.state = S::BeforeDoctypeName,
                Some('>') => self.reconsume(S::BeforeDoctypeName),
                None => {
                    self.err();
                    self.doctype = Doctype { force_quirks: true, ..Doctype::default() };
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(_) => {
                    self.err();
                    self.reconsume(S::BeforeDoctypeName);
                }
            },
            // §13.2.5.54
            S::BeforeDoctypeName => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('\0') => {
                    self.err();
                    self.doctype = Doctype { name: Some(String::from("\u{FFFD}")), ..Doctype::default() };
                    self.state = S::DoctypeName;
                }
                Some('>') => {
                    self.err();
                    self.doctype = Doctype { force_quirks: true, ..Doctype::default() };
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => {
                    self.err();
                    self.doctype = Doctype { force_quirks: true, ..Doctype::default() };
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(c) => {
                    let mut n = String::new();
                    n.push(c.to_ascii_lowercase());
                    self.doctype = Doctype { name: Some(n), ..Doctype::default() };
                    self.state = S::DoctypeName;
                }
            },
            // §13.2.5.55
            S::DoctypeName => match self.consume() {
                Some(c) if is_ws(c) => self.state = S::AfterDoctypeName,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                Some('\0') => {
                    self.err();
                    self.doctype.name.get_or_insert_with(String::new).push('\u{FFFD}');
                }
                None => self.eof_in_doctype(),
                Some(c) => self.doctype.name.get_or_insert_with(String::new).push(c.to_ascii_lowercase()),
            },
            // §13.2.5.56
            S::AfterDoctypeName => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    let at = self.pos - 1;
                    if self.lookahead(at, "public", true) {
                        self.pos = at + 6;
                        self.state = S::AfterDoctypePublicKeyword;
                    } else if self.lookahead(at, "system", true) {
                        self.pos = at + 6;
                        self.state = S::AfterDoctypeSystemKeyword;
                    } else {
                        self.err();
                        self.doctype.force_quirks = true;
                        self.reconsume(S::BogusDoctype);
                    }
                }
            },
            // §13.2.5.57
            S::AfterDoctypePublicKeyword => match self.consume() {
                Some(c) if is_ws(c) => self.state = S::BeforeDoctypePublicIdentifier,
                Some('"') => {
                    self.err();
                    self.doctype.public_id = Some(String::new());
                    self.state = S::DoctypePublicIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.err();
                    self.doctype.public_id = Some(String::new());
                    self.state = S::DoctypePublicIdentifierSingleQuoted;
                }
                Some('>') => self.doctype_gt_error(),
                None => self.eof_in_doctype(),
                Some(_) => self.doctype_bogus(),
            },
            // §13.2.5.58
            S::BeforeDoctypePublicIdentifier => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('"') => {
                    self.doctype.public_id = Some(String::new());
                    self.state = S::DoctypePublicIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.public_id = Some(String::new());
                    self.state = S::DoctypePublicIdentifierSingleQuoted;
                }
                Some('>') => self.doctype_gt_error(),
                None => self.eof_in_doctype(),
                Some(_) => self.doctype_bogus(),
            },
            // §13.2.5.59–60
            S::DoctypePublicIdentifierDoubleQuoted => self.doctype_id('"', true),
            S::DoctypePublicIdentifierSingleQuoted => self.doctype_id('\'', true),
            // §13.2.5.61
            S::AfterDoctypePublicIdentifier => match self.consume() {
                Some(c) if is_ws(c) => self.state = S::BetweenDoctypePublicAndSystemIdentifiers,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                Some('"') => {
                    self.err();
                    self.doctype.system_id = Some(String::new());
                    self.state = S::DoctypeSystemIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.err();
                    self.doctype.system_id = Some(String::new());
                    self.state = S::DoctypeSystemIdentifierSingleQuoted;
                }
                None => self.eof_in_doctype(),
                Some(_) => self.doctype_bogus(),
            },
            // §13.2.5.62
            S::BetweenDoctypePublicAndSystemIdentifiers => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                Some('"') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = S::DoctypeSystemIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = S::DoctypeSystemIdentifierSingleQuoted;
                }
                None => self.eof_in_doctype(),
                Some(_) => self.doctype_bogus(),
            },
            // §13.2.5.63
            S::AfterDoctypeSystemKeyword => match self.consume() {
                Some(c) if is_ws(c) => self.state = S::BeforeDoctypeSystemIdentifier,
                Some('"') => {
                    self.err();
                    self.doctype.system_id = Some(String::new());
                    self.state = S::DoctypeSystemIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.err();
                    self.doctype.system_id = Some(String::new());
                    self.state = S::DoctypeSystemIdentifierSingleQuoted;
                }
                Some('>') => self.doctype_gt_error(),
                None => self.eof_in_doctype(),
                Some(_) => self.doctype_bogus(),
            },
            // §13.2.5.64
            S::BeforeDoctypeSystemIdentifier => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('"') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = S::DoctypeSystemIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = S::DoctypeSystemIdentifierSingleQuoted;
                }
                Some('>') => self.doctype_gt_error(),
                None => self.eof_in_doctype(),
                Some(_) => self.doctype_bogus(),
            },
            // §13.2.5.65–66
            S::DoctypeSystemIdentifierDoubleQuoted => self.doctype_id('"', false),
            S::DoctypeSystemIdentifierSingleQuoted => self.doctype_id('\'', false),
            // §13.2.5.67
            S::AfterDoctypeSystemIdentifier => match self.consume() {
                Some(c) if is_ws(c) => {}
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    self.err(); // unexpected-character-after-doctype-system-identifier (no force-quirks)
                    self.reconsume(S::BogusDoctype);
                }
            },
            // §13.2.5.68
            S::BogusDoctype => match self.consume() {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                Some('\0') => self.err(),
                None => {
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(_) => {}
            },
            // §13.2.5.69
            S::CdataSection => match self.consume() {
                Some(']') => self.state = S::CdataSectionBracket,
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => self.emit_char(c),
            },
            // §13.2.5.70
            S::CdataSectionBracket => match self.consume() {
                Some(']') => self.state = S::CdataSectionEnd,
                _ => {
                    self.emit_char(']');
                    self.reconsume(S::CdataSection);
                }
            },
            // §13.2.5.71
            S::CdataSectionEnd => match self.consume() {
                Some(']') => self.emit_char(']'),
                Some('>') => self.state = S::Data,
                _ => {
                    self.emit_str("]]");
                    self.reconsume(S::CdataSection);
                }
            },
            // Processing instructions (2026 spec).
            S::ProcessingInstructionOpen => match self.consume() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => self.reconsume(S::ProcessingInstructionTarget),
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(_) => {
                    self.err();
                    self.pi_to_bogus_comment();
                }
            },
            S::ProcessingInstructionTarget => match self.consume() {
                Some(c) if is_ws(c) || c == '?' || c == '>' => {
                    let t = self.temp.to_ascii_lowercase();
                    if t == "xml" || t == "xml-stylesheet" {
                        self.err();
                        self.pi_to_bogus_comment();
                    } else {
                        self.pi_target = core::mem::take(&mut self.temp);
                        self.pi_data.clear();
                        self.reconsume(S::AfterProcessingInstructionTarget);
                    }
                }
                Some(c) if c.is_ascii_alphanumeric() || c == '-' || c == '_' => self.temp.push(c),
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(_) => {
                    self.err();
                    self.pi_to_bogus_comment();
                }
            },
            S::AfterProcessingInstructionTarget => match self.consume() {
                Some(c) if is_ws(c) => {}
                _ => self.reconsume(S::ProcessingInstructionData),
            },
            S::ProcessingInstructionData => match self.consume() {
                Some('?') => self.state = S::ProcessingInstructionQuestionable,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_pi();
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(c) => self.pi_data.push(c),
            },
            S::ProcessingInstructionQuestionable => match self.consume() {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_pi();
                }
                None => {
                    self.err();
                    self.emit_eof();
                }
                Some(_) => {
                    self.pi_data.push('?');
                    self.reconsume(S::ProcessingInstructionData);
                }
            },
            // §13.2.5.72
            S::CharacterReference => {
                self.temp.clear();
                self.temp.push('&');
                match self.consume() {
                    Some(c) if c.is_ascii_alphanumeric() => self.reconsume(S::NamedCharacterReference),
                    Some('#') => {
                        self.temp.push('#');
                        self.state = S::NumericCharacterReference;
                    }
                    _ => {
                        self.flush_char_ref();
                        let r = self.return_state;
                        self.reconsume(r);
                    }
                }
            }
            // §13.2.5.73
            S::NamedCharacterReference => self.named_char_ref(),
            // §13.2.5.74
            S::AmbiguousAmpersand => match self.consume() {
                Some(c) if c.is_ascii_alphanumeric() => {
                    if self.char_ref_in_attr() {
                        self.attr_value.push(c);
                    } else {
                        self.emit_char(c);
                    }
                }
                Some(';') => {
                    self.err();
                    let r = self.return_state;
                    self.reconsume(r);
                }
                _ => {
                    let r = self.return_state;
                    self.reconsume(r);
                }
            },
            // §13.2.5.75
            S::NumericCharacterReference => {
                self.char_ref_code = 0;
                match self.consume() {
                    Some(c @ ('x' | 'X')) => {
                        self.temp.push(c);
                        self.state = S::HexadecimalCharacterReferenceStart;
                    }
                    Some(c) if c.is_ascii_digit() => self.reconsume(S::DecimalCharacterReference),
                    _ => {
                        self.err();
                        self.flush_char_ref();
                        let r = self.return_state;
                        self.reconsume(r);
                    }
                }
            }
            // §13.2.5.76
            S::HexadecimalCharacterReferenceStart => match self.consume() {
                Some(c) if c.is_ascii_hexdigit() => self.reconsume(S::HexadecimalCharacterReference),
                _ => {
                    self.err();
                    self.flush_char_ref();
                    let r = self.return_state;
                    self.reconsume(r);
                }
            },
            // §13.2.5.77
            S::HexadecimalCharacterReference => match self.consume() {
                Some(c) if c.is_ascii_hexdigit() => {
                    let d = c.to_digit(16).unwrap_or(0);
                    self.char_ref_code = self.char_ref_code.saturating_mul(16).saturating_add(d);
                }
                Some(';') => self.numeric_char_ref_end(),
                _ => {
                    self.err();
                    let r = self.state;
                    self.reconsume(r);
                    self.numeric_char_ref_end();
                }
            },
            // §13.2.5.78
            S::DecimalCharacterReference => match self.consume() {
                Some(c) if c.is_ascii_digit() => {
                    let d = c.to_digit(10).unwrap_or(0);
                    self.char_ref_code = self.char_ref_code.saturating_mul(10).saturating_add(d);
                }
                Some(';') => self.numeric_char_ref_end(),
                _ => {
                    self.err();
                    let r = self.state;
                    self.reconsume(r);
                    self.numeric_char_ref_end();
                }
            },
        }
    }

    // ---- shared state bodies -------------------------------------------------------------------------------

    /// RCDATA / RAWTEXT less-than sign state (§13.2.5.9, .12).
    fn text_less_than(&mut self, text: State, end_tag_open: State) {
        match self.consume() {
            Some('/') => {
                self.temp.clear();
                self.state = end_tag_open;
            }
            _ => {
                self.emit_char('<');
                self.reconsume(text);
            }
        }
    }

    /// RCDATA / RAWTEXT / script data (escaped) end tag open state.
    fn text_end_tag_open(&mut self, text: State, end_tag_name: State) {
        match self.consume() {
            Some(c) if c.is_ascii_alphabetic() => {
                self.new_tag(true);
                self.reconsume(end_tag_name);
            }
            _ => {
                self.emit_str("</");
                self.reconsume(text);
            }
        }
    }

    /// RCDATA / RAWTEXT / script data (escaped) end tag name state.
    fn text_end_tag_name(&mut self, text: State) {
        let c = self.consume();
        match c {
            Some(ch) if is_ws(ch) && self.appropriate_end_tag() => {
                self.state = State::BeforeAttributeName;
                return;
            }
            Some('/') if self.appropriate_end_tag() => {
                self.state = State::SelfClosingStartTag;
                return;
            }
            Some('>') if self.appropriate_end_tag() => {
                self.state = State::Data;
                self.emit_tag();
                return;
            }
            Some(ch) if ch.is_ascii_alphabetic() => {
                self.tag.name.push(ch.to_ascii_lowercase());
                self.temp.push(ch);
                return;
            }
            _ => {}
        }
        // Anything else
        self.emit_str("</");
        let t = core::mem::take(&mut self.temp);
        self.emit_str(&t);
        self.reconsume(text);
    }

    /// Attribute value (double/single-quoted) state (§13.2.5.36–37).
    fn attr_value_quoted(&mut self, quote: char, me: State) {
        match self.consume() {
            Some(c) if c == quote => self.state = State::AfterAttributeValueQuoted,
            Some('&') => {
                self.return_state = me;
                self.state = State::CharacterReference;
            }
            Some('\0') => {
                self.err();
                self.attr_value.push('\u{FFFD}');
            }
            None => {
                self.err();
                self.emit_eof();
            }
            Some(c) => self.attr_value.push(c),
        }
    }

    /// DOCTYPE public/system identifier (double/single-quoted) states.
    fn doctype_id(&mut self, quote: char, public: bool) {
        match self.consume() {
            Some(c) if c == quote => {
                self.state =
                    if public { State::AfterDoctypePublicIdentifier } else { State::AfterDoctypeSystemIdentifier };
            }
            Some('>') => self.doctype_gt_error(),
            None => self.eof_in_doctype(),
            Some(c) => {
                let c = if c == '\0' {
                    self.err();
                    '\u{FFFD}'
                } else {
                    c
                };
                let id = if public { &mut self.doctype.public_id } else { &mut self.doctype.system_id };
                id.get_or_insert_with(String::new).push(c);
            }
        }
    }

    fn eof_in_doctype(&mut self) {
        self.err();
        self.doctype.force_quirks = true;
        self.emit_doctype();
        self.emit_eof();
    }

    fn doctype_gt_error(&mut self) {
        self.err();
        self.doctype.force_quirks = true;
        self.state = State::Data;
        self.emit_doctype();
    }

    fn doctype_bogus(&mut self) {
        self.err();
        self.doctype.force_quirks = true;
        self.reconsume(State::BogusDoctype);
    }

    fn pi_to_bogus_comment(&mut self) {
        self.comment.clear();
        self.comment.push('?');
        let t = core::mem::take(&mut self.temp);
        self.comment.push_str(&t);
        self.reconsume(State::BogusComment);
    }

    fn emit_pi(&mut self) {
        let target = core::mem::take(&mut self.pi_target);
        let data = core::mem::take(&mut self.pi_data);
        self.queue.push_back(Token::ProcessingInstruction { target, data });
    }

    /// §13.2.5.73: consume the longest name in the table that matches the input.
    fn named_char_ref(&mut self) {
        let (mut lo, mut hi) = (0usize, ENTITIES.len());
        let mut i = 0usize;
        let mut best: Option<(usize, usize)> = None; // (entry index, length)
        while let Some(&c) = self.input.get(self.pos + i) {
            if !c.is_ascii() {
                break;
            }
            let b = c as u8;
            let range = &ENTITIES[lo..hi];
            let l = range.partition_point(|e| {
                let n = e.0.as_bytes();
                n.len() <= i || n[i] < b
            });
            let h = range.partition_point(|e| {
                let n = e.0.as_bytes();
                n.len() <= i || n[i] <= b
            });
            if l == h {
                break;
            }
            let (nlo, nhi) = (lo + l, lo + h);
            lo = nlo;
            hi = nhi;
            i += 1;
            if ENTITIES[lo].0.len() == i {
                best = Some((lo, i));
            }
        }
        match best {
            Some((idx, len)) => {
                let (name, value) = ENTITIES[idx];
                for k in 0..len {
                    let ch = self.input[self.pos + k];
                    self.temp.push(ch);
                }
                self.pos += len;
                self.last_was_eof = false;
                let last_semicolon = name.ends_with(';');
                let next = self.input.get(self.pos).copied();
                if self.char_ref_in_attr()
                    && !last_semicolon
                    && matches!(next, Some(c) if c == '=' || c.is_ascii_alphanumeric())
                {
                    self.flush_char_ref();
                    self.state = self.return_state;
                } else {
                    if !last_semicolon {
                        self.err();
                    }
                    self.temp.clear();
                    self.temp.push_str(value);
                    self.flush_char_ref();
                    self.state = self.return_state;
                }
            }
            None => {
                self.flush_char_ref();
                self.state = State::AmbiguousAmpersand;
            }
        }
    }

    /// §13.2.5.80 numeric character reference end state (consumes nothing).
    fn numeric_char_ref_end(&mut self) {
        let mut code = self.char_ref_code;
        if code == 0 || code > 0x10FFFF || (0xD800..=0xDFFF).contains(&code) {
            self.err();
            code = 0xFFFD;
        } else if (0xFDD0..=0xFDEF).contains(&code) || (code & 0xFFFE) == 0xFFFE {
            self.err(); // noncharacter: kept
        } else if code == 0x0D || ((code < 0x20 || (0x7F..=0x9F).contains(&code)) && !matches!(code, 0x09 | 0x0A | 0x0C | 0x20)) {
            self.err();
            code = match code {
                0x80 => 0x20AC,
                0x82 => 0x201A,
                0x83 => 0x0192,
                0x84 => 0x201E,
                0x85 => 0x2026,
                0x86 => 0x2020,
                0x87 => 0x2021,
                0x88 => 0x02C6,
                0x89 => 0x2030,
                0x8A => 0x0160,
                0x8B => 0x2039,
                0x8C => 0x0152,
                0x8E => 0x017D,
                0x91 => 0x2018,
                0x92 => 0x2019,
                0x93 => 0x201C,
                0x94 => 0x201D,
                0x95 => 0x2022,
                0x96 => 0x2013,
                0x97 => 0x2014,
                0x98 => 0x02DC,
                0x99 => 0x2122,
                0x9A => 0x0161,
                0x9B => 0x203A,
                0x9C => 0x0153,
                0x9E => 0x017E,
                0x9F => 0x0178,
                other => other,
            };
        }
        self.temp.clear();
        self.temp.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
        self.flush_char_ref();
        self.state = self.return_state;
    }
}
