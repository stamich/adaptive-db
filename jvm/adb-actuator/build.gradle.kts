plugins {
    scala
    `java-library`
}

dependencies {
    implementation("org.scala-lang:scala3-library_3:3.3.8")
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.0")
}

tasks.test {
    useJUnitPlatform()
}

// The actuator carries out what the advisor recommends, inside a policy and a budget, and undoes
// what does not pay. It reaches the engine only through the `Actuator` interface, which the
// gateway implements, so this module has no engine handle of its own.
dependencies {
    api(project(":adb-advisor"))
}
