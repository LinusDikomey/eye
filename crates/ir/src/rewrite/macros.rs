#[macro_export]
macro_rules! arg {
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, _) => {};
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, $var: ident) => {
        let $crate::Argument::Ref($var) = $arg else {
            unreachable!("invalid argument type, expected Ref");
        };
    };
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, $const: literal) => {
        let $crate::Argument::Int(value) = $arg else {
            unreachable!("invalid argument type, expected Int");
        };
        if value != $const {
            return ::core::option::Option::None;
        }
    };
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, (#$int: ident)) => {
        let $crate::Argument::Int($int) = $arg else {
            unreachable!("invalid argument type, expected Int");
        };
    };
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, (float $float: ident)) => {
        let $crate::Argument::Float($float) = $arg else {
            unreachable!("invalid argument type, expected Float");
        };
    };
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, (@ $block: ident $args: ident)) => {
        let $crate::Argument::BlockTarget($crate::BlockTarget($block, $args)) = $arg else {
            unreachable!("invalid argument type, expected BlockTarget");
        };
    };
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, (type $ty: ident)) => {
        let $crate::Argument::TypeId($ty) = $arg else {
            unreachable!("invalid argument type, expected TypeId");
        };
    };
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, (global $global: ident)) => {
        let $crate::Argument::GlobalId($global) = $arg else {
            unreachable!("invalid argument type, expected GlobalId");
        };
    };
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, (fn $function: ident)) => {
        let $crate::Argument::FunctionId($function) = $arg else {
            unreachable!("invalid argument type, expected FunctionId");
        };
    };
    ($modules: ident, $ir: ident, $env: ident, $arg: ident, $($inner: tt)*) => {
        let $crate::Argument::Ref(r) = $arg else {
            unreachable!("invalid argument type, expected Ref");
        };
        if !r.is_ref() {
            return ::core::option::Option::None;
        };
        let inst = $ir.get_inst(r);
        $crate::pattern_ref!($modules, $ir, $env, inst, r, $($inner)*)
    };
}

#[macro_export]
macro_rules! args {
    ($modules: ident, $ir: ident, $env: ident, $args: expr,) => {
        let [] = $args;
    };
    ($modules: ident, $ir: ident, $env: ident, $args: expr, $a: tt) => {
        let [a] = $args;
        $crate::arg!($modules, $ir, $env, a, $a);
    };
    ($modules: ident, $ir: ident, $env: ident, $args: expr, $a: tt $b: tt) => {
        let [a, b] = $args;
        $crate::arg!($modules, $ir, $env, a, $a);
        $crate::arg!($modules, $ir, $env, b, $b);
    };
    ($modules: ident, $ir: ident, $env: ident, $args: expr, $a: tt $b: tt $c: tt) => {
        let [a, b, c] = $args;
        $crate::arg!($modules, $ir, $env, a, $a);
        $crate::arg!($modules, $ir, $env, b, $b);
        $crate::arg!($modules, $ir, $env, c, $c);
    };
    ($modules: ident, $ir: ident, $env: ident, $args: expr, $a: tt $b: tt $c: tt $d: tt) => {
        let [a, b, c, d] = $args;
        $crate::arg!($modules, $ir, $env, a, $a);
        $crate::arg!($modules, $ir, $env, b, $b);
        $crate::arg!($modules, $ir, $env, c, $c);
        $crate::arg!($modules, $ir, $env, b, $d);
    };
    ($modules: ident, $ir: ident, $env: ident, $args: expr, $a: tt $b: tt $c: tt $d: tt $e: tt) => {
        let [a, b, c, d, e] = $args;
        $crate::arg!($modules, $ir, $env, a, $a);
        $crate::arg!($modules, $ir, $env, b, $b);
        $crate::arg!($modules, $ir, $env, c, $c);
        $crate::arg!($modules, $ir, $env, d, $d);
        $crate::arg!($modules, $ir, $env, e, $e);
    };
    ($modules: ident, $ir: ident, $env: ident, $args: expr, $a: tt $b: tt $c: tt $d: tt $e: tt $f: tt) => {
        let [a, b, c, d, e, f] = $args;
        $crate::arg!($modules, $ir, $env, a, $a);
        $crate::arg!($modules, $ir, $env, b, $b);
        $crate::arg!($modules, $ir, $env, c, $c);
        $crate::arg!($modules, $ir, $env, d, $d);
        $crate::arg!($modules, $ir, $env, e, $e);
        $crate::arg!($modules, $ir, $env, f, $f);
    };
}

#[macro_export]
macro_rules! pattern_ref {
    ($modules: ident, $ir: ident, $end: ident, $inst: expr, $r: ident, _) => {};
    ($modules: ident, $ir: ident, $end: ident, $inst: expr, $r: ident, ($(% $out_r: ident = )? _)) => {
        $(
            let $out_r = $r;
        )?
    };
    ($modules: ident, $ir: ident, $env: ident, $inst: expr, $r: ident, ($(% $out_r: ident =)? $module: ident. $matched_inst: ident .. $($arg: tt)*)) => {
        let inst = $inst.as_module($modules.$module)?;
        $(
            let $out_r = $r;
        )?
        if inst.op() != $module::$matched_inst {
            return ::core::option::Option::None;
        }
        $crate::args!($modules, $ir, $env, $ir.args_n_ignore_extra(&inst), $($arg)*);
    };
    ($modules: ident, $ir: ident, $env: ident, $inst: expr, $r: ident, ($(% $out_r: ident =)? $module: ident. $matched_inst: ident $($arg: tt)*)) => {
        let inst = $inst.as_module($modules.$module)?;
        $(
            let $out_r = $r;
        )?
        if inst.op() != $module::$matched_inst {
            return ::core::option::Option::None;
        }
        $crate::args!($modules, $ir, $env, $ir.args_n(&inst), $($arg)*);
    };
}

#[macro_export]
macro_rules! ident_or_ignore {
    ($id: ident) => {
        $id
    };
    () => {
        _
    };
}

#[macro_export]
macro_rules! visitor {
    (
        $rewriter: ident,
        $output: ty,
        $ir: ident,
        $types: ident,
        $inst: ident,
        $block: ident,
        $env: ident,
        $dialects: ident,
        $ctx: ident: $ctx_ty: ty;

        $(use $module: ident: $module_path: path;)*

        patterns:

        $($pattern: tt $(if $cond: expr)? => $(=)? $result: expr;)*
    ) => {
        #[derive(Clone, Copy)]
        pub struct $rewriter {
            $(
                pub $module: $crate::ModuleOf<$module_path>,
            )*
        }
        impl $rewriter {
            pub fn new(env: &mut $crate::Environment) -> Self {
                Self {
                    $(
                        $module: env.get_dialect_module(),
                    )*
                }
            }
        }
        impl $crate::rewrite::Visitor<$ctx_ty> for $rewriter {
            type Output = $output;

            fn visit_instruction(
                &self,
                #[allow(unused)] $ir: &mut $crate::modify::IrModify,
                #[allow(unused)] $types: &$crate::Types,
                $env: &$crate::Environment,
                #[allow(unused)] $inst: &$crate::Instruction,
                r: $crate::Ref,
                $block: $crate::BlockId,
                $ctx: &mut $ctx_ty,
            ) -> ::core::option::Option<$output> {
                let $dialects = &*self;
                $(
                    #[allow(unused)]
                    use $module_path as $module;
                    #[allow(unused)]
                    let $module = self.$module;
                )*
                $(
                    let result = (|| {
                        $crate::pattern_ref!(self, $ir, $env, $inst, r, $pattern);
                        $(
                            let cond: bool = $cond;
                            if !cond {
                                return ::core::option::Option::None;
                            }
                        )*
                        // needed to allow todo!() as () to fix ! erroring on the Visitor impl
                        #[allow(clippy::diverging_sub_expression)]
                        let result = $result;
                        #[allow(unreachable_code)]
                        ::core::option::Option::Some(
                            $crate::rewrite::IntoVisit::into_visit(result, $ir, $env, $block)
                        )
                    })();
                    if let Some(value) = result {
                        return value;
                    }
                )*
                ::core::option::Option::None
            }
        }
    };
}
pub use crate::visitor;
