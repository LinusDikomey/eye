use root.list.List
use root.ptr_add
use root.c.malloc
use root.c.memcpy
use root.c.printf
use root.c.exit
use root.panic
use root.int.Int

# represents a string slice
str :: struct {
    ptr *u8
    len usize

    from_cstr :: fn(ptr *i8) -> str: str(ptr: ptr as *u8, len: len(ptr))

    is_empty :: fn(this str) -> bool: this.len == 0

    from_utf8 :: fn(bytes List[u8]) -> str {
        # TODO: utf-8 validation, allocation resizing
        ret str(ptr: bytes.buf, len: bytes.len)
    }

    impl Eq {
        eq :: fn(this str, other str) -> bool {
            if this.len != other.len: ret false
            i := 0
            while i < this.len {
                if this.byte(i) != other.byte(i): ret false
                i += 1
            }
            ret true
        }
    }

    clone :: fn(this str) -> str {
        new_ptr := malloc(this.len)
        memcpy(new_ptr, this.ptr, this.len)
        ret str(ptr: new_ptr, len: this.len)
    }

    slice :: fn(this str, start u64, end u64) -> str {
        if end > this.len {
            print("[PANIC]: string slice out of range: ")
            print(start)
            print("..")
            print(end)
            print(" > ")
            println(this.len)
            exit(1)
        }
        if start > end: start = end
        ret str(ptr: ptr_add(this.ptr, start), len: end - start)
    }

    byte :: fn(this str, index u64) -> u8 {
        if index >= this.len {
            printf("[PANIC]: str byte index out of range: %d >= %d".ptr as *i8, index, this.len)
            exit(1)
        }
        ret ptr_add(this.ptr, index)^
    }

    split :: fn(this str, pat str) -> Split {
        if pat.len == 0: panic("empty pattern supplied to split")
        ret Split(string: this, pat: pat)
    }

    lines :: fn(this str) -> List[str] {
        list := List.new()

        start := 0
        cur := 0

        while (true) {
            if cur == this.len {
                list.push(this.slice(start, cur))
                ret list
            }
            match this.byte(cur) {
                10 {
                    list.push(this.slice(start, cur))
                    cur += 1
                    if cur < this.len: if this.byte(cur) == 13: cur += 1
                    start = cur
                }
                13 {
                    # handle \r\n
                    if cur + 1 < this.len and this.byte(cur + 1) == 10 {
                        list.push(this.slice(start, cur))
                        cur += 2
                    } else {
                        cur += 1
                    }
                }
                _: cur += 1
            }
        }
        ret list
    }

    parse :: fn[I: Int](this str) -> I {
        negate := false
        x := Int.zero()
        i := 0
        while i < this.len {
            b := this.byte(i)
            if b == ASCII_MINUS and Int.eq(x, Int.zero()): negate = !negate
            else if b >= ASCII_ZERO and b <= ASCII_NINE {
                x = Int.mul(x, Int.ten())
                x = Int.add(x, Int.from_u8((b - ASCII_ZERO)))
            }
            i += 1
        }
        ret if negate: Int.sub(Int.zero(), x) else x
    }

    trim :: fn(this str) -> str {
        start := 0
        b := true
        while start < this.len and b {
            if is_whitespace(this.byte(start)):
                start += 1
            else b = false
        }

        end := this.len
        b = true
        while end > 0 and b {
            if is_whitespace(this.byte(end - 1)):
                end -= 1
            else b = false
        }
        ret this.slice(start, end)
    }

    make_ascii_lowercase :: fn(this *str) {
        i := 0
        while i < this.len {
            char_ptr := ptr_add(this.ptr, i)
            if char_ptr^ >= "A".ptr^ and char_ptr^ <= "Z".ptr^ {
                char_ptr^ += 32
            }
            i += 1
        }
    }

    make_ascii_uppercase :: fn(this str) {
        i := 0
        while i < this.len {
            char_ptr := ptr_add(this.ptr, i)
            if char_ptr^ >= "a".ptr^ and char_ptr^ <= "z".ptr^ {
                char_ptr^ -= 32
            }
            i += 1
        }
    }

    get :: fn(this str, index u64) -> Option[str] {
        if index >= this.len: ret .None
        ret .Some(str(ptr: ptr_add(this.ptr, index), len: 1))
    }
}

NEWLINE: i8 : 10
CARRIAGE_RETURN: i8 : 13
ASCII_ZERO: u8 : 48
ASCII_NINE: u8 : 48 + 9
ASCII_MINUS: u8 : 45

len :: fn(s *i8) -> u64: root.c.strlen(s) as _

use root.parse_int

Split :: struct {
    string str
    pat str


    impl Iterator[str] {
        next :: fn(self *Split) -> Option[str] {
            if self.string.len == 0: ret .None
            i := 0
            matched := 0
            while i < self.string.len {
                if self.string.byte(i) == self.pat.byte(matched) {
                    matched += 1
                    if matched == self.pat.len {
                        s := self.string.slice(0, i+1-matched)
                        self.string = self.string.slice(i+1, self.string.len)
                        ret .Some(s)
                    }
                } else matched = 0
                i += 1
            }

            s := self.string
            self.string = self.string.slice(self.string.len, self.string.len)
            ret .Some(s)
        }
    }
}

# TODO: unicode whitespace definition (when chars are handled properly)
is_whitespace :: fn(c u8) -> bool: match c {
    9..13: true, # \t, \n, \v, \f, \r
    32: true, # whitespace
    _: false,
}

# append a string to a list of bytes
push_str :: fn(l *List[u8], s str) {
    p := l.reserve(s.len)
    memcpy(p, s.ptr, s.len)
    l.len += s.len
}

ToString :: trait {
    to_string :: fn(this *Self) -> str
} for {
    impl _ for str {
        to_string :: fn(this *str) -> str: this.clone()
    }

    impl _ for u8 {
        to_string :: fn(this *u8) -> str: int_to_string(this^)
    }
    impl _ for u16 {
        to_string :: fn(this *u16) -> str: int_to_string(this^)
    }
    impl _ for u32 {
        to_string :: fn(this *u32) -> str: int_to_string(this^)
    }
    impl _ for u64 {
        to_string :: fn(this *u64) -> str: int_to_string(this^)
    }
    impl _ for i8 {
        to_string :: fn(this *i8) -> str: int_to_string(this^)
    }
    impl _ for i16 {
        to_string :: fn(this *i16) -> str: int_to_string(this^)
    }
    impl _ for i32 {
        to_string :: fn(this *i32) -> str: int_to_string(this^)
    }
    impl _ for i64 {
        to_string :: fn(this *i64) -> str: int_to_string(this^)
    }
    impl _ for bool {
        to_string :: fn(this *bool) -> str: match this^ {
            .false: "false".clone(),
            .true: "true".clone(),
        }
    }

    impl[T: ToString] _ for List[T] {
        to_string :: fn(this *List[T]) -> str {
            s := List.new()
            push_str(&s, "[")
            i := 0
            while i < this.len {
                if i != 0: push_str(&s, ", ")
                item_str := ToString.to_string(&this.get(i))
                push_str(&s, item_str)
                root.c.free(item_str.ptr)
                i += 1
            }
            push_str(&s, "]")
            ret str(ptr: s.buf, len: s.len)
        }
    }
}

int_to_string :: fn[T: Int](x T) -> str {
    # 20 digits are needed at most for a u64/i64
    MAX_LEN: u64 : 20
    buf: [u8; 20] = [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,ASCII_ZERO]
    i := MAX_LEN - 1
    negative := false
    min_signed_int := false
    if x < Int.zero() {
        negative = true
        x = -x
        # handle min signed int
        if x < Int.zero() {
            # this is very hacky but we subtract one to get the max signed int and fix the last
            # digit later since it is never 9
            min_signed_int = true
            x = Int.sub(x, Int.one())
        }
    }
    while x >= Int.ten() {
        last_digit := Int.mod(x, Int.ten())
        buf[i] = ASCII_ZERO + Int.as_u8(last_digit)
        i -= 1
        x = Int.div(x, Int.ten())
    }
    if !Int.eq(x, Int.zero()) {
        buf[i] = ASCII_ZERO + Int.as_u8(x)
    } else if i != MAX_LEN - 1 {
        i += 1
    }
    if negative {
        i -= 1
        buf[i] = ASCII_MINUS
        if min_signed_int: buf[MAX_LEN-1] += 1
    }
    len := MAX_LEN - i
    ptr := malloc(len)
    memcpy(ptr, &buf[i], len)
    ret str(ptr: ptr, len: len)
}
