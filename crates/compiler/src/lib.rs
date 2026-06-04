#![deny(unused_must_use)]
#![allow(
    clippy::new_without_default,
    clippy::too_many_arguments,
    clippy::wrong_self_convention
)]

/// Calling convention declarations
pub mod callconv;
/// typechecking and emitting hir from the ast
pub mod check;
/// the query-based compiler able to answer and cache various requests
pub mod compiler;
/// compiler errors and error formatting
pub mod error;
/// compile-time code evaluation
mod eval;
/// various helpers
mod helpers;
/// high-level intermediate representation that knows type information and resolved identifiers
pub mod hir;
/// hir->ir lowering
mod irgen;
/// Memory layout of types
mod layout;
/// call the system linker
mod link;
/// module file path discovery
mod modules;
/// find the path of the std library
pub mod std_path;
mod types;
/// TypeTable data structure and logic for type inference/checking
pub mod typing;

pub use compiler::{Compiler, Def, ModuleSpan, ProjectError};
pub use link::link;
pub use modules::all_project_files_from_root;
pub use types::{FunctionType, InvalidTypeError, Type};

id::id!(ProjectId);
impl Default for ProjectId {
    fn default() -> Self {
        Self::MISSING
    }
}
