
main :: fn {
  println("Hello, World")
  r := 2.5
  area := circle_area(r)
  print("Area of circle with radius 2.5: ~")
  println(area as i32)

  f := .Apple
  if area < 20.0 {
    # The enum can be named explicitly but it doesn't have to be if the type can be inferred
    f = Fruit.Banana
  }
  # This would give an error because 'Pear' is not defined in the enum
  # f = .Pear
  println(fruit_to_string(f))
}

PI : f64 : 3.14

circle_area :: fn(r f64) -> f64: PI * r * r

Fruit :: enum {
  Apple
  Banana
}

fruit_to_string :: fn(f Fruit) -> str: match f {
  .Apple: "Apple",
  .Banana: "Banana",
}

# a trait definition
ToString :: trait {
  to_string :: fn(self Self) -> *i8
}
