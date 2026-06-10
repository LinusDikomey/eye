use std.c.printf
use std.c.exit

main :: fn {
    # exit() can coerce into any type because it never returns
    x: i32 = if 1 > 2: exit(5) else 3
    # x := exit(5)

    println(std.int_to_str(x))
    match_never(3)
}

SomeType :: struct { x i32, y i32 }

never_return :: fn -> SomeType: exit(5)

if_never :: fn -> f32 {

    if 1 > 2 {
        exit(7)
        print("This won't even end up in the IR")
    }

    ret 3.5
}

while_never :: fn -> str {

    while false {
        exit(7)
        ret "This also won't land in the IR"
    }

    ret "Hello from while_never"
}

block_never :: fn -> i16 {
    {
        x := 123
        exit(8)
        ret x
    }
    x := exit(8)
    x = "Also not in the IR"
    5
}

match_never :: fn(x i64) -> str: match x as i8 {
    -128..20: "A",
    21: {
        x := 3
        exit(x)
        print("not in the IR")
    },
    22: "B",
    23..127: "C",
}
