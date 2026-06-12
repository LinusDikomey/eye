use error::Errors;

/// Convert to dom
mod convert;
/// Representation of a formattable syntax tree
mod dom;
/// Final layouuting and rendering of the dom
mod render;

const LINE_WIDTH: u32 = 100;
const ALLOWED_NEWLINES: u32 = 1;
const ALLOWED_NEWLINES_SCOPE: u32 = 2;

type Cst = parser::ast::Ast<parser::ast::Token>;

pub fn format(src: Box<str>) -> (String, Errors) {
    let mut errors = Errors::new();
    let cst = parser::parse::<parser::ast::Token>(src, &mut errors, dmap::new());
    let dom = convert::module(&cst);
    tracing::debug!(target: "fmt::dom", "Format dom:\n{dom:?}\n");
    (render::render(dom), errors)
}

pub fn render_cst<T: parser::ast::TreeToken>(cst: &Cst) -> String {
    let dom = convert::module(cst);
    render::render(dom)
}
