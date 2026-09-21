use compiler::{
    Compiler, Type,
    compiler::{Generics, ResolvedTypeContent, Signature},
    types::{BaseType, TypeFull},
};
use std::fmt::Write;

pub fn display_signature(compiler: &Compiler, name: &str, signature: &Signature) -> String {
    let mut s = format!("```eye\n{name} :: fn(");
    let mut first = true;
    for (name, ty) in &signature.params {
        if first {
            first = false;
        } else {
            s.push_str(", ");
        }

        write!(
            s,
            "{name} {}",
            compiler.display_type(*ty, &signature.generics)
        )
        .unwrap();
    }
    for (name, ty, _default) in &signature.named_params {
        if first {
            first = false;
        } else {
            s.push_str(", ");
        }

        write!(
            s,
            "{name} {} = ...",
            compiler.display_type(*ty, &signature.generics)
        )
        .unwrap();
    }
    write!(
        s,
        ") -> {}\n```",
        compiler.display_type(signature.return_type, &signature.generics)
    )
    .unwrap();
    s
}

pub fn ty(compiler: &Compiler, ty: Type) -> String {
    match compiler.types.lookup(ty) {
        TypeFull::Instance(base, &[]) => base_type(compiler, base),
        TypeFull::Instance(base, instance) => {
            let name = &compiler.types.get_base(base).name;
            let mut s = format!("```eye\n{name}[");
            for &ty in instance {
                write!(s, "{}", compiler.display_type(ty, &Generics::EMPTY)).unwrap();
            }
            s.push_str("\n```");
            s
        }
        // TODO: display function signature
        // TypeFull::FunctionItem { function, generics } => todo!(),
        _ => format!(
            "```eye\n{}\n```",
            compiler.display_type(ty, &Generics::EMPTY)
        ),
    }
}

pub fn base_type(compiler: &Compiler, base: BaseType) -> String {
    let ty = compiler.types.get_base(base);
    let def = match compiler.get_base_type_def(base).def {
        ResolvedTypeContent::Builtin(_) => "",
        ResolvedTypeContent::Struct(_) => " :: struct { ... }",
        ResolvedTypeContent::Enum(_) => " :: enum { ... }",
    };
    format!("```eye\n{}{def}\n```", ty.name)
}
