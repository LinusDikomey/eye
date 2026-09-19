STDOUT : i32 : 1

write :: fn(fd i32, buf *u8, count usize) -> isize extern


write_stdout :: fn(unix_s str): write(STDOUT, unix_s.ptr, unix_s.len)
