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
