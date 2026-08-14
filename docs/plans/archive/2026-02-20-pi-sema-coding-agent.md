# Pi-Sema: A Comprehensive Coding Agent in Sema

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a near-feature-complete terminal coding agent in Sema Lisp, inspired by [pi.dev](https://pi.dev), that demonstrates Sema's LLM primitives as a real-world application.

**Architecture:** Multi-file modular design using Sema's `(load ...)` module system. The agent uses `deftool`/`defagent`/`agent/run` for the core LLM loop with tool execution, adds session persistence via NDJSON files, slash commands for interactive control, AGENTS.md context loading, conversation compaction, and a rich terminal UX with color-coded tool call output.

**Tech Stack:** Sema Lisp (deftool, defagent, agent/run, llm/stream, llm/session-usage), file I/O, shell execution, JSON encoding/decoding, terminal styling (term/style, term/spinner-*).

---

## File Structure

```
examples/pi-sema/
├── main.sema              # Entry point — CLI arg parsing, REPL loop, slash command dispatch
├── agent.sema             # Agent definition, system prompt builder, context file loading
├── tools.sema             # All 7 tool definitions (read, write, edit, bash, grep, find, ls)
├── commands.sema          # Slash command implementations (/help, /clear, /model, /compact, /session, /usage, /status)
├── session.sema           # Session persistence — save/load/list/fork NDJSON conversation files
├── compact.sema           # Conversation compaction — summarize old context to stay within limits
├── context.sema           # AGENTS.md discovery + loading, skill file discovery
├── display.sema           # Terminal UX — tool call rendering, welcome banner, formatting helpers
├── util.sema              # Path resolution, string helpers, safety checks
└── README.md              # Usage docs, architecture overview
```

---

## Available Sema Primitives (reference)

**LLM:** `llm/auto-configure`, `deftool`, `defagent`, `agent/run` (with `:on-tool-call` and `:messages`), `llm/chat`, `llm/stream`, `llm/session-usage`
**I/O:** `read-line`, `println`, `println-error`, `print`, `format`, `str`
**Files:** `file/read`, `file/write`, `file/exists?`, `file/list`, `file/mkdir`, `file/delete`, `file/rename`
**Shell:** `(shell "cmd")` → `{:stdout :stderr :exit-code}`, `(shell "cmd" "arg1" "arg2")`
**JSON:** `json/encode`, `json/encode-pretty`, `json/decode`
**Terminal:** `term/style`, `term/dim`, `term/bold`, `term/green`, `term/red`, `term/cyan`, `term/magenta`, `term/spinner-start`, `term/spinner-stop`
**System:** `sys/cwd`, `sys/platform`, `sys/env`, `sys/args`, `sys/arch`, `sys/os`, `sys/which`
**Strings:** `string/split`, `string/join`, `string/trim`, `string/contains?`, `string/replace`, `string/starts-with?`, `string/ends-with?`, `string-length`, `substring`, `string-append`, `regex/match`, `regex/split`
**Data:** `assoc`, `get`, `keys`, `vals`, `map/entries`, `map/merge`, `map?`, `list?`, `nil?`, `length`, `map`, `filter`, `foldl`, `for-each`, `append`, `sort`, `take`, `drop`, `zip`, `range`, `any`, `every`, `first`, `nth`, `car`, `cdr`, `cons`
**Control:** `define`, `set!`, `let`, `let*`, `if`, `cond`, `when`, `unless`, `begin`, `lambda`/`fn`, `and`, `or`, `not`, `try`/`catch`, `error`
**Misc:** `gensym`, `exit`, `time-ms`

---

## Task 1: Utility Module (`util.sema`)

**Files:** Create `examples/pi-sema/util.sema`

Path resolution, safety, and string helpers used across all modules.

**Functions to implement:**
- `(resolve-path root path)` — join root + path, reject `..` traversal and absolute paths
- `(safe-path? root path)` — return #t if path stays within root
- `(truncate-str s max-len)` — truncate with `...` suffix
- `(timestamp)` — ISO-ish timestamp string from `time-ms`
- `(uuid)` — simple random ID using `gensym` or random
- `(file-extension path)` — extract extension
- `(basename path)` — extract filename from path
- `(dirname path)` — extract directory from path
- `(ensure-dir path)` — create directory if it doesn't exist
- `(banned-command? cmd)` — check against a denylist of dangerous shell patterns

---

## Task 2: Display Module (`display.sema`)

**Files:** Create `examples/pi-sema/display.sema`

Terminal UX: welcome banner, tool call rendering callback, formatting helpers.

**Functions to implement:**
- `(show-welcome cwd model)` — print styled welcome banner with version/model/cwd info
- `(show-prompt)` — print the input prompt (styled `pi-sema ❯ `)
- `(on-tool-call event)` — callback for `agent/run`'s `:on-tool-call`. On "start": print tool name + key args with spinner. On "end": stop spinner with ✔ and duration.
- `(show-response text)` — print assistant response with styling
- `(show-error text)` — print red error message
- `(show-info text)` — print dim info message
- `(show-divider)` — print a subtle divider line
- `(show-usage usage)` — pretty-print token usage stats

---

## Task 3: Tools Module (`tools.sema`)

**Files:** Create `examples/pi-sema/tools.sema`

All 7 LLM-callable tools, matching pi's tool set.

### Tool 1: `read-file`
- Schema: `{:path :type :string, :offset :type :integer (optional), :limit :type :integer (optional)}`
- Read file contents with line numbers. Support offset/limit pagination for large files.
- Truncate at 2000 lines / show byte count. Return line-numbered content.
- Handle binary detection (if non-UTF8, return "Binary file" message).
- Handle image files (return "Image file: WxH" placeholder).

### Tool 2: `write-file`
- Schema: `{:path :type :string, :content :type :string}`
- Auto-create parent dirs. Write content. Return confirmation + byte count.

### Tool 3: `edit-file`
- Schema: `{:path :type :string, :old_string :type :string, :new_string :type :string}`
- Find-and-replace. Must read file first. Fail if old_string not found or matches multiple times.
- Return a unified diff preview of the change.

### Tool 4: `bash`
- Schema: `{:command :type :string}`
- Run via `(shell "sh" "-c" command)`. Check `banned-command?` first.
- Truncate output at 500 lines. Return stdout + stderr + exit code.
- Set CWD context in description.

### Tool 5: `grep`
- Schema: `{:pattern :type :string, :path :type :string (optional), :include :type :string (optional)}`
- Shell out to `rg` (ripgrep) or fall back to `grep -rn`.
- Limit to 100 matches. Return file:line:content format.

### Tool 6: `find-files`
- Schema: `{:pattern :type :string, :path :type :string (optional)}`
- Shell out to `find` with name glob. Respect common ignores (.git, node_modules).
- Limit to 200 results.

### Tool 7: `list-dir`
- Schema: `{:path :type :string (optional)}`
- List directory contents with file/dir indicators and sizes.

All tools receive the workspace root via closure and resolve paths relative to it.

---

## Task 4: Context Module (`context.sema`)

**Files:** Create `examples/pi-sema/context.sema`

AGENTS.md discovery and loading, skill-like context injection.

**Functions to implement:**
- `(discover-context-files cwd)` — scan for AGENTS.md / .pi-sema/agents.md in cwd and parent dirs (up to home or root). Return list of `{:path :content}`.
- `(load-context-files cwd)` — read all discovered files, return combined content string.
- `(discover-skills cwd)` — scan `.pi-sema/skills/` for `.md` files with name/description. Return list of `{:name :description :path}`.
- `(format-skills-xml skills)` — render `<available_skills>` XML block for system prompt.

---

## Task 5: Session Module (`session.sema`)

**Files:** Create `examples/pi-sema/session.sema`

Append-only NDJSON session persistence.

**Session file format:** `~/.pi-sema/sessions/<id>.ndjson`
Each line is a JSON object with `{:type :id :timestamp ...}`:
- `"session"` — header entry: `{:type "session" :id "..." :cwd "..." :model "..." :timestamp ...}`
- `"message"` — chat message: `{:type "message" :role "user"|"assistant" :content "..." :timestamp ...}`
- `"compaction"` — compaction summary: `{:type "compaction" :summary "..." :kept-from :id :timestamp ...}`
- `"model-change"` — `{:type "model-change" :model "..." :timestamp ...}`

**Functions to implement:**
- `(session/new cwd model)` — create new session, return session state `{:id :path :messages}`
- `(session/append-entry session entry)` — append one NDJSON line to session file
- `(session/save-message session role content)` — convenience for appending a message entry
- `(session/load path)` — read NDJSON file, reconstruct messages list
- `(session/list)` — list all session files with timestamps and first user message preview
- `(session/resume path)` — load and return session state
- `(session/messages session)` — return the messages list for agent/run

---

## Task 6: Compaction Module (`compact.sema`)

**Files:** Create `examples/pi-sema/compact.sema`

Summarize old conversation context when it gets too large.

**Functions to implement:**
- `(estimate-tokens messages)` — rough token count (chars / 4)
- `(needs-compaction? messages max-tokens)` — return #t if estimated tokens exceed threshold
- `(compact-messages messages model)` — use `llm/chat` to summarize older messages into a single summary message. Keep recent N messages intact. Return new messages list with summary prepended.
- `(auto-compact session messages model max-tokens)` — check + compact if needed, persist compaction entry to session

---

## Task 7: Slash Commands (`commands.sema`)

**Files:** Create `examples/pi-sema/commands.sema`

Interactive commands dispatched from the REPL.

**Commands to implement:**
- `/help` — show all available commands with descriptions
- `/clear` — clear conversation history (start fresh, same session)
- `/model [name]` — show current model or switch to a new one
- `/compact` — force compaction of current conversation
- `/session list` — list saved sessions
- `/session new` — start a new session
- `/session resume <id>` — resume a previous session
- `/usage` — show token usage stats (via `llm/session-usage`)
- `/status` — show current state (model, session ID, message count, cwd)
- `/quit` or `/exit` — exit the agent

**Functions:**
- `(slash-command? input)` — return #t if input starts with `/`
- `(parse-command input)` — return `{:name :args}` 
- `(dispatch-command cmd state)` — execute command, return updated state (or #f for unknown)

---

## Task 8: Agent Module (`agent.sema`)

**Files:** Create `examples/pi-sema/agent.sema`

System prompt construction and agent definition.

**Functions to implement:**
- `(build-system-prompt cwd platform context-content skills)` — construct the full system prompt dynamically:
  - Agent persona ("You are pi-sema, a coding agent...")
  - Tool usage rules (read before edit, verify changes, minimal edits)
  - Working directory and platform info
  - Context from AGENTS.md files
  - Available skills XML block
  - Current date/time
- `(create-agent cwd model)` — call `build-system-prompt`, define tools, return `defagent` result
- `(run-turn agent input state)` — call `agent/run` with `:messages` from state and `:on-tool-call` callback, return updated state with new messages

---

## Task 9: Main Entry Point (`main.sema`)

**Files:** Create `examples/pi-sema/main.sema`

CLI parsing, initialization, and the main REPL loop.

**Flow:**
1. Parse CLI args: `--model`, `--print` (non-interactive), `-p "prompt"` (one-shot)
2. `llm/auto-configure` — fail early if no API key
3. Determine workspace root (`sys/cwd`)
4. Load context files (AGENTS.md)
5. Create or resume session
6. Create agent with system prompt
7. Show welcome banner
8. Enter REPL loop:
   - `show-prompt` → `read-line`
   - If slash command → `dispatch-command`
   - If EOF → exit
   - Otherwise → `run-turn` → `show-response`
   - Auto-compact if needed
   - Save messages to session
   - Loop

**One-shot mode:** If `-p "prompt"` is given, run a single turn and print the result (no REPL).

---

## Task 10: README

**Files:** Create `examples/pi-sema/README.md`

- What pi-sema is, pi.dev attribution
- Installation: just needs `sema` binary + API key
- Usage: `sema examples/pi-sema/main.sema`
- One-shot: `sema examples/pi-sema/main.sema -- -p "explain this codebase"`
- Architecture overview (module responsibilities)
- Slash commands reference
- AGENTS.md support docs
- Session management docs
- Comparison with pi.dev (what's included, what's not)

---

## Implementation Order

1. **Task 1** (util.sema) — foundation, no deps
2. **Task 2** (display.sema) — depends on util
3. **Task 3** (tools.sema) — depends on util, core functionality
4. **Task 4** (context.sema) — depends on util
5. **Task 5** (session.sema) — depends on util
6. **Task 6** (compact.sema) — depends on session
7. **Task 7** (commands.sema) — depends on session, compact, display
8. **Task 8** (agent.sema) — depends on tools, context, display
9. **Task 9** (main.sema) — depends on everything
10. **Task 10** (README.md) — last

Tasks 1-5 can be done in parallel. Tasks 6-8 depend on earlier tasks. Task 9 integrates everything.
