//! Regions: areas built from a shape's parts with `&` (both), `|` (either) and `-` (but not).
//!
//! An [`Area`] is kept as the expression itself and answered point by point
//! ([`Area::contains`]): a point is in `a & b` exactly when it is in `a` and in `b`. Nothing
//! is ever clipped into a new outline, so there are no slivers, no degenerate tangent cases and
//! no precision drift; a rotated or stretched shape just maps the point back into its unit box
//! first. Rasterizing samples it at the resolution it draws at (cells, braille dots, pixels), and
//! hit testing asks it about one point. [`Area::bounds`] is a box that holds it, so sampling
//! only looks where the area can be.

use crate::geom::{Point, Rect};
use crate::part::Part;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Area {
    /// The inside of the part at this index of the shape's parts.
    Part(usize),
    And(Box<Area>, Box<Area>),
    Or(Box<Area>, Box<Area>),
    Minus(Box<Area>, Box<Area>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AreaError {
    /// Something other than a name, an operator or a bracket where one was expected; `at` is a
    /// byte offset into the expression.
    Unexpected {
        at: usize,
        found: String,
    },
    UnknownPart(String),
    /// The part is a stroke (a line, an arrow, an open path), which has no inside.
    NotAnArea(String),
}

impl fmt::Display for AreaError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            AreaError::Unexpected { at, found } if found.is_empty() => {
                write!(f, "the area expression ends early (at {at})")
            }
            AreaError::Unexpected { at, found } => write!(f, "unexpected `{found}` at {at}"),
            AreaError::UnknownPart(id) => write!(f, "no part is called `{id}`"),
            AreaError::NotAnArea(id) => write!(f, "`{id}` is a stroke, with no inside"),
        }
    }
}

impl std::error::Error for AreaError {}

impl Area {
    /// Parses an `area` expression over the parts' ids.
    ///
    /// `&` binds tighter than `|` and `-`, which go left to right: `a | b - c` is `(a | b) - c`,
    /// and `a - b & c` is `a - (b & c)`. Brackets group. A part id is letters, digits, `_` and
    /// `-` inside a word, so `only-a` is one name and the operator is written `a - b`.
    pub fn parse(expr: &str, parts: &[Part]) -> Result<Area, AreaError> {
        let mut parser = Parser {
            tokens: tokenize(expr)?,
            pos: 0,
            end: expr.len(),
            parts,
        };
        let area = parser.sum()?;
        match parser.tokens.get(parser.pos) {
            None => Ok(area),
            Some((at, tok)) => Err(AreaError::Unexpected {
                at: *at,
                found: tok.text(),
            }),
        }
    }

    pub fn contains(&self, parts: &[Part], p: Point) -> bool {
        match self {
            Area::Part(i) => parts[*i].contains(p),
            Area::And(a, b) => a.contains(parts, p) && b.contains(parts, p),
            Area::Or(a, b) => a.contains(parts, p) || b.contains(parts, p),
            Area::Minus(a, b) => a.contains(parts, p) && !b.contains(parts, p),
        }
    }

    /// A box, in unit coordinates, that holds every point of the area.
    pub fn bounds(&self, parts: &[Part]) -> Rect {
        match self {
            Area::Part(i) => parts[*i].bounds(),
            Area::And(a, b) => a.bounds(parts).intersect(&b.bounds(parts)),
            Area::Or(a, b) => a.bounds(parts).union(&b.bounds(parts)),
            Area::Minus(a, _) => a.bounds(parts),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Name(String),
    And,
    Or,
    Minus,
    Open,
    Close,
}

impl Token {
    fn text(&self) -> String {
        match self {
            Token::Name(name) => name.clone(),
            Token::And => "&".into(),
            Token::Or => "|".into(),
            Token::Minus => "-".into(),
            Token::Open => "(".into(),
            Token::Close => ")".into(),
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn tokenize(expr: &str) -> Result<Vec<(usize, Token)>, AreaError> {
    let mut tokens = Vec::new();
    let mut chars = expr.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let token = match c {
            c if c.is_whitespace() => continue,
            '&' => Token::And,
            '|' => Token::Or,
            '-' => Token::Minus,
            '(' => Token::Open,
            ')' => Token::Close,
            c if is_word(c) => {
                let mut end = at + c.len_utf8();
                while let Some(&(i, next)) = chars.peek() {
                    let hyphen_in_word =
                        next == '-' && expr[i + 1..].chars().next().is_some_and(is_word);
                    if !(is_word(next) || hyphen_in_word) {
                        break;
                    }
                    chars.next();
                    end = i + next.len_utf8();
                }
                Token::Name(expr[at..end].to_string())
            }
            other => {
                return Err(AreaError::Unexpected {
                    at,
                    found: other.to_string(),
                });
            }
        };
        tokens.push((at, token));
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: Vec<(usize, Token)>,
    pos: usize,
    end: usize,
    parts: &'a [Part],
}

impl Parser<'_> {
    fn next(&mut self) -> Option<(usize, Token)> {
        let tok = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        tok
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(_, t)| t)
    }

    fn sum(&mut self) -> Result<Area, AreaError> {
        let mut area = self.product()?;
        loop {
            let join: fn(Box<Area>, Box<Area>) -> Area = match self.peek() {
                Some(Token::Or) => Area::Or,
                Some(Token::Minus) => Area::Minus,
                _ => return Ok(area),
            };
            self.pos += 1;
            area = join(Box::new(area), Box::new(self.product()?));
        }
    }

    fn product(&mut self) -> Result<Area, AreaError> {
        let mut area = self.atom()?;
        while self.peek() == Some(&Token::And) {
            self.pos += 1;
            area = Area::And(Box::new(area), Box::new(self.atom()?));
        }
        Ok(area)
    }

    fn atom(&mut self) -> Result<Area, AreaError> {
        match self.next() {
            Some((_, Token::Name(name))) => {
                let index = self
                    .parts
                    .iter()
                    .position(|p| p.id.as_deref() == Some(name.as_str()))
                    .ok_or_else(|| AreaError::UnknownPart(name.clone()))?;
                if !self.parts[index].is_area() {
                    return Err(AreaError::NotAnArea(name));
                }
                Ok(Area::Part(index))
            }
            Some((_, Token::Open)) => {
                let area = self.sum()?;
                match self.next() {
                    Some((_, Token::Close)) => Ok(area),
                    Some((at, tok)) => Err(AreaError::Unexpected {
                        at,
                        found: tok.text(),
                    }),
                    None => Err(AreaError::Unexpected {
                        at: self.end,
                        found: String::new(),
                    }),
                }
            }
            Some((at, tok)) => Err(AreaError::Unexpected {
                at,
                found: tok.text(),
            }),
            None => Err(AreaError::Unexpected {
                at: self.end,
                found: String::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part::Kind;

    fn parts() -> Vec<Part> {
        let circle = |id, x| {
            Part::new(
                Some(id),
                Kind::Circle {
                    center: Point::new(x, 0.5),
                    radius: 0.32,
                },
            )
        };
        vec![
            circle("left", 0.38),
            circle("right", 0.62),
            circle("only-x", 0.5),
            Part::new(
                Some("rule"),
                Kind::Line {
                    from: Point::new(0.0, 0.0),
                    to: Point::new(1.0, 1.0),
                },
            ),
        ]
    }

    fn part(i: usize) -> Box<Area> {
        Box::new(Area::Part(i))
    }

    #[test]
    fn precedence_and_grouping() {
        let parts = parts();
        assert_eq!(
            Area::parse("left | right - only-x", &parts).unwrap(),
            Area::Minus(Box::new(Area::Or(part(0), part(1))), part(2))
        );
        assert_eq!(
            Area::parse("left - right & only-x", &parts).unwrap(),
            Area::Minus(part(0), Box::new(Area::And(part(1), part(2))))
        );
        assert_eq!(
            Area::parse("left - (right | only-x)", &parts).unwrap(),
            Area::Minus(part(0), Box::new(Area::Or(part(1), part(2))))
        );
    }

    #[test]
    fn errors_say_where_and_what() {
        let parts = parts();
        let err = |s| Area::parse(s, &parts).unwrap_err();
        assert_eq!(
            err("left - middle"),
            AreaError::UnknownPart("middle".into())
        );
        assert_eq!(err("left & rule"), AreaError::NotAnArea("rule".into()));
        assert_eq!(
            err("left & & right"),
            AreaError::Unexpected {
                at: 7,
                found: "&".into()
            }
        );
        assert_eq!(
            err("(left"),
            AreaError::Unexpected {
                at: 5,
                found: String::new()
            }
        );
        assert_eq!(
            err("left right"),
            AreaError::Unexpected {
                at: 5,
                found: "right".into()
            }
        );
        assert_eq!(
            err("left * right"),
            AreaError::Unexpected {
                at: 5,
                found: "*".into()
            }
        );
    }

    #[test]
    fn venn_regions_split_the_middle_line() {
        let parts = parts();
        let only_a = Area::parse("left - right", &parts).unwrap();
        let both = Area::parse("left & right", &parts).unwrap();
        let only_b = Area::parse("right - left", &parts).unwrap();
        let at = |x| Point::new(x, 0.5);
        assert!(only_a.contains(&parts, at(0.2)) && !both.contains(&parts, at(0.2)));
        assert!(both.contains(&parts, at(0.5)) && !only_a.contains(&parts, at(0.5)));
        assert!(only_b.contains(&parts, at(0.8)) && !only_a.contains(&parts, at(0.8)));
        let lens = both.bounds(&parts);
        assert!((lens.min.x - 0.30).abs() < 1e-9 && (lens.max.x - 0.70).abs() < 1e-9);
    }
}
