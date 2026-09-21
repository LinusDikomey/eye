use std::fmt;

use color_format::{cwrite, cwriteln};

use crate::{
    Compiler,
    compiler::{BodyOrTypes, CheckedFunction, Generics},
};

pub struct CheckedFunctionDisplay<'a> {
    pub function: &'a CheckedFunction,
    pub compiler: &'a Compiler,
    pub generics: &'a Generics,
}
impl<'a> fmt::Display for CheckedFunctionDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            function,
            compiler,
            generics,
        } = self;
        cwrite!(f, "BEGIN HIR {}", function.name)?;
        if function.generic_count != 0 {
            cwrite!(f, "[")?;
            for i in 0..function.generic_count {
                if i != 0 {
                    cwrite!(f, ", ")?;
                }
                cwrite!(f, "{}", generics.get_name(i))?;
            }
            cwrite!(f, "]")?;
        }
        cwrite!(f, "(")?;
        for (i, param) in function.params.iter().enumerate() {
            if i != 0 {
                cwrite!(f, ", ")?;
            }
            cwrite!(
                f,
                "#g<${i}>: {}",
                self.compiler.display_type(function[param], generics)
            )?;
        }
        if function.varargs {
            if function.params.count != 0 {
                cwrite!(f, ", ")?;
            }
            cwrite!(f, "...")?;
        }
        cwrite!(
            f,
            ") -{} {}",
            ">",
            self.compiler
                .display_type(function[function.return_type], generics),
        )?; // TODO: problem in color-format: can't escape >
        cwrite!(f, "\n  ")?;
        match &function.body_or_types {
            BodyOrTypes::Body(body) => {
                cwrite!(f, "{}", body.display(body.root_id(), compiler, generics, 1))?;
            }
            BodyOrTypes::Types(_) => {
                cwriteln!(f, "  #m<<extern{}>", ">")?;
            }
        }
        cwriteln!(f, "\nEND HIR\n")
    }
}
