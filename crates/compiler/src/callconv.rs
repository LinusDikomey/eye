#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CallConv {
    /// Standard eye calling convention
    #[default]
    Eye,
    /// Used for the `Fn` trait call method where the arguments are passed as a single tuple
    FnTrait,
}
