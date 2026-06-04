use std.call.Fn

main :: fn {
  println(once(Double()))
  println(twice(Double()))
  # TODO: this doesn't work for now
  # println(twice_generic(Double(), 5))
}

once :: fn[F: Fn[(i32), i32]](f F) -> i32: f(123)
twice :: fn[F: Fn[(i32), i32]](f F) -> i32: f(f(123))

twice_generic :: fn[T, F: Fn[(T), T]](f F, initial T) -> T: f(f(initial))

double :: Double()
Double :: struct {
  impl Fn[(i32), i32] {
    call :: @callconv(fn_trait) fn(self Double, args (i32)) -> i32: args.0 * 2
  }
}
