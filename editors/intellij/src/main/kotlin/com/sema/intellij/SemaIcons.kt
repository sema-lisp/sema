package com.sema.intellij

import com.intellij.openapi.util.IconLoader
import javax.swing.Icon

object SemaIcons {
    @JvmField
    val FILE: Icon = IconLoader.getIcon("/icons/sema.svg", SemaIcons::class.java)
    @JvmField
    val COMPILED_FILE: Icon = IconLoader.getIcon("/icons/semac.svg", SemaIcons::class.java)
}
