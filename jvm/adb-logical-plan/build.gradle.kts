plugins {
    scala
    `java-library`
}

dependencies {
    implementation("org.scala-lang:scala3-library_3:3.3.8")
    testImplementation("org.junit.jupiter:junit-jupiter:6.1.3")
}

tasks.test {
    useJUnitPlatform()
}

dependencies {
    api(project(":adb-model"));
    api(project(":adb-sql"));
    api(project(":adb-catalog"))
}
