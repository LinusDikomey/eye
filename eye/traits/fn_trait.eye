use std.call.Fn

main :: fn {
  foo := Foo(x: 5)
  x := foo("bar", 1.5)
  println((x * 3.0) as i32)
}

Foo :: struct {
  x i32

  impl Fn[(str, f32), f32] {
    call :: @callconv(fn_trait) fn(self Foo, args (str, f32)) -> f32 {
      print("Called Foo(")
      print(self.x)
      print(") with ")
      println(args.0)
      ret self.x as f32 + args.1
    }
  }
}

