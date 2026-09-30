use std.list.List

main :: fn {
  numbers := List.new()
  println(numbers)
  numbers.push(1)
  numbers.push(2)
  numbers.push(3)
  println(numbers)
  numbers.push(4)
  println(numbers)
  numbers.push(5)
  println(numbers)
}
