use crate::{
    Environment, MCReg, ModuleOf, Ref, Refs, Types,
    mc::{Mc, McInst, Register},
    modify::IrModify,
    slots::Slots,
};

pub trait Abi<I: McInst> {
    fn implement_params(
        &self,
        args: Refs,
        ir: &mut IrModify,
        env: &Environment,
        mc: ModuleOf<Mc>,
        i: ModuleOf<I>,
        types: &Types,
        regs: &Slots<MCReg>,
    );
    fn implement_call(
        &self,
        call_inst: Ref,
        ir: &mut IrModify,
        env: &Environment,
        mc: ModuleOf<Mc>,
        i: ModuleOf<I>,
        types: &Types,
        regs: &Slots<MCReg>,
        skip_first_arg: bool,
    );
    fn implement_return(
        &self,
        value: Ref,
        ir: &mut IrModify,
        env: &Environment,
        mc: ModuleOf<Mc>,
        i: ModuleOf<I>,
        types: &Types,
        regs: &Slots<MCReg>,
        r: Ref,
    );
    fn callee_saved(&self) -> <I::Reg as Register>::RegisterBits;
    fn caller_saved(&self) -> <I::Reg as Register>::RegisterBits;
    fn return_regs(&self, value_count: u32) -> <I::Reg as Register>::RegisterBits;
}
