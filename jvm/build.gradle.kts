plugins {
    base
}

allprojects {
    group = "io.adb"
    version = "2.2.3"

    repositories {
        mavenCentral()
    }
}

subprojects {
    plugins.withId("java") {
        extensions.configure<JavaPluginExtension> {
            toolchain {
                languageVersion.set(JavaLanguageVersion.of(22))
            }
        }

        // Gradle 9.x requires the JUnit Platform launcher explicitly on the test runtime path.
        dependencies.add("testRuntimeOnly", "org.junit.platform:junit-platform-launcher:6.1.3")
    }

    // Scala 3.3.8 supports JDK 22 bytecode output, so Scala and Java now share
    // the same target level instead of forcing Scala class files down to Java 21.
    tasks.withType<org.gradle.api.tasks.scala.ScalaCompile>().configureEach {
        targetCompatibility = "22"
    }
}
