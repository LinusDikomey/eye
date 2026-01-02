use std.call.Fn

times_two :: fn(x i32) -> i32: 2 * x

main :: fn {
  x := call_twice_and_print_size(times_two)
  println(x)
}

call_twice_and_print_size :: fn[F: Fn[(i32), i32]](f F) -> i32 {
  println(F.size)
  ret f(3) + f(4)
}
