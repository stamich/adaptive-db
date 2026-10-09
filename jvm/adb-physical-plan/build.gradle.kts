plugins {
    scala
    `java-library`
}

dependencies {
    implementation("org.scala-lang:scala3-library_3:3.9.0")
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.0")
}

tasks.test {
    useJUnitPlatform()
}

dependencies {
    api(project(":adb-logical-plan"));
    api(project(":adb-model"))
    // Cost-based strategies and plan annotations use the estimator and the cost model.
    api(project(":adb-optimizer"))
}
