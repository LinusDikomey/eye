use std.call.Fn

main :: fn {
  f :: ReturnOverload()
  a: i32 = f()
  println(a)
  b: str = f()
  println(b)
}

ReturnOverload :: struct {
  @hidden impl Fn[(), i32] {
    call :: @callconv(fn_trait) fn(this ReturnOverload, args ()) -> i32: 5
  }

  @hidden impl Fn[(), str] {
    call :: @callconv(fn_trait) fn(this ReturnOverload, args ()) -> str: "hello"
  }
}
