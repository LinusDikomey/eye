// TODO
//use crate::parser::{ExpectedTokens, token::TokenType};
use color_format::*;
use core::fmt;
use span::TSpan;
use std::fmt::Write;
use std::{
    iter::{Enumerate, Peekable},
    path::Path,
    str::Lines,
};
use token::{ExpectedTokens, TokenType};

pub mod span;

#[derive(Debug, Default)]
pub struct Errors {
    pub errors: Vec<CompileError>,
    pub warnings: Vec<CompileError>,
}
impl Errors {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&mut self, errors: Self) {
        self.errors.extend(errors.errors);
        self.warnings.extend(errors.warnings);
    }

    pub fn emit_span(&mut self, err: Error, span: TSpan) {
        self.emit_err(CompileError { err, span });
    }

    pub fn emit_err(&mut self, err: CompileError) {
        let list = match err.err.severity() {
            Severity::Error => &mut self.errors,
            Severity::Warn => &mut self.warnings,
        };
        list.push(err);
    }

    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    pub fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    pub fn get_errors(&self) -> &[CompileError] {
        &self.errors
    }

    pub fn get_warnings(&self) -> &[CompileError] {
        &self.warnings
    }
}

#[derive(Debug, Clone)]
pub struct CompileError {
    pub err: Error,
    pub span: TSpan,
}
impl CompileError {
    pub fn new(err: Error, span: TSpan) -> Self {
        Self { err, span }
    }
}

#[derive(Debug, Clone)]
pub enum Error {
    Internal(String),
    FileSizeExceeeded,
    UnexpectedEndOfFile {
        expected: ExpectedTokens,
    },
    UnexpectedCharacters,
    UnexpectedToken {
        expected: ExpectedTokens,
        found: TokenType,
    },
    UnusedAttributes,
    UnknownIdent(Box<str>),
    UnknownType,
    UnknownFunction,
    UnknownVariable,
    UnknownModule,
    MissingMain,
    MainIsNotAFunction,
    MainArgs,
    MainGenerics,
    InvalidMainReturnType(String),
    UnexpectedType,
    IntLiteralOutOfRange,
    FloatLiteralOutOfRange,
    MultipleDotsInFloatLiteral,
    UseOfUnassignedVariable,
    MissingReturnValue,
    DuplicateDefinition,
    InvalidTopLevelBlockItem,
    UnknownEscapeCode,
    TypeExpected,
    ModuleExpected,
    FunctionOrTypeExpected,
    TraitExpected,
    IntExpected,
    FloatExpected,
    MismatchedType {
        expected: String,
        found: String,
    },
    ExpectedVarFoundDefinition,
    ExpectedValue,
    ExpectedValueFoundDefinition,
    ExpectedValueFoundFunction,
    ExpectedValueOrModuleFoundDefiniton,
    ExpectedValueFoundHole,
    InvalidArgCount {
        expected: u32,
        varargs: bool,
        found: u32,
    },
    NonexistantNamedArg,
    MissingNamedArgs {
        names: Box<[Box<str>]>,
    },
    CantNegateType,
    NonexistantMember(Option<MemberHint>),
    NonexistantEnumVariant,
    TypeMustBeKnownHere {
        needed_bound: Option<String>,
    },
    MissingMainFile,
    CantAssignTo,
    CantTakeRef,
    CantDeref,
    CantUseRootPath,
    InferredTypeNotAllowedHere,
    ArrayTooLarge,
    ArraySizeCantBeInferredHere,
    InvalidArgumentCountForArrayIndex,
    TupleIndexingOnNonValue,
    TupleIndexOutOfRange {
        member_count: u32,
    },
    NotAnInstanceMethod,
    InvalidGenericCount {
        expected: u8,
        found: u8,
    },
    UnexpectedGenerics,
    NotConst,
    FunctionOrStructTypeExpected,
    RecursiveDefinition,
    CantIndex,
    ExpectedConstValue,
    UnusedExpressionValue,
    InfiniteLoop,
    NotAPattern {
        coming_soon: bool,
    },
    NotAPatternRangeValue,
    Inexhaustive,
    DuplicateDependency(String),
    TooManyGenerics(usize),
    HoleLHSOnly,
    CantMutateHole,
    InvalidGlobalVarPattern,
    NotATraitMember {
        trait_name: String,
        function: String,
    },
    NotAllFunctionsImplemented {
        unimplemented: Vec<String>,
    },
    TraitSignatureMismatch(ImplIncompatibility),
    UnsatisfiedTraitBound {
        trait_name: Box<str>,
        ty: String,
    },
    TypeAnnotationNeeded {
        bound: Box<str>,
    },
    InvalidCast {
        from: String,
        to: String,
    },
    TrivialCast,
    EvalFailed(ir::eval::Error),
    EvalReturnedStackPointer,
    CantInferFromBody {
        ty: String,
    },
    NameExpected,
    BreakOutsideLoop,
    ContinueOutsideLoop,
    InvalidCallConv,
}
impl Error {
    pub fn conclusion(&self) -> &'static str {
        match self {
            Error::Internal(_) => "internal compiler error",
            Error::FileSizeExceeeded => "maximum file size exceeded",
            Error::UnexpectedEndOfFile { .. } => "unexpected end of file",
            Error::UnexpectedCharacters => "unexpected characters",
            Error::UnexpectedToken { .. } => "token was not expected here",
            Error::UnknownIdent(_) => "identifier not found",
            Error::UnusedAttributes => "unused attributes",
            Error::UnknownType => "type not found",
            Error::UnknownFunction => "function not found",
            Error::UnknownVariable => "variable not found",
            Error::UnknownModule => "module not found",
            Error::MissingMain => "no main function provided",
            Error::MainIsNotAFunction => "main is not a function",
            Error::MainArgs => "the main function shouln't take arguments",
            Error::MainGenerics => "the main function shouln't be generic",
            Error::InvalidMainReturnType(_) => "invalid main return type",
            Error::UnexpectedType => "type was not expected here",
            Error::IntLiteralOutOfRange => "int literal out of range",
            Error::FloatLiteralOutOfRange => "float literal out of range",
            Error::MultipleDotsInFloatLiteral => "multiple dots in float literal",
            Error::UseOfUnassignedVariable => "variable is uninitialized here",
            Error::MissingReturnValue => "missing return value",
            Error::DuplicateDefinition => "duplicate definition",
            Error::InvalidTopLevelBlockItem => "block item found in top level scope",
            Error::UnknownEscapeCode => "invalid escape code",
            Error::TypeExpected => "expected a type",
            Error::ModuleExpected => "expected a module",
            Error::FunctionOrTypeExpected => "expected a function or type",
            Error::TraitExpected => "expected a trait",
            Error::IntExpected => "integer expected",
            Error::FloatExpected => "float expected",
            Error::MismatchedType { .. } => "type mismatch",
            Error::ExpectedVarFoundDefinition => "expected variable but found a definition",
            Error::ExpectedValue => "a value was expected",
            Error::ExpectedValueFoundDefinition => "expected value but found a definition",
            Error::ExpectedValueFoundFunction => "expected value but found a function",
            Error::ExpectedValueOrModuleFoundDefiniton => {
                "expected value or module but found a definition"
            }
            Error::ExpectedValueFoundHole => "expected a value but found a hole",
            Error::InvalidArgCount { .. } => "invalid argument count",
            Error::NonexistantNamedArg => "this named argument doesn't exist",
            Error::MissingNamedArgs { names } => {
                if names.len() == 1 {
                    "missing named argument"
                } else {
                    "missing named arguments"
                }
            }
            Error::CantNegateType => "can't negate this value",
            Error::NonexistantMember(_) => "member doesn't exist",
            Error::NonexistantEnumVariant => "enum variant doesn't exist",
            Error::TypeMustBeKnownHere { .. } => "type of value must be known here",
            Error::MissingMainFile => "no main.eye file found",
            Error::CantAssignTo => "not assignable",
            Error::CantTakeRef => "can't take reference of value",
            Error::CantDeref => "can't dereference value",
            Error::CantUseRootPath => "root path can't be used here",
            Error::InferredTypeNotAllowedHere => "type can't be inferred here",
            Error::ArrayTooLarge => "maximum array size exceeded",
            Error::ArraySizeCantBeInferredHere => "can't infer array size here",
            Error::InvalidArgumentCountForArrayIndex => "single argument for array index expected",
            Error::TupleIndexingOnNonValue => "indexing a non-tuple",
            Error::TupleIndexOutOfRange { .. } => "tuple doesn't have this many items",
            Error::NotAnInstanceMethod => "not an instance method",
            Error::InvalidGenericCount { .. } => "invalid generic count",
            Error::UnexpectedGenerics => "no generics expected",
            Error::NotConst => "can't evaluate constantly",
            Error::FunctionOrStructTypeExpected => "expected a struct type or a function",
            Error::RecursiveDefinition => "definition depends on itself recursively",
            Error::CantIndex => "can't index this",
            Error::ExpectedConstValue => "constant value expected",
            Error::UnusedExpressionValue => "unused expression value",
            Error::InfiniteLoop => "possibly detected infinite loop",
            Error::NotAPattern { .. } => "not a pattern",
            Error::NotAPatternRangeValue => "can't be used for ranges in patterns",
            Error::Inexhaustive => "not all possible values were covered",
            Error::DuplicateDependency(_) => "duplicate dependency",
            Error::TooManyGenerics(_) => "too many generics",
            Error::HoleLHSOnly => "hole can only used on left-hand side of an assignment",
            Error::CantMutateHole => "can't mutate hole",
            Error::InvalidGlobalVarPattern => "invalid pattern for a global variable definition",
            Error::NotATraitMember { .. } => "not a member of the implemented trait",
            Error::NotAllFunctionsImplemented { .. } => {
                "not all functions of the trait are implemented"
            }
            Error::TraitSignatureMismatch(_) => {
                "signature doesn't match the function's signature in the trait definition"
            }
            Error::UnsatisfiedTraitBound { .. } => "unsatisfied trait bound",
            Error::TypeAnnotationNeeded { .. } => "type annotation needed",
            Error::InvalidCast { .. } => "this cast is not valid",
            Error::TrivialCast => "this cast is trivial",
            Error::EvalFailed(ir::eval::Error::InfiniteLoop) => {
                "evaluation failed due to an infinite loop"
            }
            Error::EvalFailed(ir::eval::Error::ExternCallFailed(_)) => {
                "evaluation failed while calling an extern function"
            }
            Error::EvalFailed(ir::eval::Error::StackOverflow) => {
                "evaluation failed due to a stack overflow"
            }
            Error::EvalFailed(ir::eval::Error::ProvenanceViolation) => {
                "evaluation failed due to a provenance violation"
            }
            Error::EvalFailed(ir::eval::Error::OutOfMemory) => {
                "evaluation failed by running out of memory"
            }
            Error::EvalReturnedStackPointer => {
                "evaluation returned an invalid pointer to the stack"
            }
            Error::NameExpected => "expected a name",
            Error::CantInferFromBody { .. } => "type can't be inferred from function body",
            Error::BreakOutsideLoop => "break used outside a loop",
            Error::ContinueOutsideLoop => "continue used outside a loop",
            Error::InvalidCallConv => "invalid calling convention",
        }
    }
    pub fn details(&self) -> Option<String> {
        Some(match self {
            Error::Internal(msg) => cformat!(
                "#r<internal compiler error> #y!<(this is a bug and not your fault)>: {}",
                msg,
            ),
            Error::UnexpectedEndOfFile { expected } => cformat!("expected {} instead", expected,),
            Error::UnexpectedToken { expected, found } => {
                cformat!("expected {} but found {}", expected, found)
            }
            Error::UnknownIdent(name) => cformat!("the identifier #c<{name}> was not found"),
            Error::ExpectedValueFoundHole => {
                cformat!("the name #r<_> is reserved for values that are thrown away")
            }
            Error::InvalidMainReturnType(ty) => cformat!(
                "the main function should return either an integer or the unit type #m<()> but returns {}",
                ty
            ),
            &Error::TupleIndexOutOfRange { member_count } => {
                cformat!(
                    "Tuple only has #c<{member_count}> positional member{}",
                    if member_count == 1 { "" } else { "s" }
                )
            }
            Error::MismatchedType { expected, found } => {
                cformat!(
                    "expected value of type #m<{}> but found #m<{}>",
                    expected,
                    found
                )
            }
            &Error::InvalidArgCount {
                expected,
                varargs,
                found,
            } => {
                cformat!(
                    "expected #g<{}{}> argument{} but #r<{}> {} found",
                    if varargs { "at least " } else { "" },
                    expected,
                    if expected == 1 { "" } else { "s" },
                    found,
                    if found == 1 { "was" } else { "were" }
                )
            }
            Error::MissingNamedArgs { names } => {
                let mut text = "expected arguments ".to_owned();
                for (i, name) in names.iter().enumerate() {
                    if i != 0 {
                        if i + 1 == names.len() {
                            text.push_str(" and ");
                        } else {
                            text.push_str(", ");
                        }
                    }
                    cwrite!(text, "#c<{name}>").unwrap();
                }
                text
            }
            Error::NonexistantMember(Some(hint)) => cformat!("#c<hint>: {}", hint.hint()),
            Error::TypeMustBeKnownHere {
                needed_bound: Some(bound),
            } => {
                cformat!("#c<hint>: a type satisfying #m<{bound}> is needed")
            }
            Error::InvalidGenericCount { expected, found } => cformat!(
                "expected #y<{}> parameters but found #r<{}>",
                expected,
                found
            ),
            Error::UnusedExpressionValue => {
                cformat!("this expression only produces a value that is not used")
            }
            Error::NotAPattern { coming_soon } => {
                if *coming_soon {
                    cformat!("this might be a valid pattern #c<soon>")
                } else {
                    cformat!("this expression is not a valid pattern")
                }
            }
            Error::DuplicateDependency(name) => {
                cformat!("multiple dependencies named '#m<{}>'", name)
            }
            Error::TooManyGenerics(count) => cformat!(
                "the maximum number of generics allowed is #g<255> but found #r<{}>",
                count
            ),
            Error::HoleLHSOnly => {
                cformat!("#r<_> is not a value and can only be used to ignore the value")
            }
            Error::CantMutateHole => cformat!("#r<_> can only be assigned to"),
            Error::InvalidGlobalVarPattern => cformat!(
                "only a single #c<identifier> can be used as a pattern when creating global variables"
            ),
            Error::NotATraitMember {
                trait_name,
                function,
            } => cformat!(
                "the function #m<{function}> is not a member of the trait #m<{trait_name}>",
            ),
            Error::NotAllFunctionsImplemented { unimplemented } => {
                assert!(!unimplemented.is_empty());
                if unimplemented.len() == 1 {
                    cformat!("missing function: #m<{}>", unimplemented[0])
                } else {
                    let mut it = unimplemented.iter();
                    let mut s = format!("missing functions: #m<{}>", it.next().unwrap());
                    for f in it {
                        use std::fmt::Write;
                        cwrite!(&mut s, ", #m<{f}>").unwrap();
                    }
                    s
                }
            }
            Error::TraitSignatureMismatch(incompat) => match incompat {
                ImplIncompatibility::Generics => "the generics differ".to_owned(),
                ImplIncompatibility::VarargsNeeded => cformat!("#c<hint>: add varargs"),
                ImplIncompatibility::NoVarargsNeeded => cformat!("#c<hint>: remove the varargs"),
                ImplIncompatibility::ArgCount { base, impl_ } => {
                    cformat!("there are #r<{impl_}> arguments but #r<{base}> in the trait function")
                }
                ImplIncompatibility::Arg(i) => cformat!("argument #r<{i}> has the wrong type"), // TODO: concrete type mismatch errors
                ImplIncompatibility::ReturnType => "the return type is different".to_owned(),
                ImplIncompatibility::CallConv => "the calling convention is different".to_owned(),
            },
            Error::UnsatisfiedTraitBound { trait_name, ty } => {
                cformat!("the type #m<{ty}> doesn't satisfy the #m<{trait_name}> trait")
            }
            Error::TypeAnnotationNeeded { bound } => {
                cformat!("a type satisfying #c<{bound}> is needed")
            }
            Error::InvalidCast { from, to } => {
                cformat!("cannot cast from a value of type #m<{from}> to #m<{to}>")
            }
            Error::TrivialCast => {
                cformat!("#c<hint>: remove this cast")
            }
            Error::EvalFailed(ir::eval::Error::InfiniteLoop) => {
                cformat!(
                    "#c<hint>: configuring the backwards jump limit \
                    (currently always #blue<{}>) will be configurable in the future",
                    ir::eval::BACKWARDS_JUMP_LIMIT
                )
            }
            Error::EvalFailed(ir::eval::Error::ExternCallFailed(s)) => {
                cformat!("#r<error>: {s}")
            }
            Error::CantInferFromBody { ty } => cformat!("got: {ty}"),
            _ => return None,
        })
    }
    pub fn severity(&self) -> Severity {
        match self {
            Self::UnusedExpressionValue | Self::CantMutateHole | Self::TrivialCast => {
                Severity::Warn
            }
            _ => Severity::Error,
        }
    }
}
impl Error {
    pub fn at(self, start: u32, end: u32) -> CompileError {
        CompileError {
            err: self,
            span: TSpan::new(start, end),
        }
    }
    pub fn at_span(self, span: TSpan) -> CompileError {
        CompileError { err: self, span }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warn,
}
impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => cwrite!(f, "#r!<error>"),
            Self::Warn => cwrite!(f, "#y!<warn>"),
        }
    }
}

pub fn print(error: &Error, span: TSpan, src: &str, file: &Path) {
    if span == TSpan::MISSING {
        match error.severity() {
            Severity::Error => cprintln!("#r<error>: #r!<{}>", error.conclusion()),
            Severity::Warn => cprintln!("#y<warn>: #y!<{}>", error.conclusion()),
        }
        cprintln!("#c<at>: #u<{}>:?", file.display());

        if let Some(details) = error.details() {
            cprintln!("  {details}");
        }
        return;
    }
    let s = span.start as usize;
    let e = span.end as usize;

    let pos = calculate_position(src, span.start);

    let src_loc: std::borrow::Cow<str> = if e >= src.len() {
        if s >= src.len() {
            "[no file location]".into()
        } else {
            (&src[s..]).into()
        }
    } else {
        (&src[s..e]).into()
    };

    let pre = &src[pos.start_of_line_byte as usize..s];

    // find end of the line to print the rest of the line
    let post = if e == src.len() || src.as_bytes()[e] == b'\n' {
        ""
    } else {
        let mut surrounding_end = std::cmp::min(src.len(), e);
        while surrounding_end < src.len() && src.as_bytes()[surrounding_end] != b'\n' {
            surrounding_end += 1;
        }
        &src[e..surrounding_end]
    };

    cprint!("{}: ", error.severity());
    match error.severity() {
        Severity::Error => cprintln!("#r!<{}>", error.conclusion()),
        Severity::Warn => cprintln!("#y!<{}>", error.conclusion()),
    }
    cprintln!(
        "#c<at>: #u<{}:{}:{}>",
        file.to_string_lossy(),
        pos.line + 1,
        pos.column + 1
    );
    let spaces = std::cmp::max(
        4,
        (pos.line + src_loc.lines().count() as u32)
            .to_string()
            .len(),
    );
    let p = cformat!("{} #c<|> ", " ".repeat(spaces));

    println!("{p}");

    let mut lines = src_loc.lines().enumerate().peekable();

    let first = lines.next().unwrap_or((s, ""));

    let post_if_last = |lines: &mut Peekable<Enumerate<Lines>>| {
        if lines.peek().is_some() {
            println!();
        } else {
            println!("{post}");
        }
    };

    cprint!("#c<{:#4} | >{}{}", pos.line + 1, pre, first.1);
    post_if_last(&mut lines);

    let pre_error_offset = " ".repeat(pre.chars().count());
    match error.severity() {
        Severity::Error => cprintln!(
            "{}{}#r!<{}>",
            p,
            pre_error_offset,
            "^".repeat(first.1.chars().count())
        ),
        Severity::Warn => cprintln!(
            "{}{}#y!<{}>",
            p,
            pre_error_offset,
            "^".repeat(first.1.chars().count())
        ),
    }
    if let Some(details) = error.details() {
        cprintln!("{}{}{}", p, pre_error_offset, details);
    }

    while let Some((i, line_str)) = lines.next() {
        let line = pos.line + 1 + i as u32;

        cprint!("#c<{:#4} | >{}", line, line_str);
        post_if_last(&mut lines);

        match error.severity() {
            Severity::Error => cprintln!("{}#r!<{}>", p, "^".repeat(line_str.chars().count())),
            Severity::Warn => cprintln!("{}#y!<{}>", p, "^".repeat(line_str.chars().count())),
        }
    }
    println!();
}

pub struct Position {
    /// zero-indexed line
    pub line: u32,
    /// zero-indexed column in the line
    pub column: u32,
    /// index of the start of the line the position is located in
    pub start_of_line_byte: u32,
}

pub fn calculate_position(src: &str, position: u32) -> Position {
    // calculate line and position in line
    let until_start = if position as usize >= src.len() {
        src
    } else {
        &src[..position as usize]
    };
    let mut line = 0;
    let mut column = 0;
    let mut start_of_line_byte = 0;
    for (i, c) in until_start.char_indices() {
        if c == '\n' {
            line += 1;
            column = 0;

            start_of_line_byte = i as u32 + 1;
        } else {
            column += 1;
        }
    }
    Position {
        line,
        column,
        start_of_line_byte,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum MemberHint {
    InferredEnum,
    TupleOfLength(u32),
}
impl MemberHint {
    fn hint(&self) -> String {
        match self {
            Self::InferredEnum => "you are trying to access a member of an inferred enum, \
                you might have to annotate the type of an explicit enum to be able to access \
                it's members"
                .to_owned(),
            Self::TupleOfLength(n) => {
                cformat!("the tuple you are accessing only has #b<{n}> elements")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum ImplIncompatibility {
    Generics,
    VarargsNeeded,
    NoVarargsNeeded,
    ArgCount { base: u32, impl_: u32 },
    Arg(u32),
    ReturnType,
    CallConv,
}
