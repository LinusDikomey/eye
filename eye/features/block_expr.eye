
main :: fn {
  x := if 1 < 2 {
    println("Yep")
    42
  } else {
    println("Oh oh")
    2
  }
  println(x)

  # different return types from branches but it's allowed since match is in statement position
  match 5 {
    2: println("Hello"),
    3: {
      12
    },
    5: f(),
    4: "hi",
  }
}

f :: fn -> i32 {
  println("Hi")
  5
}
