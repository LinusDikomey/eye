use string.str
use option.Option

print :: fn[T: ToString](value T) {
    s := ToString.to_string(&value)
    c.printf("%.*s".ptr as *i8, s.len as i32, s.ptr)
    c.free(s.ptr)
}
println :: fn[T: ToString](value T) {
    s := ToString.to_string(&value)
    c.printf("%.*s\n".ptr as *i8, s.len as i32, s.ptr)
    c.free(s.ptr)
}

skip_char :: fn {
    tmp: u64 = 0
    c.scanf("%c".ptr as *i8, (&tmp) as *i8)
}

readline :: fn -> str {
    line := c.malloc(1024) as *i8
    c.scanf("%1023[^\n]".ptr as *i8, line)
    skip_char()
    ret str.from_cstr(line)
}

input :: fn(msg str) -> str {
    print(msg)
    line := readline()
    ret line
}

int_to_str :: fn(i i32) -> str {
    max_len := 10
    buffer := c.malloc(max_len) as *i8
    c.snprintf(buffer, max_len, "%d".ptr as *i8, i)
    ret str.from_cstr(buffer)
}

parse_int :: fn(s *i8) -> i32 {
    ret c.atoi(s)
}

streq :: fn(a *i8, b *i8) -> bool {
    ret c.strcmp(a, b) == 0
}

Buf :: struct {
    ptr *u8
    size u64
    cap u64
}

buf :: fn(initial_cap u64) -> Buf: Buf(ptr: c.malloc(initial_cap), size: 0, cap: initial_cap)

buf_write :: fn(buf Buf, ptr *u8, len u64) -> Buf {
    if buf.cap - buf.size >= len {
        # enough capacity
        c.memcpy((buf.ptr as u64 + buf.size) as *u8, ptr, len)
        buf.size += len
    } else {
        # not enough capacity, reallocate
        new_size := buf.size + len
        new_cap := new_size
        new_ptr := c.malloc(new_size)
        c.memcpy(new_ptr, buf.ptr, buf.size)
        c.free(buf.ptr)

        c.memcpy((new_ptr as u64 + buf.size) as *u8, ptr, len)

        buf = Buf(ptr: new_ptr, size: new_size, cap: new_cap)
    }
    ret buf
}

panic :: fn(msg str) -> Never {
    c.printf("[PANIC]: %.*s\n".ptr as *i8, msg.len as i32, msg.ptr)
    c.exit(1)
}

assert :: fn(b bool, message str) {
    if !b {
        panic(message)
    }
}

## Adds an offset to the pointer and returns the offset pointer.
## The offset is element-wise, so the number of bytes added will be multiplied by the size of T.
ptr_add :: fn[T](ptr *T, offset u64) -> *T: (ptr as u64 + offset * T.stride) as *T

## returns a null pointer
null :: fn[T] -> *T: 0 as u64 as *T

# TODO: should be generic over the element type and probably work on slices
sort :: fn(l *List[u64]) {
    if l.len < 2: ret
    i := 0
    while i < l.len {
        j := 0
        while j < l.len - 1 - i {
            a := l.get(j)
            b := l.get(j+1)
            if a > b {
                l.put(j+1, a)
                l.put(j, b)
            }
            j += 1
        }
        i += 1
    }
}

Eq :: trait {
    eq :: fn(this Self, other Self) -> bool 
} for {
  impl _ for u8 {
    eq :: fn(this u8, other u8) -> bool: intrinsics.eq(this, other)
  }
  impl _ for u16 {
    eq :: fn(this u16, other u16) -> bool: intrinsics.eq(this, other)
  }
  impl _ for u32 {
    eq :: fn(this u32, other u32) -> bool: intrinsics.eq(this, other)
  }
  impl _ for u64 {
    eq :: fn(this u64, other u64) -> bool: intrinsics.eq(this, other)
  }

  impl _ for i8 {
    eq :: fn(this i8, other i8) -> bool: intrinsics.eq(this, other)
  }
  impl _ for i16 {
    eq :: fn(this i16, other i16) -> bool: intrinsics.eq(this, other)
  }
  impl _ for i32 {
    eq :: fn(this i32, other i32) -> bool: intrinsics.eq(this, other)
  }
  impl _ for i64 {
    eq :: fn(this i64, other i64) -> bool: intrinsics.eq(this, other)
  }
  impl _ for f32 {
    eq :: fn(this f32, other f32) -> bool: intrinsics.eq(this, other)
  }
  impl _ for f64 {
    eq :: fn(this f64, other f64) -> bool: intrinsics.eq(this, other)
  }
  impl _ for bool {
    eq :: fn(this bool, other bool) -> bool: intrinsics.eq(this, other)
  }
  impl _ for type {
    eq :: fn(this type, other type) -> bool: intrinsics.eq(this, other)
  }

  impl[T: Eq, U: Eq] _ for (T, U) {
    eq :: fn(this (T, U), other (T, U)) -> bool {
      ret Eq.eq(this.0, other.0) and Eq.eq(this.1, other.1)
    }
  }
  impl[T: Eq, U: Eq, V: Eq] _ for (T, U, V) {
    eq :: fn(this (T, U, V), other (T, U, V)) -> bool {
      ret Eq.eq(this.0, other.0) and Eq.eq(this.1, other.1) and Eq.eq(this.2, other.2)
    }
  }
  impl[T: Eq, U: Eq, V: Eq, W: Eq] _ for (T, U, V, W) {
    eq :: fn(this (T, U, V, W), other (T, U, V, W)) -> bool {
      ret Eq.eq(this.0, other.0) and Eq.eq(this.1, other.1) and Eq.eq(this.2, other.2) and Eq.eq(
        this.3
        other.3
      )
    }
  }
}

Ordering :: enum {
  Less
  Equal
  Greater
}

Ord :: trait {
  ord :: fn(this Self, other Self) -> Ordering
} for {
  impl[T: int.Int + Eq] _ for T {
    ord :: fn(this T, other T) -> Ordering {
      ret if int.Int.lt(this, other): .Less
      else if int.Int.eq(this, other): .Equal else .Greater
    }
  }
}


max :: fn[T: Ord](a T, b T) -> T: match Ord.ord(a, b) {
    .Less: b,
    .Equal: a,
    .Greater: a,
}

min :: fn[T: Ord](a T, b T) -> T: match Ord.ord(a, b) {
  .Less: a,
  .Equal: a,
  .Greater: b,
}
