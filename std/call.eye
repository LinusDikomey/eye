
Fn :: trait[Args, Output] {
  call :: @callconv(fn_trait) fn(this Self, args Args) -> Output
} for {
    # single-argument fns only for now, change the syntax of fn types so that this is possible
    # to implement generically
    impl[T, U] _[(T), U] for fn(T) -> U {
        call :: @callconv(fn_trait) fn(this fn(T) -> U, args (T)) -> U: this(args.0)
    }
}
