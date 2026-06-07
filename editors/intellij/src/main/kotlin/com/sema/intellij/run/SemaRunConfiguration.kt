package com.sema.intellij.run

import com.intellij.execution.Executor
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.LocatableConfigurationBase
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.execution.configurations.RunProfileState
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.project.Project
import org.jdom.Element

class SemaRunConfiguration(
    project: Project,
    factory: ConfigurationFactory,
    name: String,
) : LocatableConfigurationBase<RunProfileState>(project, factory, name) {

    var scriptPath: String = ""
    var arguments: String = ""
    var workingDirectory: String = project.basePath ?: ""

    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState {
        return SemaRunState(environment, this)
    }

    override fun getConfigurationEditor(): SettingsEditor<out RunConfiguration> {
        return SemaRunConfigurationEditor(project)
    }

    override fun readExternal(element: Element) {
        super.readExternal(element)
        scriptPath = element.getAttributeValue("scriptPath") ?: ""
        arguments = element.getAttributeValue("arguments") ?: ""
        workingDirectory = element.getAttributeValue("workingDirectory") ?: project.basePath ?: ""
    }

    override fun writeExternal(element: Element) {
        super.writeExternal(element)
        element.setAttribute("scriptPath", scriptPath)
        element.setAttribute("arguments", arguments)
        element.setAttribute("workingDirectory", workingDirectory)
    }
}
