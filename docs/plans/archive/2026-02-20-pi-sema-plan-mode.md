# Pi-Sema Plan Mode: Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a plan-before-execute workflow to pi-sema with tool gating, so the agent investigates and plans before making changes, then executes in checkpointed batches with human approval.

**Architecture:** Three-mode state machine (`:normal`, `:plan`, `:execute`) controlled via slash commands. Two separate `defagent` instances — a read-only planning agent and the full execution agent. Plan text stored in state and persisted to session NDJSON. Execution injects the plan as context and batches steps with `/next` continuation.

**Tech Stack:** Sema Lisp — `defagent`, `agent/run`, `llm/chat`, existing state map pattern, NDJSON session persistence.

---

## Design Decisions

### Why two agents instead of prompt-only gating?

`on-tool-call` is observational-only — it cannot block tool execution. And `agent/run` pulls tools from the `Agent` struct with no per-turn override. So the only way to truly prevent writes during planning is to create a separate agent with only read-only tools. This is a real guardrail, not a polite instruction.

### Why plain text plans instead of structured data?

Structured plans (parsed step lists with `:done?` flags) would require a plan parser, and LLM output is unreliable for strict formats. Plain text plans work because:
- The LLM generates them naturally
- `/plan show` just prints the stored string
- Execution mode injects the plan as context and tells the agent "do the next few steps"
- Step tracking happens via the agent's own awareness of what it's done, plus user checkpoints

### How does the plan survive compaction?

The plan is stored out-of-band in `:plan` on the state map, NOT in the message history. Each execution turn re-injects a "Pinned Plan" user message at the front of the message list. This means compaction can summarize execution history without losing the plan itself.

---

## File Changes Overview

```
Modified:
  tools.sema      — Add read-only-tools export function
  agent.sema      — Add create-planning-agent, plan-aware system prompts, mode-aware run-turn
  commands.sema   — Add /plan, /approve, /next, /mode commands; update /help, /status
  display.sema    — Add mode-aware prompt, plan display helpers
  main.sema       — Add :mode and :plan to initial state; route execute-mode input through /next
  session.sema    — Add "plan" NDJSON entry type support
```

No new files needed — this extends the existing module structure.

---

## Task 1: Read-Only Tool Set (`tools.sema`)

**Files:** Modify `examples/pi-sema/tools.sema`

Add a function that returns only the investigation tools (no write-file, edit-file, or bash).

**What to add** (after the `all-tools` definition at line 237-238):

```scheme
(define (read-only-tools)
  (list read-file grep find-files list-dir))
```

This is the tool set the planning agent will use. The agent can read files, search, find, and list — but cannot write, edit, or run commands.

**Verification:** `cargo run -- -e '(load "examples/pi-sema/tools.sema") (println (length (read-only-tools)))'` should print `4`.

---

## Task 2: Planning Agent and Mode-Aware System Prompts (`agent.sema`)

**Files:** Modify `examples/pi-sema/agent.sema`

### Step 1: Add planning system prompt builder

Add after `build-system-prompt` (after line 62):

```scheme
(define (build-planning-prompt cwd platform context-content skills-xml goal)
  (let ((sections '()))

    ;; Persona — planning mode
    (set! sections (append sections (list
      "You are pi-sema in PLANNING MODE. You are investigating a codebase and creating an implementation plan. You MUST NOT make any changes — only read, search, and analyze.")))

    ;; Planning rules
    (set! sections (append sections (list
      "## Planning Rules

1. Investigate the codebase thoroughly before writing the plan.
2. Use read-file, grep, find-files, and list-dir to understand the code.
3. You have NO write tools — you cannot edit files or run commands.
4. Your final output must be a concrete implementation plan with:
   - Exact file paths to create or modify
   - What changes to make in each file (specific, not vague)
   - Verification steps (commands to run, tests to check)
   - Steps ordered by dependency
5. Keep the plan concise — each step should be 2-5 minutes of work.
6. If anything is unclear, ask the user before finalizing the plan.")))

    ;; Environment
    (set! sections (append sections (list
      (format "## Environment

Working directory: ~a
Platform: ~a
Date: ~a" cwd platform (timestamp)))))

    ;; Goal
    (set! sections (append sections (list
      (format "## Goal

~a" goal))))

    ;; Project context
    (when (not (= context-content ""))
      (set! sections (append sections (list
        (string-append "## Project Context\n\n" context-content)))))

    ;; Skills
    (when (not (= skills-xml ""))
      (set! sections (append sections (list skills-xml))))

    (string/join sections "\n\n")))
```

### Step 2: Add execution system prompt injection

Add after the planning prompt builder:

```scheme
(define (build-execution-preamble plan)
  (format "## Active Plan

You are executing the following plan. Do the next 2-3 steps, then STOP and report what you completed. Do not continue beyond 3 steps — the user will type /next to continue.

If you encounter a blocker (missing dependency, test failure, unclear instruction), STOP immediately and explain the issue. Do not guess.

---

~a

---

Execute the next unfinished steps now." plan))
```

### Step 3: Add create-planning-agent function

Add after `create-agent` (after line 79):

```scheme
(define (create-planning-agent cwd model goal)
  (set-workspace-root! cwd)
  (let* ((platform (format "~a (~a)" (sys/os) (sys/arch)))
         (context (load-context-files cwd))
         (skills (discover-skills cwd))
         (skills-xml (format-skills-xml skills))
         (prompt (build-planning-prompt cwd platform context skills-xml goal)))
    (defagent pi-sema-planner
      {:system prompt
       :tools (read-only-tools)
       :max-turns 30
       :model model})))
```

### Step 4: Add mode-aware run-turn

Replace the existing `run-turn` function (line 85-86) with:

```scheme
(define (run-turn agent input messages)
  (agent/run agent input {:on-tool-call on-tool-call :messages messages}))

(define (run-plan-turn state input)
  "Run a turn in planning mode with a fresh planning agent."
  (let* ((planning-agent (create-planning-agent
                           (get state :cwd)
                           (get state :model)
                           input))
         (result (agent/run planning-agent input {:on-tool-call on-tool-call})))
    ;; Planning agent runs with empty messages — fresh context
    ;; Return the plan text as a result map
    {:response result :plan result}))

(define (run-execute-turn state)
  "Run an execution turn, injecting the plan as context."
  (let* ((plan (get state :plan ""))
         (preamble (build-execution-preamble plan))
         (messages (get state :messages '()))
         ;; Inject plan preamble as first user message if not already present
         (exec-messages (if (null? messages)
                          (list {:role "user" :content preamble})
                          ;; Replace first message with updated preamble
                          (cons {:role "user" :content preamble}
                                (cdr messages))))
         (agent (get state :agent)))
    (agent/run agent "Continue executing the plan. Do the next 2-3 steps."
      {:on-tool-call on-tool-call :messages exec-messages})))
```

**Verification:** Load agent.sema and confirm `create-planning-agent` and `run-plan-turn` are defined without errors.

---

## Task 3: Plan Slash Commands (`commands.sema`)

**Files:** Modify `examples/pi-sema/commands.sema`

### Step 1: Add plan commands to dispatch

Replace the dispatch-command cond (lines 38-50):

```scheme
(define (dispatch-command cmd state)
  "Dispatch a parsed command to its handler. Returns updated state or 'quit."
  (let ((name (get cmd :name ""))
        (args (get cmd :args "")))
    (cond
      ((equal? name "help")    (cmd-help state))
      ((equal? name "clear")   (cmd-clear state))
      ((equal? name "model")   (cmd-model args state))
      ((equal? name "compact") (cmd-compact state))
      ((equal? name "session") (cmd-session args state))
      ((equal? name "usage")   (cmd-usage state))
      ((equal? name "status")  (cmd-status state))
      ((equal? name "plan")    (cmd-plan args state))
      ((equal? name "approve") (cmd-approve state))
      ((equal? name "next")    (cmd-next state))
      ((equal? name "quit")    'quit)
      ((equal? name "exit")    'quit)
      (else
        (show-error (format "Unknown command: /~a. Type /help for available commands." name))
        state))))
```

### Step 2: Add /plan command

Add after the /status section:

```scheme
;; ============================================================
;; Section 10: /plan [goal]
;; ============================================================

(define (cmd-plan args state)
  (let ((current-mode (get state :mode :normal)))
    (cond
      ;; /plan show — display current plan
      ((equal? args "show")
       (let ((plan (get state :plan "")))
         (if (= plan "")
           (show-info "No active plan.")
           (begin
             (println-error "")
             (println-error (term/style "  Active Plan:" :bold))
             (println-error "")
             (println plan)
             (println-error ""))))
       state)

      ;; /plan clear — discard plan and return to normal mode
      ((equal? args "clear")
       (show-info "Plan cleared. Returning to normal mode.")
       (assoc (assoc state :mode :normal) :plan ""))

      ;; /plan <goal> — enter planning mode and generate a plan
      ((not (= args ""))
       (show-info (format "Entering planning mode: ~a" args))
       (show-info "Investigating codebase with read-only tools...")
       (println-error "")
       (try
         (let* ((result (run-plan-turn state args))
                (plan-text (get result :plan "")))
           (show-response plan-text)
           (show-info "Plan generated. Type /approve to execute, /plan clear to discard.")
           ;; Save plan to session
           (try
             (session/append (get state :session)
               {:type "plan"
                :goal args
                :content plan-text
                :timestamp (timestamp)})
             (catch e #f))
           (assoc (assoc state :mode :plan) :plan plan-text))
         (catch e
           (show-error (format "Planning failed: ~a" (get e :message (str e))))
           state)))

      ;; /plan with no args — show usage
      (else
        (show-info "Usage: /plan <goal> | /plan show | /plan clear")
        state))))
```

### Step 3: Add /approve command

```scheme
;; ============================================================
;; Section 11: /approve
;; ============================================================

(define (cmd-approve state)
  (let ((mode (get state :mode :normal))
        (plan (get state :plan "")))
    (cond
      ((= plan "")
       (show-error "No plan to approve. Use /plan <goal> first.")
       state)
      ((equal? mode :execute)
       (show-info "Already in execution mode. Use /next to continue.")
       state)
      (else
       (show-info "Plan approved. Entering execution mode.")
       (show-info "Starting execution — the agent will do 2-3 steps then pause.")
       (show-info "Type /next to continue, /plan show to review, /plan clear to abort.")
       (println-error "")
       (try
         (let* ((result (run-execute-turn (assoc state :mode :execute)))
                (response (get result :response ""))
                (messages (get result :messages '())))
           (show-response response)
           ;; Save to session
           (try
             (begin
               (session/save-message (get state :session) "user" "[plan execution: initial batch]")
               (session/save-message (get state :session) "assistant" response))
             (catch e #f))
           (show-info "Batch complete. Type /next to continue, /plan clear to stop.")
           (assoc (assoc (assoc state :mode :execute) :messages messages) :plan (get state :plan)))
         (catch e
           (show-error (format "Execution error: ~a" (get e :message (str e))))
           state))))))
```

### Step 4: Add /next command

```scheme
;; ============================================================
;; Section 12: /next
;; ============================================================

(define (cmd-next state)
  (let ((mode (get state :mode :normal))
        (plan (get state :plan "")))
    (cond
      ((not (equal? mode :execute))
       (show-error "Not in execution mode. Use /plan <goal> then /approve first.")
       state)
      ((= plan "")
       (show-error "No active plan. Use /plan <goal> to create one.")
       state)
      (else
       (show-info "Continuing execution...")
       (println-error "")
       (try
         (let* ((result (run-execute-turn state))
                (response (get result :response ""))
                (messages (get result :messages '())))
           (show-response response)
           ;; Save to session
           (try
             (begin
               (session/save-message (get state :session) "user" "[plan execution: next batch]")
               (session/save-message (get state :session) "assistant" response))
             (catch e #f))
           ;; Auto-compact if needed
           (let ((compacted (auto-compact
                              (get state :session)
                              messages
                              (get state :model)
                              80000)))
             (show-info "Batch complete. Type /next to continue, /plan clear when done.")
             (assoc (assoc state :messages compacted) :plan plan)))
         (catch e
           (show-error (format "Execution error: ~a" (get e :message (str e))))
           state))))))
```

### Step 5: Update /help to show plan commands

Replace the `cmd-help` function body to add plan commands after `/compact`:

Add these lines after the `/compact` help line and before `/session list`:

```scheme
  (println-error (format "    ~a  Investigate and create a plan"
    (term/cyan (format "~a" "/plan <goal>     "))))
  (println-error (format "    ~a  Show current plan"
    (term/cyan (format "~a" "/plan show       "))))
  (println-error (format "    ~a  Discard plan, return to normal"
    (term/cyan (format "~a" "/plan clear      "))))
  (println-error (format "    ~a  Approve plan, start execution"
    (term/cyan (format "~a" "/approve         "))))
  (println-error (format "    ~a  Execute next batch of steps"
    (term/cyan (format "~a" "/next            "))))
```

### Step 6: Update /status to show mode and plan

Add mode and plan lines to `cmd-status` after the model line:

```scheme
    (println-error (term/dim (format "  Mode:           ~a" (get state :mode :normal))))
    (when (not (= (get state :plan "") ""))
      (println-error (term/dim (format "  Plan:           ~a" (truncate-str (get state :plan "") 50)))))
```

**Verification:** Load commands.sema and confirm dispatch-command handles "plan", "approve", and "next" without errors.

---

## Task 4: Mode-Aware Display (`display.sema`)

**Files:** Modify `examples/pi-sema/display.sema`

### Step 1: Add mode-aware prompt

Replace `show-prompt` (line 23-24):

```scheme
(define (show-prompt . args)
  (let ((mode (if (null? args) :normal (car args))))
    (cond
      ((equal? mode :plan)
       (print-error (term/style "pi-sema [plan] ❯ " :bold :yellow)))
      ((equal? mode :execute)
       (print-error (term/style "pi-sema [exec] ❯ " :bold :green)))
      (else
       (print-error (term/style "pi-sema ❯ " :bold :magenta))))))
```

**Verification:** Visual — the prompt should change color/label based on mode.

---

## Task 5: Main Loop Mode Integration (`main.sema`)

**Files:** Modify `examples/pi-sema/main.sema`

### Step 1: Add :mode and :plan to initial state

Update `current-state` (lines 90-95) to include the new keys:

```scheme
    (define current-state
      {:session session
       :messages '()
       :model model
       :cwd cwd
       :agent agent
       :mode :normal
       :plan ""})
```

### Step 2: Pass mode to show-prompt

Update the REPL loop (lines 153-156) to pass mode:

```scheme
    (let loop ()
      (show-prompt (get current-state :mode :normal))
      (handle-input (read-line))
      (loop))
```

### Step 3: Block free-form input in execute mode

In the `handle-input` function's `else` branch (line 124), add a mode check before running the agent turn. When in `:execute` mode, free-form input should be rejected with a hint to use `/next`:

Update the `else` branch (lines 124-151):

```scheme
        ;; Agent turn
        (else
         (set! eof-count 0)
         (let ((trimmed (string/trim input))
               (mode (get current-state :mode :normal)))

           ;; In execute mode, nudge user to use /next
           (when (equal? mode :execute)
             (show-info "In execution mode. Use /next to continue, or /plan clear to exit.")
             #f)

           (when (not (equal? mode :execute))
             (define result
               (try
                 (run-turn (:agent current-state) trimmed (:messages current-state))
                 (catch e
                   (show-error (format "Agent error: ~a" (get e :message (str e))))
                   #f)))

             (when result
               (show-response (:response result))
               (set! current-state (assoc current-state :messages (:messages result)))

               ;; Save to session
               (try
                 (begin
                   (session/save-message (:session current-state) "user" trimmed)
                   (session/save-message (:session current-state) "assistant" (:response result)))
                 (catch e #f))

               ;; Auto-compact if needed
               (define compacted (auto-compact
                                   (:session current-state)
                                   (:messages current-state)
                                   (:model current-state)
                                   80000))
               (set! current-state (assoc current-state :messages compacted))))))
```

**Verification:** Start pi-sema, confirm the prompt shows `pi-sema ❯ ` in normal mode. After `/plan <goal>`, confirm it shows `pi-sema [plan] ❯ `. After `/approve`, confirm it shows `pi-sema [exec] ❯ `.

---

## Task 6: Session Persistence for Plans (`session.sema`)

**Files:** Modify `examples/pi-sema/session.sema`

### Step 1: Handle plan entries in session/load

In the `session/load` function, after building messages (around line 91), add plan detection:

Add a `plan` binding after the `messages` binding:

```scheme
           ;; Find latest plan entry (if any)
           (plan
             (let loop ((rest (reverse entries)) (found ""))
               (if (null? rest)
                 found
                 (if (equal? (get (car rest) :type) "plan")
                   (get (car rest) :content "")
                   (loop (cdr rest) found)))))
```

And add `:plan plan` to the returned map (line 102-106):

```scheme
      {:id (get header :id)
       :path path
       :messages messages
       :model model
       :plan plan
       :cwd (get header :cwd)}
```

**Verification:** Create a session, use `/plan`, save, then `/session resume` to confirm the plan survives.

---

## Task 7: Update README (`README.md`)

**Files:** Modify `examples/pi-sema/README.md`

Add a "Plan Mode" section after the "Slash Commands" section:

```markdown
## Plan Mode

pi-sema supports a plan-before-execute workflow for complex tasks:

1. **Create a plan:** `/plan <goal>` — The agent investigates the codebase with read-only tools (no writes allowed) and generates a concrete implementation plan.
2. **Review the plan:** The plan is displayed immediately. Use `/plan show` to re-read it.
3. **Approve and execute:** `/approve` starts execution. The agent executes 2-3 plan steps, then pauses for your review.
4. **Continue:** `/next` executes the next batch of steps. Repeat until done.
5. **Abort:** `/plan clear` discards the plan and returns to normal mode.

### Why plan mode?

- **Alignment** — You see exactly what the agent intends before any files change.
- **Safety** — Planning uses a separate read-only agent that physically cannot write files or run commands.
- **Control** — Execution pauses every 2-3 steps so you can course-correct.

Plans are persisted to the session file, so they survive `/session resume`.
```

Add the new commands to the slash commands table:

```markdown
| `/plan <goal>` | Investigate codebase and generate a plan |
| `/plan show` | Display the current plan |
| `/plan clear` | Discard plan, return to normal mode |
| `/approve` | Approve plan and start execution |
| `/next` | Execute next batch of plan steps |
```

---

## Implementation Order

1. **Task 1** (tools.sema) — 1 line, no deps
2. **Task 2** (agent.sema) — planning agent + execution prompts, depends on Task 1
3. **Task 3** (commands.sema) — slash commands, depends on Task 2
4. **Task 4** (display.sema) — mode-aware prompt, no deps
5. **Task 5** (main.sema) — state + loop integration, depends on Tasks 3-4
6. **Task 6** (session.sema) — plan persistence, independent
7. **Task 7** (README.md) — docs, last

Tasks 1, 4, and 6 can be done in parallel. Tasks 2 → 3 → 5 are sequential. Task 7 is last.
