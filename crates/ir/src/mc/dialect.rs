use crate::{Parameter, Usage, instructions};

instructions! {
    Mc "mc" McInsts

    IncomingBlockArgs !varargs = Some(Parameter::MCReg(Usage::DefUse));

    /// usage is special-cased in register allocator, used in pairs where each first register
    /// is assigned to the second one.
    Copy !varargs = Some(Parameter::MCReg(Usage::Use));

    /// Same as Copy but with the target registers being block arguments
    AssignBlockArgs !varargs = Some(Parameter::MCReg(Usage::Use));

    /// Represents a value stored on the stack by referencing a stack slot. Note that this is
    /// similar to mem.Decl but with a concrete stack slot assigned. It may also hold abi
    /// parameters and spilled values.
    /// Since this exists across instruction selection, it may be both used as an SSA Ref or
    /// by referencing the created `addr` vreg. Both reference the same value
    StackValue addr: MCReg(Usage::Def) slot: Int32 !pure;
}
