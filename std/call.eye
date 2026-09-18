
Fn :: trait[Args, Output] {
  call :: @callconv(fn_trait) fn(this Self, args Args) -> Output
} for {
    impl[T, U] _[T, U] for fn T -> U {
        call :: @callconv(fn_trait) fn(this fn T -> U, args T) -> U {
            root.intrinsics.intrinsic("call_ptr", this, args)
        }
    }
}
