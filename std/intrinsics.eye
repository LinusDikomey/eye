
# this is the function responsible for calling compiler intrinsics.
# All available intrinsics are defined in this file via this function.
# Calling this with anything other than a string literal or an unknown intrinsic *will* crash the
# compiler.
intrinsic :: fn[T](s str, ...) -> T extern

eq :: fn[T](l T, r T) -> bool: intrinsic("eq", l, r)
rotate_left :: fn[T](x T, rot T) -> T: intrinsic("rotate_left", x, rot)
xor :: fn[T](x T, y T) -> T: intrinsic("xor", x, y)
