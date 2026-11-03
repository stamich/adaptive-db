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

// The advisor reads telemetry snapshots, the catalog's index definitions and table statistics. It
// never touches the engine: its only output is a report of recommendations.
dependencies {
    api(project(":adb-model"))
    api(project(":adb-statistics"))
    api(project(":adb-catalog"))
    api(project(":adb-telemetry"))
}
