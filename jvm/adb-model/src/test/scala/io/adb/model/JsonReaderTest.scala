package io.adb.model

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

/** Tests of the shared JSON reader. */
final class JsonReaderTest:
  /** The reader handles every JSON construct the engine emits. */
  @Test def readsJson(): Unit =
    val value = JsonReader.parse("""{"a":[1,-2.5,true,false,null],"b":{"c":"x\"y\u00f3"},"d":[],"e":18446744073709551615}""")
    assertEquals(
      Map(
        "a" -> Vector(1L, -2.5, true, false, null),
        "b" -> Map("c" -> "x\"yó"),
        "d" -> Vector.empty,
        "e" -> BigInt("18446744073709551615")
      ),
      value
    )

  /** Malformed documents fail with `IllegalArgumentException`, never another exception. */
  @Test def rejectsMalformedJson(): Unit =
    for bad <- Vector("""{"a":1""", "[1] x", "\"abc", "\"\\u12\"", "\"\\q\"", "-", "[" * 100 + "]" * 100, "") do
      assertThrows(classOf[IllegalArgumentException], () => { JsonReader.parse(bad); () }, bad)
