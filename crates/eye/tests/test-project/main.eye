Point :: struct {
  x i32
  y i32
}

use folder.file

main :: fn {
  p := Point(x: 4, y: 2)
  value := other.add(p.x, 2)
  print("Hello World: ")
  println(value)
  print("Square Magnitude of Point: ")
  println(other.squareMag(p))
  file.function()

  # works because function is reexported from mod.eye in folder
  folder.function()

  {
    # local use statements
    use file.function

    function()
  }
}
