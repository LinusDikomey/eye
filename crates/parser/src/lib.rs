pub mod ast;
mod parse;
mod tokenize;

use dmap::DHashMap;

use error::{CompileError, Error, Errors, span::TSpan};
use token::{ExpectedTokens, Token};
use tokenize::Tokenizer;

use crate::ast::{Definition, TreeToken};

fn unexpected(token: Token, expected: ExpectedTokens) -> CompileError {
    CompileError {
        err: Error::UnexpectedToken {
            expected,
            found: token.ty,
        },
        span: TSpan::new(token.start, token.end),
    }
}

pub fn parse<T: TreeToken>(
    source: Box<str>,
    errors: &mut Errors,
    definitions: DHashMap<String, Definition<T>>,
) -> ast::Ast<T> {
    let mut ast_builder = ast::AstBuilder::new();
    let mut parser = Parser {
        ast: &mut ast_builder,
        toks: Tokenizer::new(&source, errors),
    };

    let scope = parser.parse_module(definitions);
    ast_builder.finish_with_top_level_scope(source, scope)
}

type ParseResult<T> = Result<T, CompileError>;

struct Parser<'a, T: TreeToken> {
    ast: &'a mut ast::AstBuilder<T>,
    toks: tokenize::Tokenizer<'a>,
}

/// Represents the necessity of delimiters in delimited lists
#[derive(Debug)]
pub enum Delimit {
    /// Require a delimiter between elements
    Yes,
    /// Don't expect a delimiter
    No,
    /// The delimiter may be omitted
    Optional,
    /// delimiter may be omitted if the next entry starts on a new line
    OptionalIfNewLine,
}
impl From<()> for Delimit {
    fn from((): ()) -> Self {
        Self::Yes
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{Ast, Definition, Expr};
    use error::Errors;

    fn test_parse(s: &str) -> Ast {
        let mut errors = Errors::new();
        let ast = super::parse(s.into(), &mut errors, dmap::new());
        assert_eq!(errors.error_count(), 0);
        ast
    }

    #[test]
    fn parse_function() {
        let ast = test_parse("f :: fn { x := 3 }");
        let scope = &ast[ast.top_level_scope_id()];
        assert_eq!(scope.definitions.len(), 1);
        let Definition::Expr(f_def) = scope.definitions["f"] else {
            panic!("expected definition f");
        };
        let Expr::Function { id } = ast[ast[f_def].value] else {
            panic!("expected function definition");
        };
        let body = ast[id].body.unwrap();
        let Expr::Block { scope, items, .. } = ast[body] else {
            panic!("expected body");
        };
        assert_eq!(items.count, 1);
        assert_eq!(ast[scope].definitions.len(), 0);
    }
}
