main :: fn {
  z: fn(i64) -> i64 = fn(x): x + 2
  println(z(3))
  println(g(z))
}

g :: fn[F: std.call.Fn[(i64), i64]](a F) -> i64: a(1) + a(2)
