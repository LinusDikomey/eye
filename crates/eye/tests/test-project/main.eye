
# fn printf(fmt *i8, ...) i32 extern
use std.c.printf

Point :: struct { x i32, y i32 }

use folder.file # modules can be used of course

main :: fn {
    p := Point(x: 4, y: 2)
    printf("Hello World: %d\n".ptr as *i8, other.add(p.x, 2))
    printf("Square Magnitude of Point: %d\n".ptr as *i8, other.squareMag(p))
    file.function()

    # works because function is reexported from mod.eye in folder
    folder.function()

    {
        # local use statements
        use file.function
        function()
    }
}
