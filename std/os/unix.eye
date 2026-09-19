STDOUT : i32 : 1

write :: fn(fd i32, buf *u8, count usize) -> isize extern


write_stdout :: fn(s str): write(STDOUT, s.ptr, s.len)
