type LanguageRegistration = {
  libraryPath: string
  extensions: string[]
  languageSymbol?: string
  metaVarChar?: string
  expandoChar?: string
}

declare const registration: LanguageRegistration
export = registration
