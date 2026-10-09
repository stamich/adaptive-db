plugins {
    scala
    application
}

dependencies {
    implementation("org.scala-lang:scala3-library_3:3.9.0")
    implementation(project(":adb-gateway"))
    implementation(project(":adb-catalog"))
    implementation(project(":adb-native"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.0")
}

application {
    mainClass.set("io.adb.cli.Main")
}

tasks.test {
    useJUnitPlatform()
}
