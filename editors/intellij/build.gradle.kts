import org.jetbrains.changelog.Changelog
import org.jetbrains.intellij.platform.gradle.TestFrameworkType

plugins {
    id("org.jetbrains.kotlin.jvm") version "2.4.0"
    id("org.jetbrains.intellij.platform") version "2.16.0"
    id("org.jetbrains.changelog") version "2.5.0"
}

group = providers.gradleProperty("pluginGroup").get()
version = providers.gradleProperty("pluginVersion").get()

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        val platformLocalPath = providers.gradleProperty("platformLocalPath")
        if (platformLocalPath.isPresent) {
            local(platformLocalPath)
        } else {
            create(providers.gradleProperty("platformType").get(), providers.gradleProperty("platformVersion").get())
        }

        val lsp4ijLocalPath = providers.gradleProperty("lsp4ijLocalPath")
        if (lsp4ijLocalPath.isPresent) {
            localPlugin(lsp4ijLocalPath)
        } else {
            plugin("com.redhat.devtools.lsp4ij:0.20.1")
        }

        testFramework(TestFrameworkType.Platform)
        pluginVerifier()
        zipSigner()
    }

    testImplementation("junit:junit:4.13.2")
}

sourceSets {
    create("integrationTest") {
        compileClasspath += sourceSets.main.get().output
        runtimeClasspath += sourceSets.main.get().output
    }
}

val integrationTestImplementation by configurations.getting {
    extendsFrom(configurations.testImplementation.get())
}

dependencies {
    intellijPlatform {
        testFramework(TestFrameworkType.Starter, configurationName = "integrationTestImplementation")
    }
    integrationTestImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
    integrationTestImplementation("org.kodein.di:kodein-di-jvm:7.20.2")
    integrationTestImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-core-jvm:1.10.1")
}

val integrationTest by intellijPlatformTesting.testIdeUi.registering {
    task {
        val integrationTestSourceSet = sourceSets.getByName("integrationTest")
        testClassesDirs = integrationTestSourceSet.output.classesDirs
        classpath = integrationTestSourceSet.runtimeClasspath
        useJUnitPlatform()
        systemProperty("path.to.build.plugin", layout.buildDirectory.dir("distributions").get().asFile.resolve("${rootProject.name}-${version}.zip").absolutePath)
    }
}

kotlin {
    jvmToolchain(21)
}

intellijPlatform {
    instrumentCode = false

    pluginConfiguration {
        name = providers.gradleProperty("pluginName")
        version = providers.gradleProperty("pluginVersion")
        ideaVersion {
            sinceBuild = providers.gradleProperty("pluginSinceBuild")
            // until-build is intentionally omitted for broad forward compatibility.
            // (Decision: open-ended range. An unset until-build defaults to a bounded
            // "<branch>.*" in IntelliJ Platform Gradle Plugin 2.x, so we null it explicitly.)
            untilBuild = provider { null }
        }

        // Pull the change notes for the version being built from CHANGELOG.md.
        val changelog = project.changelog
        changeNotes = project.provider {
            with(changelog) {
                renderItem(
                    (getOrNull(project.version.toString()) ?: getUnreleased())
                        .withHeader(false)
                        .withEmptySections(false),
                    Changelog.OutputType.HTML,
                )
            }
        }
    }

    signing {
        certificateChain = providers.environmentVariable("CERTIFICATE_CHAIN")
        privateKey = providers.environmentVariable("PRIVATE_KEY")
        password = providers.environmentVariable("PRIVATE_KEY_PASSWORD")
    }

    publishing {
        token = providers.environmentVariable("PUBLISH_TOKEN")
        // Plain version (e.g. 1.0.0) -> "default" (stable); a pre-release suffix
        // (e.g. 1.1.0-beta.1) -> "beta".
        channels = providers.gradleProperty("pluginVersion")
            .map { listOf(it.substringAfter('-', "").substringBefore('.').ifEmpty { "default" }) }
    }

    pluginVerification {
        // Experimental-API usages (LSP4IJ DAP descriptors) are accepted: they are
        // reported as warnings, not failures, and are deliberately left out of the
        // failureLevel set. See SemaDebugAdapterDescriptorFactory for rationale.
        ides {
            current()
            recommended()
        }
    }
}

changelog {
    groups.empty()
    repositoryUrl = providers.gradleProperty("pluginRepositoryUrl")
}
