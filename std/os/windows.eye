
HANDLE :: struct {
}
DWORD :: u32
BOOL :: i32
OVERLAPPED :: struct {
}

STD_OUTPUT_HANDLE : DWORD : {
  x: i32 = -11
  x as DWORD
}

GetStdHandle :: fn(nStdHandle DWORD) -> HANDLE extern

WriteFile :: fn(
  hFile HANDLE
  lpBuffer *u8
  nNumberOfBytesToWrite DWORD
  lpNumberOfBytesWritten *DWORD
  lpOverlapped *OVERLAPPED
) -> BOOL extern


write_stdout :: fn(s str) {
  handle := GetStdHandle(STD_OUTPUT_HANDLE)
  while s.len != 0 {
    written := 0
    # FIXME: assumes len fits in u32
    if WriteFile(handle, s.ptr, s.len as u32, &written, root.null()) == 0 {
      panic("Failed to write to stdout")
    }
    written := written as usize
    s.ptr = root.ptr_add(s.ptr, written)
    s.len -= written
  }
}
