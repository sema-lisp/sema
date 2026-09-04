# Browser examples

Each `.sema` file here is a complete sema-web app. From this directory, run one
with the dev server. It serves the runtime, opens a browser, and reloads on
save:

```bash
sema web counter.sema            # imperative dom/* API + localStorage
sema web counter-reactive.sema   # state / update! / hiccup / mount!
sema web wordle.sema             # responsive Lisp-themed word game
sema web llm-chat.sema           # streaming chat through the built-in LLM proxy
sema web hello.sema
```

`llm-chat.sema` needs a provider key in the environment (`ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, ...); the dev server forwards requests with it.

The `.html` pages are the same apps as they would be embedded in a site built
with a bundler. They import `@sema-lang/sema-web` by its package name, so open
them through Vite (or add an import map); they do not work from `file://`.
