
# this must be kept in sync with the definition in the compiler
# (which just hardcodes the enum ordinals for now)
Os :: enum {
  None
  Linux
  Windows
  Darwin
}

OS : Os : root.intrinsics.intrinsic("os")


write_stdout : fn(str) : match OS {
  .None: os_not_implemented(),
  # TODO: or patterns would be useful here
  .Linux: unix.write_stdout,
  .Darwin: unix.write_stdout,
  .Windows: windows.write_stdout,
}


os_not_implemented :: fn -> Never: panic("The current os doesn't implement this")
