plugins {
    scala
    application
}

dependencies {
    implementation("org.scala-lang:scala3-library_3:3.3.8")
    implementation(project(":adb-model"))
    implementation(project(":adb-catalog"))
    implementation(project(":adb-sql"))
    implementation(project(":adb-logical-plan"))
    implementation(project(":adb-optimizer"))
    implementation(project(":adb-physical-plan"))
    implementation(project(":adb-native"))
    implementation(project(":adb-gateway"))
}

application {
    mainClass.set("io.adb.benchmark.BenchmarkMain")
}
