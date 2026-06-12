mod literal;
mod primitive;
mod token_types;

use core::fmt;

pub use literal::{FloatLiteral, IntLiteral};
pub use primitive::{FloatType, IntType, Primitive};
pub use token_types::TokenType;

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub start: u32,
    pub end: u32,
    pub ty: TokenType,
    /// is this token on a different line than the previous one
    pub new_line: bool,
}

impl Token {
    pub fn new(ty: TokenType, start: u32, end: u32, new_line: bool) -> Self {
        Self {
            start,
            end,
            ty,
            new_line,
        }
    }

    pub fn get_val<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start as usize..self.end as usize]
    }
}

#[derive(Debug, Clone)]
pub enum ExpectedTokens {
    Specific(TokenType),
    AnyOf(Box<[TokenType]>),
    Expr,
    Type,
    Item,
    EndOfMultilineComment,
    EndOfStringLiteral,
}
impl std::fmt::Display for ExpectedTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            &Self::Specific(tok) => {
                write!(f, "{tok}")
            }
            Self::AnyOf(toks) => {
                for (i, tok) in toks.iter().enumerate() {
                    match i {
                        0 => {}
                        i if i != 0 || i == toks.len() - 1 => {
                            write!(f, " or ")?;
                        }
                        _ => write!(f, ", ")?,
                    }
                    write!(f, "{tok}")?;
                }
                Ok(())
            }
            Self::Expr => write!(f, "an expression"),
            Self::Type => write!(f, "a type"),
            Self::Item => write!(f, "an item"),
            Self::EndOfMultilineComment => write!(f, "end of comment"),
            Self::EndOfStringLiteral => write!(f, "end of string literal"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct TokenTypes<const N: usize>(pub [TokenType; N]);
impl From<TokenType> for TokenTypes<1> {
    fn from(x: TokenType) -> Self {
        Self([x])
    }
}
impl<const N: usize> From<[TokenType; N]> for TokenTypes<N> {
    fn from(x: [TokenType; N]) -> Self {
        Self(x)
    }
}

impl<const N: usize> From<TokenTypes<N>> for ExpectedTokens {
    fn from(t: TokenTypes<N>) -> Self {
        match t.0.as_slice() {
            [t] => Self::Specific(*t),
            other => Self::AnyOf(other.into()),
        }
    }
}
impl From<TokenType> for ExpectedTokens {
    fn from(value: TokenType) -> Self {
        Self::Specific(value)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Keyword {
    Primitive(Primitive),
    Fn,
    Ret,
    And,
    Or,
    As,
    Struct,
    Enum,
    Trait,
    Impl,
    If,
    Else,
    Match,
    While,
    For,
    Extern,
    Root,
    Use,
    Asm,
    Break,
    Continue,
    In,
}
impl From<Keyword> for &'static str {
    fn from(k: Keyword) -> Self {
        match k {
            Keyword::Primitive(p) => p.into(),
            Keyword::Fn => "fn",
            Keyword::Ret => "ret",
            Keyword::And => "and",
            Keyword::Or => "or",
            Keyword::As => "as",
            Keyword::Struct => "struct",
            Keyword::Enum => "enum",
            Keyword::Trait => "trait",
            Keyword::Impl => "impl",
            Keyword::If => "if",
            Keyword::Else => "else",
            Keyword::Match => "match",
            Keyword::While => "while",
            Keyword::For => "for",
            Keyword::Extern => "extern",
            Keyword::Root => "root",
            Keyword::Use => "use",
            Keyword::Asm => "asm",
            Keyword::Break => "break",
            Keyword::Continue => "continue",
            Keyword::In => "in",
        }
    }
}
impl Keyword {
    pub fn parse(s: &str) -> Option<Keyword> {
        use Keyword::Primitive as P;
        use Primitive::*;
        Some(match s {
            "i8" => P(I8),
            "i16" => P(I16),
            "i32" => P(I32),
            "i64" => P(I64),
            "i128" => P(I128),

            "u8" => P(U8),
            "u16" => P(U16),
            "u32" => P(U32),
            "u64" => P(U64),
            "u128" => P(U128),

            "f32" => P(F32),
            "f64" => P(F64),

            "type" => P(Type),

            "fn" => Keyword::Fn,
            "ret" => Keyword::Ret,
            "and" => Keyword::And,
            "or" => Keyword::Or,
            "as" => Keyword::As,
            "struct" => Keyword::Struct,
            "enum" => Keyword::Enum,

            "trait" => Keyword::Trait,
            "impl" => Keyword::Impl,

            "if" => Keyword::If,
            "else" => Keyword::Else,
            "match" => Keyword::Match,
            "while" => Keyword::While,
            "for" => Keyword::For,

            "extern" => Keyword::Extern,
            "root" => Keyword::Root,
            "use" => Keyword::Use,

            "asm" => Keyword::Asm,

            "break" => Keyword::Break,
            "continue" => Keyword::Continue,
            "in" => Keyword::In,
            _ => return None,
        })
    }
}
impl From<Keyword> for TokenType {
    fn from(k: Keyword) -> Self {
        TokenType::Keyword(k)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Add,
    Sub,
    Mul,
    Div,
    Mod,

    Assignment(AssignType),

    Or,
    And,

    Equals,
    NotEquals,

    LT,
    GT,
    LE,
    GE,

    Range,
    RangeExclusive,
}
impl From<TokenType> for Option<Operator> {
    fn from(tok: TokenType) -> Self {
        use Operator::*;
        Some(match tok {
            TokenType::Plus => Add,
            TokenType::Minus => Sub,
            TokenType::Star => Mul,
            TokenType::Slash => Div,
            TokenType::Percent => Mod,

            //TokenType::Equals => Assignment(AssignType::Assign),
            TokenType::PlusEquals => Assignment(AssignType::AddAssign),
            TokenType::MinusEquals => Assignment(AssignType::SubAssign),
            TokenType::StarEquals => Assignment(AssignType::MulAssign),
            TokenType::SlashEquals => Assignment(AssignType::DivAssign),
            TokenType::PercentEquals => Assignment(AssignType::ModAssign),

            TokenType::Keyword(Keyword::Or) => Or,
            TokenType::Keyword(Keyword::And) => And,

            TokenType::DoubleEquals => Equals,
            TokenType::BangEquals => NotEquals,

            TokenType::LessThan => LT,
            TokenType::GreaterThan => GT,
            TokenType::LessEquals => LE,
            TokenType::GreaterEquals => GE,

            TokenType::DotDot => Range,
            TokenType::DotDotLessThan => RangeExclusive,

            _ => return None,
        })
    }
}
impl Operator {
    pub fn precedence(self) -> u8 {
        use Operator::*;
        match self {
            Assignment(_) => 10,
            Range | RangeExclusive => 20,
            Or => 30,
            And => 40,
            Equals | NotEquals => 50,
            LT | LE | GT | GE => 60,
            Add | Sub => 70,
            Mul | Div | Mod => 80,
        }
    }
    pub fn is_boolean(self) -> bool {
        matches!(self, Operator::Or | Operator::And)
    }
    pub fn is_logical(self) -> bool {
        matches!(
            self,
            Operator::Equals
                | Operator::NotEquals
                | Operator::LT
                | Operator::GT
                | Operator::LE
                | Operator::GE
        )
    }
}
impl fmt::Display for Operator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Operator::Add => "+",
            Operator::Sub => "-",
            Operator::Mul => "*",
            Operator::Div => "/",
            Operator::Mod => "%",
            Operator::Assignment(AssignType::Assign) => "=",
            Operator::Assignment(AssignType::AddAssign) => "+=",
            Operator::Assignment(AssignType::SubAssign) => "-=",
            Operator::Assignment(AssignType::MulAssign) => "*=",
            Operator::Assignment(AssignType::DivAssign) => "/=",
            Operator::Assignment(AssignType::ModAssign) => "%=",
            Operator::Or => "or",
            Operator::And => "and",
            Operator::Equals => "==",
            Operator::NotEquals => "!=",
            Operator::LT => "<",
            Operator::GT => ">",
            Operator::LE => "<=",
            Operator::GE => ">=",
            Operator::Range => "..",
            Operator::RangeExclusive => "..<",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignType {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
}
