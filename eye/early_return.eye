
main :: fn {
    std.c.printf("%d, %d, %d, %d\n".ptr as *i8, half(-3), half(0), half(1), half(7))
    print(test(7))
    print(", ")
    println(test(11))
    println(std.int_to_str(while_loop()))
    do_nothing()
}

do_nothing :: fn {
    ret
    println("This should never be printed!")
}

half :: fn(x i32) -> i32 {
    if x < 2: ret 0
    ret x / 2
}

test :: fn(num i32) -> str {
    ret if num < 10 {
        ret "Your number is small"
    } else "Your number is at least 10"
    # some unreachable code
    x := 3
    y := 4
    if x < y { ret "" } else { ret "foo" }
}

while_loop :: fn -> i32 {
    while true {
        ret 0
    }
    ret 1
}
