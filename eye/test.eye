sayHello :: fn(newline bool) {
  print("Hello")
  if newline: println("")
}

bye :: fn {
  println("Bye")
}

add :: fn(x i32, y i32) -> i32: x + y

abs :: fn(x i32) -> i32: if x < 0: -x else x
bla :: fn(x i32, y i32) {
  z := 3
  w := 5
}

main :: fn {
  incAndPrint :: fn(i i32) -> i32 {
    print("I is ")
    println(i)
    ret i + 1
  }

  # print(string(abs(-4)), "\n")
  v := Vec3(x: 1.0, y: 2.5, z: 3.0)

  # print("Vec before assignment: ", string(v.x), " ", string(v.y), " ", string(v.z), "\n")
  v.y = 3.1
  # print("Vec after assignment: ", string(v.x), " ", string(v.y), " ", string(v.z), "\n")
  # print("x: ", string(v.x), "\n")
  name := "John Doe"
  sayHello(false)
  print(", ")
  println(name)
  inp := "123456789".parse()
  # print("You entered: ", string(inp), "\n")
  x := 4

  if inp < 5: print("Your number is smaller than 5\n")
  else print("Your number is 5 or larger\n")

  y: i32 = inp / 2
  # print("Half your number is: ", string(y), "\n")
  print("Some calculations:\n")
  # test()("Calling return value from test()\n")
  printVec3(addVec3(Vec3(x: 1., y: 2., z: 3.), Vec3(x: 4., y: 7., z: 9.)))

  i := 5
  while i < 10: i = incAndPrint(i)

  {
    PhysicsObject :: struct {
      pos Vec3
      rot Vec3
    }
    Vec3 :: struct {
      x i32
      y i32
      z i32
    }

    obj := PhysicsObject(pos: Vec3(x: 1, y: 2, z: 3), rot: Vec3(x: 4, y: 5, z: 6))
    print("Physics Object: ")
    println(obj.pos.y + obj.rot.z)
  }
  # Here Vec3 means Vec3 from the global scope again
  v := Vec3(x: 1., y: 2.5, z: 3.141)
  printVec3(v)
  commentTest()
  bye()
}

Vec3 :: struct {
  x f32
  y f32
  z f32
}

addVec3 :: fn(a Vec3, b Vec3) -> Vec3: Vec3(x: a.x + b.x, y: a.y + b.y, z: a.z + b.z)
printVec3 :: fn(v Vec3) {
  print("Vec3[")
  x := v.x as i32
  print(x)
  print(", ")
  print((v.y as i32))
  print(", ")
  print((v.z as i32))
  print("]\n")
}

Transform :: struct {
  pos Vec3
  rot Vec3
  scale Vec3
}

commentTest :: fn {
  # this is a comment
  #-
    This is a
    multiline comment
    -#
  #- nested #- multiline -# comment -#

  # Should not start a Multiline comment in a comment #-
  println("This is not commented out")
  # #--# Still a comment after multiline
}
