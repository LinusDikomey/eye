# eyelang

A basic statically typed systems-level programming language.
The compiler is written in rust using LLVM as its backend.
A simple, fast x86_64 backend is also on it's way.

## Ideas
- Runtime Speed: The language should make it possible to write code about as fast as C.
- Short syntax: Shorter code is more fun to write and easier to read.
- Type inference: Having to write less types makes the code shorter and refactoring easier.
- Compilation speed: Compilation speed over a few seconds makes debugging painfully slow.
- Simplicity: Syntax features are nice but too many of them make the language too complicated.

## Examples

### Basic
```rust
main :: fn {
    println("Hello World")
    x := 3
    y := 2 * x + 3
    y += 1
    print("X is ")
    print(x)
    print(" and y is ")
    println(y)
}
```

### Functions and pointers
```rust
use std.c.printf

add_pointer_values :: fn(x *i32, y *i32) -> i32: x^ + y^

main :: fn {
    x := 5
    y := 7
    print("Result: ")
    println(add_pointer_values(&x, &y))
}
```

### Expressions, type inference
```rust
add :: fn(x i64, y i64) -> i64: x + y

main :: fn {
    x := 3 # x is inferred to have type i64
    pointer := &x
    print("Result: ")
    println(if 1 < 2: add(pointer^, 4) else -1)
}
```

### Heap allocation
```rust
use std.c

Vec3 :: struct { x f64, y f64, z f64 }

print_vec3 :: fn(v *Vec3) {
    c.printf("Vec3: [%.1f, %.1f, %.1f]\n".ptr as *i8, v^.x, v^.y, v^.z)
}

main :: fn {
    v := c.malloc(Vec3.size) as *Vec3
    v^ = Vec3(x: 1.0, y: 2.0, z: 3.0)
    print_vec3(v)
}
```

### Enums
Enums can be used locally without a type definition.
```rust
main :: fn {
    color := .NoColor
    inp := std.input("Which color do you want? ")
    if inp == "red": color = .Red
        else if inp == "green": color = .Green
        else if inp == "blue": color = .Blue

    if .NoColor := color:
        print("You didn't select a valid color")
    else {
        print("Your color is ")
        # match exhaustively checks patterns on a value. Removing any of the
        # arms would create an error (even though the enum is completely inferred!)
        println(match color {
            .Red: "Red",
            .Green: "Green",
            .Blue: "Blue",
            .NoColor: std.panic("unreachable"),
        })
    }
}
```
Enum variants can also have arguments. This is known as sum types/algebraic data types in many other languages.
Note that implicit enums can infer to explicitly typed enums without having to name the type.
```rust
Fruit :: enum { Apple(i32), Banana, Citrus }

main :: fn {
    # f is of type Fruit here because it is passed to print_fruit
    f := .Banana
    f = .Apple(5)

    # This would create a compilation error because Orange is not a valid variant of Fruit
    # f = .Orange

    print_fruit(f)

    print_fruit(.Banana)
}

print_fruit :: fn(f Fruit): match f {
    .Apple(radius) {
        print("Apple with radius ")
        println(std.int_to_str(radius))
    }
    .Banana: println("A Banana"),
    .Citrus: println("A Lemon?")
}
```

## Build Instructions

You can use the nix flake for building and developing. Use `nix build` or `nix develop`.

Otherwise, you will need to install LLVM to build this project.
Look at the [llvm-sys](https://crates.io/crates/llvm-sys) crate for detailed instructions on how
to build or install llvm.

To run a program, use `cargo run -- run example.eye`
or install the compiler with `cargo install` and use
`eye run example.eye` allthough this will require the std library to be present in the same
directory as the binary or the working directory. Installing via the nix flake will handle that
part for you.
