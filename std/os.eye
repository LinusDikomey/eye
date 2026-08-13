
# this must be kept in sync with the definition in the compiler
# (which just hardcodes the enum ordinals for now)
Os :: enum {
  None
  Linux
  Windows
  Darwin
}

OS : Os : root.intrinsics.intrinsic("os")
