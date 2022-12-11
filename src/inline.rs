use crate::lex;
use crate::Span;

use lex::Delimiter;
use lex::Symbol;

use Atom::*;
use Container::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Atom {
    Softbreak,
    Hardbreak,
    Escape,
    Nbsp,
    Ellipsis,
    EnDash,
    EmDash,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Container {
    Span,
    // typesetting
    Subscript,
    Superscript,
    Insert,
    Delete,
    Emphasis,
    Strong,
    Mark,
    // smart quoting
    SingleQuoted,
    DoubleQuoted,
    // Verbatim
    Verbatim,
    RawFormat,
    InlineMath,
    DisplayMath,
    // Links
    ReferenceLink,
    InlineLink,
    AutoLink,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EventKind {
    Enter(Container),
    Exit(Container),
    Atom(Atom),
    Str,
    Attributes,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: EventKind,
    pub span: Span,
}

/// Current parsing state of elements that are not recursive, i.e. may not contain arbitrary inline
/// elements, can only be one of these at a time.
#[derive(Debug)]
enum State {
    None,
    /// Within a verbatim element, e.g. '$`xxxxx'
    Verbatim {
        kind: Container,
        opener_len: usize,
        opener_event: usize,
    },
    /// Potentially within an attribute list, e.g. '{a=b '.
    Attributes {
        comment: bool,
    },
    /// Potentially within an autolink URL or an inline link URL, e.g. '<https://' or
    /// '[text](https://'.
    Url {
        auto: bool,
    },
    /// Potentially within a reference link tag, e.g. '[text][tag '
    ReferenceLinkTag,
}

impl State {
    fn verbatim(&self) -> Option<(Container, usize, usize)> {
        if let Self::Verbatim {
            kind,
            opener_len,
            opener_event,
        } = self
        {
            Some((*kind, *opener_len, *opener_event))
        } else {
            None
        }
    }
}

pub struct Parser<'s> {
    openers: Vec<(Container, usize)>,
    events: std::collections::VecDeque<Event>,
    span: Span,

    lexer: lex::Lexer<'s>,

    state: State,
    last: bool,
}

impl<'s> Parser<'s> {
    pub fn new() -> Self {
        Self {
            openers: Vec::new(),
            events: std::collections::VecDeque::new(),
            span: Span::new(0, 0),

            lexer: lex::Lexer::new(""),

            state: State::None,
            last: false,
        }
    }

    pub fn parse(&mut self, src: &'s str, last: bool) {
        self.lexer = lex::Lexer::new(src);
        if last {
            assert!(!self.last);
        }
        self.last = last;
    }

    fn eat(&mut self) -> Option<lex::Token> {
        let tok = self.lexer.next();
        if let Some(t) = &tok {
            self.span = self.span.extend(t.len);
        }
        tok
    }

    fn peek(&mut self) -> Option<&lex::Token> {
        self.lexer.peek()
    }

    fn reset_span(&mut self) {
        self.span = Span::empty_at(self.span.end());
    }

    fn parse_event(&mut self) -> Option<Event> {
        self.reset_span();
        self.eat().map(|first| {
            self.parse_verbatim(&first)
                .or_else(|| self.parse_container(&first))
                .or_else(|| self.parse_atom(&first))
                .unwrap_or(Event {
                    kind: EventKind::Str,
                    span: self.span,
                })
        })
    }

    fn parse_atom(&mut self, first: &lex::Token) -> Option<Event> {
        let atom = match first.kind {
            lex::Kind::Newline => Softbreak,
            lex::Kind::Hardbreak => Hardbreak,
            lex::Kind::Escape => Escape,
            lex::Kind::Nbsp => Nbsp,
            lex::Kind::Seq(lex::Sequence::Period) if first.len == 3 => Ellipsis,
            lex::Kind::Seq(lex::Sequence::Hyphen) if first.len == 2 => EnDash,
            lex::Kind::Seq(lex::Sequence::Hyphen) if first.len == 3 => EmDash,
            _ => return None,
        };

        Some(Event {
            kind: EventKind::Atom(atom),
            span: self.span,
        })
    }

    fn parse_verbatim(&mut self, first: &lex::Token) -> Option<Event> {
        self.state
            .verbatim()
            .map(|(kind, opener_len, opener_event)| {
                dbg!(&self.events, opener_event);
                assert_eq!(self.events[opener_event].kind, EventKind::Enter(kind));
                let kind = if matches!(first.kind, lex::Kind::Seq(lex::Sequence::Backtick))
                    && first.len == opener_len
                {
                    self.state = State::None;
                    let kind =
                        if matches!(kind, Verbatim) && self.lexer.peek_ahead().starts_with("{=") {
                            let mut chars = self.lexer.peek_ahead()[2..].chars();
                            let len = chars
                                .clone()
                                .take_while(|c| !c.is_whitespace() && !matches!(c, '{' | '}'))
                                .count();
                            if len > 0 && chars.nth(len) == Some('}') {
                                self.lexer = lex::Lexer::new(chars.as_str());
                                let span_format = Span::by_len(self.span.end() + "{=".len(), len);
                                self.events[opener_event].kind = EventKind::Enter(RawFormat);
                                self.events[opener_event].span = span_format;
                                self.span = span_format;
                                RawFormat
                            } else {
                                Verbatim
                            }
                        } else {
                            kind
                        };
                    EventKind::Exit(kind)
                } else {
                    EventKind::Str
                };
                Event {
                    kind,
                    span: self.span,
                }
            })
            .or_else(|| {
                match first.kind {
                    lex::Kind::Seq(lex::Sequence::Dollar) => {
                        let math_opt = (first.len <= 2)
                            .then(|| {
                                if let Some(lex::Token {
                                    kind: lex::Kind::Seq(lex::Sequence::Backtick),
                                    len,
                                }) = self.peek()
                                {
                                    Some((
                                        if first.len == 2 {
                                            DisplayMath
                                        } else {
                                            InlineMath
                                        },
                                        *len,
                                    ))
                                } else {
                                    None
                                }
                            })
                            .flatten();
                        if math_opt.is_some() {
                            self.eat(); // backticks
                        }
                        math_opt
                    }
                    lex::Kind::Seq(lex::Sequence::Backtick) => Some((Verbatim, first.len)),
                    _ => None,
                }
                .map(|(kind, opener_len)| {
                    dbg!(&self.events);
                    self.state = State::Verbatim {
                        kind,
                        opener_len,
                        opener_event: self.events.len(),
                    };
                    Event {
                        kind: EventKind::Enter(kind),
                        span: self.span,
                    }
                })
            })
    }

    fn parse_container(&mut self, first: &lex::Token) -> Option<Event> {
        enum Dir {
            Open,
            Close,
            Both,
        }

        match first.kind {
            lex::Kind::Sym(Symbol::Asterisk) => Some((Strong, Dir::Both)),
            lex::Kind::Sym(Symbol::Underscore) => Some((Emphasis, Dir::Both)),
            lex::Kind::Sym(Symbol::Caret) => Some((Superscript, Dir::Both)),
            lex::Kind::Sym(Symbol::Tilde) => Some((Subscript, Dir::Both)),
            lex::Kind::Sym(Symbol::Quote1) => Some((SingleQuoted, Dir::Both)),
            lex::Kind::Sym(Symbol::Quote2) => Some((DoubleQuoted, Dir::Both)),
            lex::Kind::Open(Delimiter::Bracket) => Some((Span, Dir::Open)),
            lex::Kind::Close(Delimiter::Bracket) => Some((Span, Dir::Close)),
            lex::Kind::Open(Delimiter::BraceAsterisk) => Some((Strong, Dir::Open)),
            lex::Kind::Close(Delimiter::BraceAsterisk) => Some((Strong, Dir::Close)),
            lex::Kind::Open(Delimiter::BraceCaret) => Some((Superscript, Dir::Open)),
            lex::Kind::Close(Delimiter::BraceCaret) => Some((Superscript, Dir::Close)),
            lex::Kind::Open(Delimiter::BraceEqual) => Some((Mark, Dir::Open)),
            lex::Kind::Close(Delimiter::BraceEqual) => Some((Mark, Dir::Close)),
            lex::Kind::Open(Delimiter::BraceHyphen) => Some((Delete, Dir::Open)),
            lex::Kind::Close(Delimiter::BraceHyphen) => Some((Delete, Dir::Close)),
            lex::Kind::Open(Delimiter::BracePlus) => Some((Insert, Dir::Open)),
            lex::Kind::Close(Delimiter::BracePlus) => Some((Insert, Dir::Close)),
            lex::Kind::Open(Delimiter::BraceTilde) => Some((Subscript, Dir::Open)),
            lex::Kind::Close(Delimiter::BraceTilde) => Some((Subscript, Dir::Close)),
            lex::Kind::Open(Delimiter::BraceUnderscore) => Some((Emphasis, Dir::Open)),
            lex::Kind::Close(Delimiter::BraceUnderscore) => Some((Emphasis, Dir::Close)),
            _ => None,
        }
        .map(|(cont, dir)| {
            self.openers
                .iter()
                .rposition(|(c, _)| *c == cont)
                .and_then(|o| {
                    matches!(dir, Dir::Close | Dir::Both).then(|| {
                        let (_, e) = &mut self.openers[o];
                        self.events[*e].kind = EventKind::Enter(cont);
                        self.openers.drain(o..);
                        EventKind::Exit(cont)
                    })
                })
                .unwrap_or_else(|| {
                    self.openers.push((cont, self.events.len()));
                    // use str for now, replace if closed later
                    EventKind::Str
                })
        })
        .map(|kind| Event {
            kind,
            span: self.span,
        })
    }
}

impl<'s> Iterator for Parser<'s> {
    type Item = Event;

    fn next(&mut self) -> Option<Self::Item> {
        let mut need_more = false;
        while self.events.is_empty()
            || !self.openers.is_empty()
            || !matches!(self.state, State::None)
            || self // for merge
                .events
                .back()
                .map_or(false, |ev| matches!(ev.kind, EventKind::Str))
        {
            if let Some(ev) = self.parse_event() {
                self.events.push_back(ev);
                dbg!(&self.events, &self.state);
            } else {
                need_more = true;
                break;
            }
        }

        if self.last || !need_more {
            self.events
                .pop_front()
                .map(|e| {
                    if matches!(e.kind, EventKind::Str) {
                        // merge str events
                        let mut span = e.span;
                        while self
                            .events
                            .front()
                            .map_or(false, |ev| matches!(ev.kind, EventKind::Str))
                        {
                            let ev = self.events.pop_front().unwrap();
                            assert_eq!(span.end(), ev.span.start());
                            span = span.union(ev.span);
                        }
                        Event {
                            kind: EventKind::Str,
                            span,
                        }
                    } else {
                        e
                    }
                })
                .or_else(|| {
                    self.state.verbatim().map(|(kind, _, _)| {
                        self.state = State::None;
                        Event {
                            kind: EventKind::Exit(kind),
                            span: self.span,
                        }
                    })
                })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use crate::Span;

    use super::Atom::*;
    use super::Container::*;
    use super::EventKind::*;
    use super::Verbatim;

    macro_rules! test_parse {
        ($($st:ident,)? $src:expr $(,$($token:expr),* $(,)?)?) => {
            #[allow(unused)]
            let mut p = super::Parser::new();
            p.parse($src, true);
            let actual = p.map(|ev| (ev.kind, ev.span.of($src))).collect::<Vec<_>>();
            let expected = &[$($($token),*,)?];
            assert_eq!(actual, expected, "\n\n{}\n\n", $src);
        };
    }

    #[test]
    fn str() {
        test_parse!("abc", (Str, "abc"));
        test_parse!("abc def", (Str, "abc def"));
    }

    #[test]
    fn verbatim() {
        test_parse!(
            "`abc`",
            (Enter(Verbatim), "`"),
            (Str, "abc"),
            (Exit(Verbatim), "`"),
        );
        test_parse!(
            "`abc\ndef`",
            (Enter(Verbatim), "`"),
            (Str, "abc\ndef"),
            (Exit(Verbatim), "`"),
        );
        test_parse!(
            "`abc&def`",
            (Enter(Verbatim), "`"),
            (Str, "abc&def"),
            (Exit(Verbatim), "`"),
        );
        test_parse!(
            "`abc",
            (Enter(Verbatim), "`"),
            (Str, "abc"),
            (Exit(Verbatim), ""),
        );
        test_parse!(
            "``abc``",
            (Enter(Verbatim), "``"),
            (Str, "abc"),
            (Exit(Verbatim), "``"),
        );
        test_parse!(
            "abc `def`",
            (Str, "abc "),
            (Enter(Verbatim), "`"),
            (Str, "def"),
            (Exit(Verbatim), "`"),
        );
        test_parse!(
            "abc`def`",
            (Str, "abc"),
            (Enter(Verbatim), "`"),
            (Str, "def"),
            (Exit(Verbatim), "`"),
        );
    }

    #[test]
    fn math() {
        test_parse!(
            "$`abc`",
            (Enter(InlineMath), "$`"),
            (Str, "abc"),
            (Exit(InlineMath), "`"),
        );
        test_parse!(
            "$`abc` str",
            (Enter(InlineMath), "$`"),
            (Str, "abc"),
            (Exit(InlineMath), "`"),
            (Str, " str"),
        );
        test_parse!(
            "$$`abc`",
            (Enter(DisplayMath), "$$`"),
            (Str, "abc"),
            (Exit(DisplayMath), "`"),
        );
        test_parse!(
            "$`abc",
            (Enter(InlineMath), "$`"),
            (Str, "abc"),
            (Exit(InlineMath), ""),
        );
        test_parse!(
            "$```abc```",
            (Enter(InlineMath), "$```"),
            (Str, "abc"),
            (Exit(InlineMath), "```"),
        );
    }

    #[test]
    fn container_basic() {
        test_parse!(
            "_abc_",
            (Enter(Emphasis), "_"),
            (Str, "abc"),
            (Exit(Emphasis), "_"),
        );
        test_parse!(
            "{_abc_}",
            (Enter(Emphasis), "{_"),
            (Str, "abc"),
            (Exit(Emphasis), "_}"),
        );
    }

    #[test]
    fn container_nest() {
        test_parse!(
            "{_{_abc_}_}",
            (Enter(Emphasis), "{_"),
            (Enter(Emphasis), "{_"),
            (Str, "abc"),
            (Exit(Emphasis), "_}"),
            (Exit(Emphasis), "_}"),
        );
        test_parse!(
            "*_abc_*",
            (Enter(Strong), "*"),
            (Enter(Emphasis), "_"),
            (Str, "abc"),
            (Exit(Emphasis), "_"),
            (Exit(Strong), "*"),
        );
    }

    #[test]
    fn container_unopened() {
        test_parse!("*}abc", (Str, "*}abc"));
    }

    #[test]
    fn container_close_parent() {
        test_parse!(
            "{*{_abc*}",
            (Enter(Strong), "{*"),
            (Str, "{_abc"),
            (Exit(Strong), "*}"),
        );
    }

    #[test]
    fn container_close_block() {
        test_parse!("{_abc", (Str, "{_abc"));
        test_parse!("{_{*{_abc", (Str, "{_{*{_abc"));
    }
}
