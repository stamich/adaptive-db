plugins {
    scala
    `java-library`
}

// The benchmark harness of the JVM side (Milestone 2.5.4.1): the Scala counterpart of the Rust
// crate `adb-bench`. It depends on nothing but the Scala library, so any module can time with it.
dependencies {
    implementation("org.scala-lang:scala3-library_3:3.3.8")
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.0")
}

tasks.test {
    useJUnitPlatform()
}
