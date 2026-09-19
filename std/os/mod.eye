
# this must be kept in sync with the definition in the compiler
# (which just hardcodes the enum ordinals for now)
Os :: enum {
  None
  Linux
  Windows
  Darwin
}

OS : Os : root.intrinsics.intrinsic("os")

is_comptime :: fn -> bool: root.intrinsics.intrinsic("is_comptime")


write_stdout : fn(str) : match OS {
  # TODO: error here (currently erroring involves write_stdout)
  .None: unix.write_stdout,
  # TODO: or patterns would be useful here
  .Linux: unix.write_stdout,
  .Darwin: unix.write_stdout,
  .Windows: windows.write_stdout,
}


os_not_implemented :: fn -> Never: panic("The current os doesn't implement this")
