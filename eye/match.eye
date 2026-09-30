use std.c.printf

Fruit :: enum {
  Apple
  Banana
  Pineapple
}

main :: fn {
  # when no arm is matched, undefined values will be returned from the match expression right now
  f := Fruit.Banana
  text := match f {
    .Apple: "an apple",
    .Banana: "a banana",
    .Pineapple: "a pineapple",
    # comma is optional for the last match arm
  }
  print("I'm eating ")
  println(text)

  num := 3
  s := match num {
    1: "bye",
    2: "goodbye",
    3: "see you later alligator",
    _other: "unknown",
  }
  println(s)

  # block arms
  # returning values from block arms will become possible with the 'yield' keyword
  match num {
    1: {
      println("hello")
    },
    # no comma needed after block
    2: {
      println("bye")
    },
    # comma can still be used
    3: {
      x := 1 + 1 + num
      print("result of calculation: ")
      println(x)
    },
    _other: {},
  }
}
