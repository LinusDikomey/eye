
puts :: fn(s *i8) -> i32 extern
printf :: fn(fmt *i8, ...) -> i32 extern
snprintf :: fn(buffer *i8, max_len u64, fmt *i8, ...) -> i32 extern
scanf :: fn(fmt *i8, ...) -> i32 extern
strcmp :: fn(str1 *i8, str2 *i8) -> i32 extern
strlen :: fn(str *i8) -> i32 extern

malloc :: fn(size u64) -> *u8 extern
free :: fn(ptr *u8) extern
memcpy :: fn(dest *u8, src *u8, n u64) extern

atoi :: fn(s *i8) -> i32 extern


FILE :: struct {
  invalid :: fn -> *FILE: root.null()
}


fopen :: fn(path *i8, mode *i8) -> *FILE extern
fclose :: fn(handle *FILE) -> i32 extern
fread :: fn(ptr *i8, size u64, count u64, stream *FILE) -> u64 extern

sleep :: fn(seconds u32) extern
usleep :: fn(usecs u64) extern

time :: fn(time *i64) -> i64 extern

rand :: fn -> i32 extern
srand :: fn(seed u32) extern

exit :: fn(status i32) -> Never extern
