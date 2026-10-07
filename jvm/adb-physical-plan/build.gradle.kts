plugins {
    scala
    `java-library`
}

dependencies {
    implementation("org.scala-lang:scala3-library_3:3.3.8")
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.0")
    // Planner tests bind and optimize real SQL before physical planning.
    testImplementation(project(":adb-optimizer"))
}

tasks.test {
    useJUnitPlatform()
}

dependencies {
    api(project(":adb-logical-plan"));
    api(project(":adb-model"))
}
