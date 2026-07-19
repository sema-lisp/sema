/// Built-in macros loaded at interpreter startup.
/// These expand to core special forms and don't require evaluator changes.
pub const PRELUDE: &str = r#"
;; Thread-first: inserts val as the FIRST argument of each form
;; (-> 5 (+ 3) (* 2)) => (* (+ 5 3) 2) => 16
(defmacro -> (val . forms)
  (if (null? forms)
    val
    (let ((form (car forms))
          (rest (cdr forms)))
      (if (list? form)
        `(-> (,(car form) ,val ,@(cdr form)) ,@rest)
        `(-> (,form ,val) ,@rest)))))

;; Thread-last: inserts val as the LAST argument of each form
;; (->> (range 10) (filter odd?) (map square)) => (map square (filter odd? (range 10)))
(defmacro ->> (val . forms)
  (if (null? forms)
    val
    (let ((form (car forms))
          (rest (cdr forms)))
      (if (list? form)
        `(->> (,(car form) ,@(cdr form) ,val) ,@rest)
        `(->> (,form ,val) ,@rest)))))

;; Thread-as: binds val to a name, allowing arbitrary placement
;; (as-> 5 x (+ x 3) (* x x) (- x 1)) => 63
(defmacro as-> (val name . forms)
  (if (null? forms)
    val
    (let ((form (car forms))
          (rest (cdr forms)))
      `(let ((,name ,val))
         (as-> ,form ,name ,@rest)))))

;; Conditional thread-first: short-circuits on nil
;; (some-> m :key :nested) => nil if any step returns nil
(defmacro some-> (val . forms)
  (if (null? forms)
    val
    (let ((form (car forms))
          (rest (cdr forms)))
      (if (list? form)
        `(let ((v# ,val))
           (if (nil? v#) nil (some-> (,(car form) v# ,@(cdr form)) ,@rest)))
        `(let ((v# ,val))
           (if (nil? v#) nil (some-> (,form v#) ,@rest)))))))

;; when-let: bind a value, execute body only if non-nil
;; (when-let (x (get m :key)) (println x))
(defmacro when-let (binding . body)
  (let ((var (car binding))
        (expr (cadr binding)))
    `(let ((,var ,expr))
       (when (not (nil? ,var))
         ,@body))))

;; if-let: bind a value, branch on nil/non-nil
;; (if-let (x (get m :key)) (use x) (default))
(defmacro if-let (binding then else)
  (let ((var (car binding))
        (expr (cadr binding)))
    `(let ((,var ,expr))
       (if (nil? ,var) ,else ,then))))

;; with-stream: bind stream, execute body, auto-close on exit (even on error)
;; (with-stream (s (stream/open-input "f.txt")) (stream/read-all s))
(defmacro with-stream (binding . body)
  (let ((var (car binding))
        (expr (cadr binding)))
    `(let ((,var ,expr))
       (let ((res# (try (begin ,@body)
                     (catch e#
                       (stream/close ,var)
                       (throw e#)))))
         (stream/close ,var)
         res#))))

;; with-open: RAII cleanup alias of with-stream, so files and sockets share one
;; closeable form. Closes the bound resource on both the normal and error paths.
;; (with-open (sock (ws/connect "wss://…")) (ws/send sock "hi") (ws/recv sock))
(defmacro with-open (binding . body)
  `(with-stream ,binding ,@body))

;; guard: R7RS structured exception handling.
;; (guard (var clause ...) body ...) evaluates body; if a condition is raised
;; (via `raise`/`throw` or a native runtime error), it is bound to var and the
;; clauses are evaluated like `cond` (an `else` clause, if present, must be
;; last). If no clause matches, the condition is re-raised in guard's own
;; dynamic position (guard's try/catch has already unwound body's frames, so
;; the re-raise propagates from here, not from deep inside body).
;;
;; R7RS semantics: var is bound to the RAISED OBJECT. For (raise obj)/(throw
;; obj) that is obj itself (the raw value) — so clause tests read it directly:
;;   (guard (e ((string? e) e) (else :unknown)) (raise "x")) ; => "x"
;; A native runtime error ((/ 1 0), (car 5), (error "msg"), an unbound
;; variable, …) has no raw raised object, so var is Sema's error MAP
;; ({:type ... :message ... :value ...}); dispatch on (:type e)/(:message e).
;; A clause that wants only native errors should gate on (map? e) first, since
;; keyword access like (:type e) raises a type error on a raw non-map object.
;;
;; NOTE: (car '()) / (first []) return nil in Sema (a deliberate safe-accessor
;; deviation from R7RS `car`), so they do NOT raise and guard never fires on
;; them — use (car 5) or (/ 1 0) to see guard catch a native runtime error.
(defmacro guard (spec . body)
  (let ((var (car spec))
        (clauses (cdr spec)))
    (let ((has-else
            (and (not (null? clauses))
                 (let ((last-clause (car (reverse clauses))))
                   (and (list? last-clause)
                        (equal? (car last-clause) 'else))))))
      `(try
         (begin ,@body)
         (catch guard-err#
           ;; Unwrap the {:type :user :value obj} wrapper so var is the raw
           ;; raised object; native errors (non-:user) stay as the error map.
           (let ((,var (if (equal? (:type guard-err#) :user)
                           (:value guard-err#)
                           guard-err#)))
             (cond ,@clauses
                   ,@(if has-else
                         (list)
                         ;; No clause matched: re-raise the object bound to var.
                         ;; Re-raising var (not the wrapper) keeps re-raise
                         ;; faithful — an outer guard again unwraps :user and
                         ;; recovers the same raw object (or native error map).
                         (list (list 'else (list 'raise var)))))))))))

;; ws/listen: drive a receive loop on a websocket, dispatching each frame to the
;; matching handler. Spawns an async task and returns its promise — await it (or
;; otherwise keep driving the runtime) to observe the loop. All handlers are optional:
;;   :on-open    (fn (conn) …)      — called once before the loop starts
;;   :on-message (fn (conn msg) …)  — msg is the text string or binary bytevector
;;   :on-close   (fn (conn info) …) — info is {:code … :reason …}
;;   :on-error   (fn (conn err) …)  — a recv/protocol error; the loop then stops
;; (async/await (ws/listen sock {:on-message (fn (c m) (println m))}))
(defmacro ws/listen (conn handlers)
  `(let ((conn# ,conn)
         (hs# ,handlers))
     (let ((on-open# (get hs# :on-open))
           (on-message# (get hs# :on-message))
           (on-close# (get hs# :on-close))
           (on-error# (get hs# :on-error)))
       (when on-open# (on-open# conn#))
       (async/spawn
         (fn ()
           (let loop ()
             (let ((msg# (try (ws/recv conn#)
                           (catch e#
                             (when on-error# (on-error# conn# e#))
                             :ws-listen-error))))
               (cond
                 ((= msg# :ws-listen-error) nil)
                 ((null? msg#)
                  (when on-close# (on-close# conn# {:code 1006 :reason "closed"})))
                 ((not (null? (get msg# :text)))
                  (when on-message# (on-message# conn# (get msg# :text)))
                  (loop))
                 ((not (null? (get msg# :binary)))
                  (when on-message# (on-message# conn# (get msg# :binary)))
                  (loop))
                 ((not (null? (get msg# :close)))
                  (when on-close# (on-close# conn# (get msg# :close))))
                 (else (loop))))))))))
;; Terminal setup/teardown guards. Each runs BODY and ALWAYS restores on exit —
;; even if BODY throws — so a crash can't leave the terminal broken. They mirror
;; with-stream's try/catch-rethrow-then-cleanup shape and return BODY's value.
;; Compose them (outermost restores last): typically
;;   (io/with-raw-mode (term/with-alt-screen (term/with-mouse ...body...)))
;;
;; TODO: these enable/disable guards are pure boilerplate. One option is a
;; compile-time Rust helper (macro_rules! or a codegen fn) that emits the guard
;; Sema source from a (name, enable, disable) table and appends it to this
;; prelude string. Deferred — and maybe not worth it: it trades the prelude's
;; "just readable Sema" property for DRY, `io/with-raw-mode` doesn't fit the
;; shape (it threads a restore token from setup into teardown), and it needs its
;; own thorough test pass. Left explicit for now.

;; Enter the alternate screen + hide the cursor; restore both on exit.
(defmacro term/with-alt-screen (. body)
  `(begin
     (term/enter-alt-screen)
     (term/hide-cursor)
     (let ((res# (try (begin ,@body)
                   (catch e#
                     (term/show-cursor)
                     (term/leave-alt-screen)
                     (throw e#)))))
       (term/show-cursor)
       (term/leave-alt-screen)
       res#)))

;; Put the TTY in raw mode; restore cooked mode on exit. An unrestored raw TTY
;; leaves the shell unusable (no echo, no line editing), so this guard matters
;; most. Binds nothing (the restore token is handled internally).
(defmacro io/with-raw-mode (. body)
  `(let ((raw# (io/tty-raw!)))
     (let ((res# (try (begin ,@body)
                   (catch e#
                     (when raw# (io/tty-restore! raw#))
                     (throw e#)))))
       (when raw# (io/tty-restore! raw#))
       res#)))

;; Enable mouse reporting; disable it on exit so mouse escape reports don't spew
;; into the shell afterward.
(defmacro term/with-mouse (. body)
  `(begin
     (term/enable-mouse)
     (let ((res# (try (begin ,@body)
                   (catch e#
                     (term/disable-mouse)
                     (throw e#)))))
       (term/disable-mouse)
       res#)))

;; Enable bracketed paste; disable on exit so paste markers can't leak to the
;; shell. io/read-key returns pasted text as {:kind :paste :text …}.
(defmacro term/with-bracketed-paste (. body)
  `(begin
     (term/enable-bracketed-paste)
     (let ((res# (try (begin ,@body)
                   (catch e#
                     (term/disable-bracketed-paste)
                     (throw e#)))))
       (term/disable-bracketed-paste)
       res#)))

;; Enable focus reporting; disable on exit. io/read-key returns focus in/out as
;; {:kind :focus :focused #t|#f}.
(defmacro term/with-focus-events (. body)
  `(begin
     (term/enable-focus-events)
     (let ((res# (try (begin ,@body)
                   (catch e#
                     (term/disable-focus-events)
                     (throw e#)))))
       (term/disable-focus-events)
       res#)))

;; Push kitty keyboard flags (default 17 = disambiguate + associated-text) and
;; pop them on exit. Terminals without kitty support ignore it, so this is safe
;; to wrap unconditionally.
(defmacro term/with-kitty-keys (. body)
  `(begin
     (term/enable-kitty-keys!)
     (let ((res# (try (begin ,@body)
                   (catch e#
                     (term/disable-kitty-keys!)
                     (throw e#)))))
       (term/disable-kitty-keys!)
       res#)))

;; dotimes: execute body n times with a counter variable
;; (dotimes (i 10) (println i)) — prints 0..9
(defmacro dotimes (binding . body)
  (let ((var (car binding))
        (count (cadr binding)))
    `(do ((,var 0 (+ ,var 1)))
       ((= ,var ,count))
       ,@body)))

;; for-range: loop from start to end (exclusive) with optional step
;; (for-range (i 0 10) (println i)) — prints 0..9
;; (for-range (i 0 10 2) (println i)) — prints 0,2,4,6,8
(defmacro for-range (binding . body)
  (let ((var (car binding))
        (start (cadr binding))
        (end (caddr binding))
        (step (if (null? (cdddr binding)) 1 (car (cdddr binding)))))
    `(do ((,var ,start (+ ,var ,step)))
       ((>= ,var ,end))
       ,@body)))

;; with-span: run body inside a named tracing span carrying an attributes map.
;; Ends the span on exit (Error status if the body throws); returns the body's value.
;; (with-span "ingest" {:batch.size 100} (process)) — use {} for no attributes.
(defmacro with-span (name attrs . body)
  `(otel/span ,name (lambda () ,@body) ,attrs))

;; with-session: group every span started in body into a session (Langfuse Sessions/Users).
;; (with-session "chat-42" {:user "alice"} (llm/complete "...")) — use {} for no user.
(defmacro with-session (id config . body)
  `(otel/with-session ,id ,config (lambda () ,@body)))

;; defworkflow: define + run a sequential, journaled workflow.
;; (defworkflow audit-auth "doc" {:phases [...] :budget {:tokens N :usd N}} (phase ...) ...)
;; The meta map's `:budget` submap caps spend: `:tokens` (deterministic) and/or `:usd`
;; (best-effort, pricing-table dependent). Exceeding a cap latches the run and refuses
;; to launch further `step` leaves; the run ends {:status :failed :reason "budget
;; exceeded"}. Under a concurrent fan-out the cap still trips, but per-step token
;; accounting is best-effort (the LAST_USAGE thread-local is not swapped per task).
;; The cap is PER-INVOCATION: a `--resume` run starts spend at 0 (memoized leaves
;; replay for free and don't recharge), so it does not carry the prior run's spend.
;; expands to a (workflow/run name doc meta thunk) call. `name` is a bare symbol that
;; becomes a string; `meta` is the metadata map literal, spliced verbatim (like the
;; with-span attrs map at :103); the body forms become the run thunk. workflow/run opens
;; the journal sink under ./.sema/runs/<run-id>/, emits run.started/run.ended, writes
;; result.json, and returns the {:status :success :value ...} / {:status :failed ...}
;; envelope. Keeping defworkflow a macro leaves the VM untouched (deftool/defagent are
;; special forms, but this matches the ->/when-let prelude family).
;;
;; :mcp {alias {...spec...} ...} declares MCP servers workflow/run auth-resolves +
;; connects BEFORE the body thunk runs (docs/plans/2026-06-24-workflow-mcp-auth.md
;; §2/§3). The surface syntax binds each declared alias as an ORDINARY VARIABLE in
;; the body — `(mcp/call asana "create_task" ...)`, not `(mcp/call (workflow/mcp-handle
;; 'asana) ...)`. `:mcp`'s alias keys are bare symbols (`{asana {...}}`, not `{:asana
;; {...}}` or `{"asana" {...}}`), so evaluating that submap AS ORDINARY CODE would try
;; to look up `asana` as a variable and fail (map literals evaluate both keys and
;; values — see lower.rs's MakeMap). `:mcp` is meant to be fully static anyway (the
;; plan's "declared, checked before any user code runs" design), so when `meta` is a
;; LITERAL map with a LITERAL `:mcp` submap, this macro (a) re-quotes that submap in
;; the spliced meta, so it evaluates to itself (symbol keys intact) rather than being
;; evaluated as code, and (b) wraps the body in a `let` binding each alias to
;; `(workflow/mcp-handle (quote alias))`, one per declared alias, in the map's own
;; (BTreeMap/Value::Ord) key order. A non-literal `meta`, or a `meta` with no literal
;; `:mcp` key, leaves the expansion unchanged — `:mcp` MUST be inspectable at
;; macro-expansion time; a computed meta expression can't declare it.
;;
;; Safety of calling workflow/mcp-handle from these bindings: the `let` wrapping only
;; wraps `body`, and `body` is the workflow/run THUNK — workflow/run doesn't invoke
;; the thunk until after its auth-resolution step has resolved every declared server
;; (or the run has already exited via the needs-auth/failed gate without running the
;; thunk at all). So by the time these bindings are ever evaluated, workflow/mcp-handle
;; always has a resolved handle to return.
(defmacro defworkflow (name doc meta . body)
  (let* ((mcp-decl (if (map? meta) (get meta :mcp) nil))
         (has-mcp? (map? mcp-decl)))
    (if has-mcp?
      (let* ((aliases (keys mcp-decl))
             (quoted-meta (assoc meta :mcp (list (quote quote) mcp-decl)))
             (bindings (map (fn (a) (list a (list (quote workflow/mcp-handle) (list (quote quote) a))))
                          aliases)))
        `(workflow/run (symbol->string (quote ,name)) ,doc ,quoted-meta
           (lambda () (let ,bindings ,@body))))
      `(workflow/run (symbol->string (quote ,name)) ,doc ,meta (lambda () ,@body)))))

;; phase: a journaled MARKER inside a workflow body (workflow.js semantics) — not a
;; wrapper, not control flow. `(phase "Audit")` closes the previously-open phase and
;; opens "Audit"; every `step`/`checkpoint` that follows attributes to it until the
;; next `(phase …)` or the run end (which closes the last open phase). Returns nil.
(defmacro phase (label)
  `(workflow/phase ,label))

;; checkpoint: a run-scoped state write/read. The write form delays its value
;; expression so `workflow/checkpoint` can return a resume memo before evaluating
;; expensive or side-effecting work.
(defmacro checkpoint (key . rest)
  (cond
    ((null? rest) `(workflow/checkpoint ,key))
    ((null? (cdr rest)) `(workflow/checkpoint ,key (fn () ,(car rest))))
    (else (error "checkpoint takes 1 or 2 arguments"))))

;; step: a journaled orchestration unit (workflow.js `step(prompt, {…})`) — the
;; workflow's atomic call site, anonymous and workflow-owned (unlike `agent`, the
;; named/reusable actor). Runs the prompt through the configured provider and returns
;; TYPED DATA when `:schema` is supplied (validated via `llm/extract`), or the
;; completion text otherwise. The optional opts map carries `:name` (the role label
;; shown in the dashboard, default "step"), `:schema`, `:tools`, and `:agent`. The
;; call is wrapped by `workflow/step`, which emits agent.started/agent.result + a
;; per-step budget event. (The `agent.*` event names are the FROZEN internal journal
;; contract — they predate the step rename and stay; `agent_name` carries the step's
;; role label, or the defagent's name on the `:agent` path.)
;;
;;   (step "List the auth-relevant files under src/." {:name "scout" :schema [:list :string]})
;;   (step "Summarize the changelog.")                  ; no schema -> returns text
;;
;; Routing on opts (`:agent` and inline `:tools`/`:model` are mutually exclusive — the
;; agent owns those; the static checker warns if both are given):
;;   :agent A     -> run the configured defagent A AS this step via `agent/run` (its
;;                   own system prompt + tools + model + max-turns), the prompt as the
;;                   user message. The agent's genuine tool calls still journal as
;;                   agent.tool_call through the same `:on-tool-call` shim. `:name`
;;                   defaults to A's own name. With `:schema`, A's text is validated.
;;   :tools [...] -> the REAL tool loop (via llm/chat, which owns run_tool_loop),
;;                   journaling each genuine tool call as an agent.tool_call event.
;;                   With no `:schema` it returns the loop's final TEXT.
;;   :schema S    -> llm/extract (typed data).
;;   else         -> llm/complete (text).
;;
;; The `:schema` validate path (shared by the `:agent` and `:tools` branches) is a
;; hybrid: first a PURE local parse (json/decode the final text + presence/type check —
;; no extra model call when the text already decodes to a satisfying value); when the
;; local parse fails it falls back to an `llm/extract` re-ask round IF re-asking can
;; help — for a non-map schema (e.g. `[:list :string]`, which llm/extract validates
;; structurally) or a map with a `:type` field. A bare-keyword field map is
;; presence-only (re-ask can't tighten it), so that case surfaces the raw text instead
;; of wasting a call.
;; Per-step budget for a multi-round tool loop is best-effort (the Budget event
;; reflects the final round's usage; LAST_USAGE is a single slot — same caveat as
;; fan-out accounting).
(defmacro step (prompt . rest)
  (let ((opts-form (if (null? rest) {} (car rest))))
    `(let ((st-opts0# ,opts-form)
           (st-prompt# ,prompt))
       (let* ((st-agent# (get st-opts0# :agent))
              ;; explicit :name wins; else on the :agent path default the role label to
              ;; the defagent's own name; else leave absent (workflow/step → "step").
              (st-opts# (if (and (not (nil? st-agent#)) (nil? (get st-opts0# :name)))
                          (assoc st-opts0# :name (agent/name st-agent#))
                          st-opts0#)))
         ;; Inject the resolved prompt + a stable schema repr so workflow/step can
         ;; compute this leaf's resume content-key (these `:__`-prefixed keys are
         ;; internal — read by the runtime, ignored by the dashboard).
         (workflow/step (assoc st-opts#
                          :__prompt (str st-prompt#)
                          :__schema-repr (str (get st-opts# :schema)))
           (fn ()
             (let ((st-schema# (get st-opts# :schema))
                   (st-tools# (get st-opts# :tools))
                   ;; pure local schema check: presence for every key, plus a type
                   ;; check for map specs that declare `:type` (mirrors the native
                   ;; validate_extraction in sema-llm — bare-keyword specs are
                   ;; presence-only, unknown type names always pass).
                   (st-valid?# (fn (v# s#)
                                 (and (map? v#) (map? s#)
                                   (every? (fn (k#)
                                             (let ((spec# (get s# k#)))
                                               (if (and (map? spec#) (contains? spec# :type))
                                                 (and (contains? v# k#)
                                                   (let ((tv# (get v# k#))
                                                         (ty# (get spec# :type)))
                                                     (cond
                                                       ((or (= ty# :string) (= ty# "string")) (string? tv#))
                                                       ((or (= ty# :number) (= ty# "number")) (number? tv#))
                                                       ((or (= ty# :boolean) (= ty# "boolean")
                                                            (= ty# :bool) (= ty# "bool")) (boolean? tv#))
                                                       ((or (= ty# :list) (= ty# "list")
                                                            (= ty# :array) (= ty# "array")) (list? tv#))
                                                       (else #t))))
                                                 (contains? v# k#))))
                                     (keys s#)))))
                   ;; true when an llm/extract re-ask can actually recover typed data:
                   ;; a non-map schema (e.g. [:list :string]) is structurally validated by
                   ;; llm/extract, so re-ask helps; a MAP schema helps only if some field
                   ;; declares :type (a bare-keyword field map is presence-only — re-asking
                   ;; can't tighten it, so we surface the raw text instead of wasting a call).
                   (st-typed-schema?# (fn (s#)
                                        (if (map? s#)
                                          (not (every? (fn (k#)
                                                         (let ((spec# (get s# k#)))
                                                           (not (and (map? spec#) (contains? spec# :type)))))
                                                 (keys s#)))
                                          #t)))
                   ;; the `:on-tool-call` shim — journals each genuine tool call as an
                   ;; agent.tool_call event. Shared by the `:agent` and `:tools` branches.
                   (st-on-tool# (fn (ev#)
                                  (when (= (:event ev#) "start")
                                    (workflow/tool-call (:tool ev#) (:args ev#))))))
               ;; validate the final text against `:schema` (no-op text passthrough when
               ;; no schema). Shared by the `:agent` and `:tools` branches.
               (let ((st-validate# (fn (txt# sch#)
                                     (if (nil? sch#)
                                       txt#
                                       (let ((parsed# (try (json/decode txt#)
                                                           (catch _e# :__step-parse-failed#))))
                                         (if (and (not (= parsed# :__step-parse-failed#))
                                                  (st-valid?# parsed# sch#))
                                           parsed#
                                           (if (st-typed-schema?# sch#)
                                             (llm/extract sch# txt#)
                                             txt#)))))))
                 (cond
                   ;; :agent branch — run the configured defagent as this step. agent/run
                   ;; with opts returns {:response text :messages [...]}.
                   ((not (nil? st-agent#))
                    (st-validate#
                      (:response (agent/run st-agent# st-prompt# {:on-tool-call st-on-tool#}))
                      st-schema#))
                   ;; :tools branch — real run_tool_loop, one agent.tool_call per call.
                   ((not (nil? st-tools#))
                    (st-validate#
                      (llm/chat (prompt (user st-prompt#))
                        {:tools st-tools# :on-tool-call st-on-tool#})
                      st-schema#))
                   ;; :schema branch — typed extract, with :sema-form sentinel.
                   ;; :sema-form → parse the LLM's completion text as Sema source,
                   ;; returning a list of top-level forms via read-many. All other
                   ;; schema values go through the generic llm/extract path.
                   ((not (nil? st-schema#))
                    (if (= st-schema# :sema-form)
                      (read-many (llm/complete st-prompt#))
                      (llm/extract st-schema# st-prompt#)))
                   ;; plain — text completion.
                   (else (llm/complete st-prompt#)))))))))))

;; workflow/run-form: evaluate a workflow form (or a list of top-level forms as
;; returned by a {:schema :sema-form} step) and return its {:status …} envelope.
;; A list whose first element is itself a list → eval each form in sequence,
;; return the last result (mirrors what top-level do does for multi-form files).
;; A single form (head is a symbol, e.g. defworkflow) → eval it directly.
(defmacro workflow/run-form (form)
  `(let ((wrf-form# ,form))
     (if (and (list? wrf-form#) (not (null? wrf-form#)) (list? (car wrf-form#)))
       (foldl (fn (_acc wrf-f#) (eval wrf-f#)) nil wrf-form#)
       (eval wrf-form#))))

;; llm/embed is a SINGLE first-class native function (crates/sema-llm/src/
;; builtins.rs) that branches internally on the runtime context: synchronous
;; inline at top level, offloaded+overlapping inside a spawned task. Keeping it a
;; native (not a router macro) is what makes `(procedure? llm/embed)` true and lets
;; it be used as a value — `(map llm/embed …)`, `(async/pool-map llm/embed …)`.

;; ── Owned-concurrency engine (Task 04) ───────────────────────────────────────
;; The OWNED combinators (`async/spawn-all`, `async/map`, `async/pool-map`,
;; `async/race-owned`, `async/with-timeout`) OWN the tasks they create: on a
;; fail-fast settlement (first failure / race winner / timeout) they CANCEL and
;; reap every unfinished child before propagating. This is the opposite of the
;; observational `async/all`/`async/race`/`async/timeout`, which never cancel the
;; promises a caller supplied — the owned forms are built ON those observational
;; combinators (which fail-fast on the FIRST child settlement) plus an explicit
;; cancel sweep.
;;
;; A hard constraint drives the helper shape: `async/spawn`/`async/cancel` issue
;; structural runtime requests at the enclosing task's bytecode boundary. A
;; synchronous higher-order native (`map`) cannot propagate such a request from
;; a directly invoked callback. Every spawn/cancel step therefore runs at
;; bytecode level via explicit recursion, never `(map async/spawn …)`. Pure list
;; operations (length, etc.) remain safe inside higher-order callbacks.

;; __spawn-thunks: spawn each zero-arg thunk into its own task, returning the
;; promises in INPUT order. Bytecode-level recursion preserves the runtime request.
(define (__spawn-thunks thunks)
  (if (null? thunks)
      (list)
      (let ((p (async/spawn (car thunks))))
        (cons p (__spawn-thunks (cdr thunks))))))

;; __spawn-apply: spawn one task per item that computes `(wf item)`, returning
;; the promises in INPUT order. Bytecode-level recursion — the per-item worker
;; closure `(fn () (wf item))` is built HERE (not inside a `map` lambda), since a
;; closure created during a higher-order native's re-entrant nested call would
;; index the wrong function table.
(define (__spawn-apply wf items)
  (if (null? items)
      (list)
      (let ((item (car items)))
        (let ((p (async/spawn (fn () (wf item)))))
          (cons p (__spawn-apply wf (cdr items)))))))

;; __cancel-all: request cancellation of every promise in a list (best-effort
;; reap of owned children on a fail-fast path). Bytecode-level dispatch preserves
;; the runtime request; cancelling a synthetic or settled promise is a harmless no-op.
(define (__cancel-all promises)
  (if (null? promises)
      nil
      (begin (async/cancel (car promises))
             (__cancel-all (cdr promises)))))

;; __owned-all: await every already-spawned child, returning values in INPUT
;; order. Fail-fast + OWNED: `async/all` raises on the FIRST child failure/
;; cancellation (while later children may still be pending); we then CANCEL and
;; reap every child before re-raising, so a still-running sibling is stopped
;; before its side effects — the ownership property the observational `async/all`
;; deliberately lacks. Empty input → empty list. The single engine behind
;; spawn-all / map / pool-map (each differs only in how it produces the children).
(define (__owned-all children)
  (if (null? children)
      (list)
      (try (async/all children)
           (catch e
             (__cancel-all children)
             (throw e)))))

;; __prefill-sem: seed a semaphore channel with `k` availability tokens
;; (bytecode-level channel/send — capacity is exactly k so none block).
(define (__prefill-sem sem k)
  (if (<= k 0)
      nil
      (begin (channel/send sem #t)
             (__prefill-sem sem (- k 1)))))

;; async/pool-map: bounded-concurrency fan-out. Applies `f` to each item with at
;; most `n` tasks running concurrently, returning results in INPUT order.
;;
;;   (async/pool-map fetch urls 8)   ; fetch all urls, <=8 sockets open at once
;;
;; `f`, `items` and `n` are all ordinary values, so this could be a plain
;; function — it's a macro only because the prelude loader registers macros, not
;; top-level defines. The args are spliced verbatim into a `let` (each is bound
;; once, so they evaluate exactly once and in argument order).
;;
;; `n <= 0` is an argument error (a positive concurrency is required).
;;
;; Concurrency is bounded by a semaphore built from a capacity-`n` channel
;; pre-filled with `n` tokens: each spawned task first `(channel/recv sem)`
;; (acquire — suspends when the pool is full, which is what caps concurrency),
;; runs `(pool-f item)`, then releases its token on BOTH the success and error
;; paths (via try/catch — a throwing `f` must still release, or the pool
;; deadlocks). Owned fan-out: `__owned-all` awaits the workers in INPUT order and,
;; on the first failure, CANCELS and reaps every unfinished worker (parked on the
;; semaphore or mid-`f`) before re-raising — unlike the observational `async/all`.
(defmacro async/pool-map (f items n)
  `(let ((pool-f# ,f)
         (pool-items# ,items)
         (pool-n# ,n))
     (if (<= pool-n# 0)
         (error "async/pool-map: concurrency must be a positive integer")
         (let ((pool-sem# (channel/new pool-n#)))
           ;; Pre-fill the semaphore with n tokens (the available concurrency slots).
           (__prefill-sem pool-sem# pool-n#)
           ;; Per-item worker: acquire a slot (parks if the pool is full — this is
           ;; what caps concurrency), run `f`, release on BOTH the success and error
           ;; paths, and re-raise a failure so __owned-all's fail-fast cleanup fires.
           (let ((pool-worker#
                   (fn (item#)
                     (channel/recv pool-sem#)             ; acquire a slot (parks if full)
                     (let ((result# (try {:ok (pool-f# item#)}
                                         (catch e# {:err e#}))))
                       (channel/send pool-sem# #t)         ; release on BOTH paths
                       (if (contains? result# :err)
                         (throw (:err result#))            ; re-raise so failures surface
                         (:ok result#))))))
             (__owned-all (__spawn-apply pool-worker# pool-items#)))))))

;; __fanout-tagged: the single bounded-concurrency fan-out engine shared by `parallel`
;; and `pipeline`. Applies worker `wf` to each item with at most `n` tasks running at
;; once (semaphore = capacity-`n` channel pre-filled with `n` tokens), results in INPUT
;; order. Each result is wrapped `{:ok v}` / `{:err e}` so the caller picks the failure
;; policy — a throwing worker never aborts the batch. Internal (the `#`-suffixed
;; bindings and the `__` name mark it as not-for-direct-use); `parallel`/`pipeline` are
;; the public surface.
;; Children are spawned at bytecode level via `__spawn-apply` (NOT `(map
;; async/spawn …)`): `async/spawn` issues a structural runtime request that a
;; synchronous `map` callback cannot propagate. The per-item worker `fo-worker#`
;; is built HERE at bytecode level (like `async/pool-map`'s `pool-worker#`) and
;; handed to `__spawn-apply`, which wraps and spawns each `(fo-worker# item)`
;; directly.
(defmacro __fanout-tagged (wf items n)
  `(let ((fo-f# ,wf)
         (fo-items# ,items)
         (fo-sem# (channel/new ,n)))
     (for-range (i# 0 ,n) (channel/send fo-sem# #t))   ; n concurrency tokens
     (let ((fo-worker#
             (fn (item#)
               (channel/recv fo-sem#)                   ; acquire (parks when full)
               (let ((r# (try {:ok (fo-f# item#)}
                              (catch e# {:err e#}))))
                 (channel/send fo-sem# #t)              ; release on BOTH paths
                 r#))))                                  ; tagged; caller decides policy
       (async/all (__spawn-apply fo-worker# fo-items#)))))

;; parallel: run a list of zero-arg thunks concurrently (bounded), awaiting them ALL
;; before returning — a BARRIER. Results come back in input order; a thunk that throws
;; yields `nil` in its slot (the batch never aborts), so `(filter (fn (x) (not (nil? x)))
;; results)` drops failures. Mirrors the Claude Code `workflow.js` `parallel`. Optional
;; trailing arg overrides the default concurrency cap (8).
;;
;;   (parallel (list (fn () (http/get a)) (fn () (http/get b))))   ; both at once
(defmacro parallel (thunks . rest)
  (let ((n (if (null? rest) 8 (car rest))))
    `(map (fn (pr#) (if (contains? pr# :err) nil (:ok pr#)))
          (__fanout-tagged (fn (th#) (th#)) ,thunks ,n))))

;; pipeline: each item flows through ALL stage fns independently — NO barrier between
;; stages (every item is its own task, so item A can be in stage 3 while item B is still
;; in stage 1). A stage that throws drops that item to `nil` and skips its remaining
;; stages. Results align to `items` (nils for dropped). Mirrors the `workflow.js`
;; `pipeline`. Each stage fn takes the previous stage's result.
;;
;;   (pipeline files
;;     (fn (f) (step (str "Audit " f) {:schema finding}))
;;     (fn (x) (step (str "Verify " (:claim x)) {:schema verdict})))
(defmacro pipeline (items . stages)
  `(map (fn (pp#) (if (contains? pp# :err) nil (:ok pp#)))
        (__fanout-tagged
          (fn (it#) (foldl (fn (acc# st#) (st# acc#)) it# (list ,@stages)))
          ,items 8)))

;; settled/ok? / settled/err?: predicates for settled results ({:ok v} / {:err e}).
;; Slash-namespaced to avoid clobbering user-defined functions named ok?/err?.
(defmacro settled/ok?  (r) `(contains? ,r :ok))
(defmacro settled/err? (r) `(contains? ,r :err))

;; settled-partition: split a list of settled results into {:ok (...vals) :err (...errs)}.
;; Extracts the inner values — successes in :ok, failure reasons in :err.
(defmacro settled-partition (results)
  `(let ((rs# ,results))
     {:ok  (map (fn (r#) (:ok  r#)) (filter (fn (r#) (contains? r# :ok))  rs#))
      :err (map (fn (r#) (:err r#)) (filter (fn (r#) (contains? r# :err)) rs#))}))

;; parallel-settled: like `parallel`, but each slot is the raw {:ok v}/{:err e} settled
;; result in INPUT order — nothing is collapsed to nil. The author picks the failure
;; policy (retry / fallback / record / drop). Optional trailing arg overrides the default
;; concurrency cap (8), exactly like `parallel`.
;;
;;   (parallel-settled (list (fn () 1) (fn () (throw "boom")) (fn () 3)))
;;   => ({:ok 1} {:err #<error>} {:ok 3})
(defmacro parallel-settled (thunks . rest)
  (let ((n (if (null? rest) 8 (car rest))))
    `(__fanout-tagged (fn (th#) (th#)) ,thunks ,n)))

;; pipeline-settled: like `pipeline`, but a stage that throws yields {:err e} for that
;; item (instead of nil), preserving the error. Items that survive every stage are {:ok final}.
;;
;;   (pipeline-settled (list 0 1 2)
;;     (fn (i) (if (= i 1) (throw "boom") i))
;;     (fn (x) (* x 10)))
;;   => ({:ok 0} {:err #<error "boom">} {:ok 20})
(defmacro pipeline-settled (items . stages)
  `(__fanout-tagged
     (fn (it#) (foldl (fn (acc# st#) (st# acc#)) it# (list ,@stages)))
     ,items 8))

;; with-retry: run a thunk with bounded exponential backoff on failure.
;;   opts: {:max 3            ; total attempts (default 3)
;;          :base-ms 200      ; first backoff delay in ms (default 200)
;;          :factor 2         ; delay multiplier per attempt (default 2)
;;          :on (fn (e n) …)} ; optional per-failure hook (error, attempt index)
;; On throw: sleeps base-ms * factor^(n-1) via (async/sleep …) — cooperative, parks
;; the task so siblings run — then retries up to :max attempts total. Returns the
;; thunk's value on success; re-raises the last error after :max failures so it
;; composes cleanly with __fanout-tagged's catch (a with-retry leaf that exhausts its
;; budget surfaces as {:err e} in a parallel-settled / pipeline-settled slot).
;; NOTE: each retry attempt counts as a separate provider call for budget purposes.
(defmacro with-retry (opts thunk)
  `(let ((wr-opts# ,opts)
         (wr-f#    ,thunk))
     (let ((wr-max#  (or (get wr-opts# :max) 3))
           (wr-base# (or (get wr-opts# :base-ms) 200))
           (wr-fac#  (or (get wr-opts# :factor) 2))
           (wr-on#   (get wr-opts# :on)))
       (letrec ((go# (fn (n# delay#)
                       (try (wr-f#)
                            (catch e#
                              (when wr-on# (wr-on# e# n#))
                              (if (>= n# wr-max#)
                                (throw e#)               ; exhausted → re-raise
                                (begin
                                  (async/sleep delay#)    ; cooperative backoff
                                  (go# (+ n# 1) (* delay# wr-fac#)))))))))
         (go# 1 wr-base#)))))

;; retry: run `thunk` up to :max-attempts times (default 3), backing off
;; :base-delay-ms (default 100) * :backoff (default 2.0) ^ attempt between
;; failures, re-raising the last error once attempts are exhausted.
;;   (retry thunk) or (retry thunk {:max-attempts 3 :base-delay-ms 100 :backoff 2.0})
;; A native cannot retain its Rust loop state across a structural suspension,
;; so in async context the loop lives here and backs off via `async/sleep` while
;; siblings run. `__retry-setup` shares its option parsing and clamping with the
;; blocking native. At top level the blocking native uses real `thread::sleep`.
(define (retry thunk . __retry-rest)
  (if (or (__async-context?) (__runtime-quantum?))
      (let ((__retry-opts (apply __retry-setup thunk __retry-rest)))
        (letrec ((__retry-go (fn (__retry-n __retry-delay)
                    (try (thunk)
                         (catch __retry-e
                           (if (>= __retry-n (:max-attempts __retry-opts))
                               (throw __retry-e)
                               (begin
                                 (when (> __retry-delay 0) (async/sleep __retry-delay))
                                 (__retry-go (+ __retry-n 1)
                                             (int (* __retry-delay (:backoff __retry-opts)))))))))))
          (__retry-go 1 (:base-delay-ms __retry-opts))))
      (apply __retry-blocking thunk __retry-rest)))

;; make-parameter: R7RS parameter object. Returns a variadic procedure closed
;; over a mutable cell (the current value) and an optional converter.
;;   (p)      -> current value
;;   (p v)    -> SRFI-39 mutate: set the value to (converter v), return it
;; The remaining two-arg forms are a private protocol used by `parameterize`
;; to install/restore values without re-applying the converter on restore:
;;   (p '__param-convert v) -> (converter v), no mutation (convert-only)
;;   (p '__param-raw v)     -> set the value to v AS-IS, no conversion (raw set)
;; Any non-'__param-convert first arg of a 2-arg call takes the raw-set path,
;; but callers should use '__param-raw for clarity.
;; (make-parameter 10 (lambda (x) (* x 2))) => a parameter whose value is always
;; doubled on install; the converter runs once at (make-parameter) time too.
(define (make-parameter init . rest)
  (let* ((converter (if (null? rest) (lambda (x) x) (car rest)))
         (value (converter init)))
    (lambda (. args)
      (cond
        ((null? args) value)
        ((null? (cdr args)) (set! value (converter (car args))) value)
        ((eq? (car args) '__param-convert) (converter (cadr args)))
        (else (set! value (cadr args)) value)))))

;; __parameterize: engine behind the `parameterize` macro. Converts every new
;; value BEFORE installing any of them (a throwing converter leaves every
;; parameter untouched — atomic all-or-nothing), then installs, runs thunk,
;; and restores the RAW old values (never re-converted) whether thunk returns
;; normally or raises — mirroring the with-stream/with-retry catch-rethrow-
;; then-restore idiom.
(define (__parameterize params vals thunk)
  (let ((news (map (lambda (p v) (p '__param-convert v)) params vals)))
    (let ((olds (map (lambda (p) (p)) params)))
      (map (lambda (p n) (p '__param-raw n)) params news)
      (let ((res (try (thunk)
                   (catch e
                     (map (lambda (p o) (p '__param-raw o)) params olds)
                     (throw e)))))
        (map (lambda (p o) (p '__param-raw o)) params olds)
        res))))

;; parameterize: dynamically rebind parameters for the extent of body,
;; restoring the prior value on exit (including on a raised condition).
;; (parameterize ((p v) ...) body ...)
(defmacro parameterize (bindings . body)
  `(__parameterize
     (list ,@(map car bindings))
     (list ,@(map cadr bindings))
     (fn () ,@body)))

;; async/spawn-all: spawn a list of zero-arg thunks as concurrent tasks and await
;; them all, returning results in INPUT order. Unbounded — every thunk gets its own
;; task at once; use `async/pool-map` to cap how many run concurrently. OWNED: the
;; first child failure/cancellation CANCELS and reaps the still-running siblings
;; before propagating (fail-fast), unlike the observational `async/all`. Empty
;; input → empty list.
;;
;;   (async/spawn-all (list (fn () (http/get a)) (fn () (http/get b))))  ; both at once
(defmacro async/spawn-all (thunks)
  `(__owned-all (__spawn-thunks ,thunks)))

;; async/map: concurrent map — apply `f` to each item in its OWN task, results in
;; INPUT order. The unbounded sibling of `async/pool-map` (no concurrency cap) and
;; OWNED: same fail-fast cancel-and-reap of the outstanding children as spawn-all.
;;
;;   (async/map fetch urls)            ; fetch every url concurrently
;;   (async/map (fn (q) (llm/complete q)) prompts)
(defmacro async/map (f items)
  `(__owned-all (__spawn-apply ,f ,items)))

;; async/race-owned: run a list of zero-arg thunks concurrently and settle on the
;; FIRST to finish, returning its value (or re-raising its error). OWNED: every
;; losing child is CANCELLED and reaped before returning — the fail-fast dual of
;; the observational `async/race` (which leaves losers running). Requires ≥1 thunk.
;;
;;   (async/race-owned (list (fn () (http/get mirror-a)) (fn () (http/get mirror-b))))
(define (async/race-owned thunks)
  (if (null? thunks)
      (error "async/race-owned: requires at least one thunk")
      (let ((children (__spawn-thunks thunks)))
        ;; `async/race` settles on the FIRST child (value or error); either way we
        ;; then CANCEL and reap every child, so losers are stopped before their
        ;; side effects (the winner cancel is a no-op — it already settled).
        (try (let ((winner (async/race children)))
               (__cancel-all children)
               winner)
             (catch e
               (__cancel-all children)
               (throw e))))))

;; async/with-timeout: run one owned child (`thunk`) with a deadline of `ms`
;; milliseconds. If the child settles first, its outcome is preserved (value
;; returned, error re-raised). If the deadline wins, the child is CANCELLED and
;; reaped, then a structured `{:type :timeout}` condition is raised.
;;
;;   (async/with-timeout 30000 (fn () (llm/complete prompt)))
(define (async/with-timeout ms thunk)
  (let ((child (async/spawn thunk))
        ;; A deadline task resolving to a distinct sentinel; racing it against the
        ;; child tells the two apart without relying on `async/timeout` (which
        ;; can't distinguish a timeout from a child rejection from its catch).
        (timer (async/spawn (fn () (async/sleep ms) :__with-timeout-elapsed))))
    (let ((outcome (try {:v (async/race (list child timer))}
                        (catch e {:e e}))))       ; child errored before the deadline
      ;; Owned cleanup: cancel BOTH — the child on a timeout, the timer on settle.
      (async/cancel child)
      (async/cancel timer)
      (cond
        ((contains? outcome :e) (throw (:e outcome)))          ; preserve child error
        ((eq? (:v outcome) :__with-timeout-elapsed)
         (throw {:type :timeout :message (str "operation exceeded " ms " ms")}))
        (else (:v outcome))))))                                 ; child value

;; ── Non-blocking multi-round agent loop (issue #61 §3a, ADR #68) ──────────────
;; In a runtime task, drive the agent conversation from bytecode: each provider
;; round is one native (`__agent-step`) that suspends on an external wait, so
;; sibling tasks overlap and `async/timeout`/`async/cancel` can cut the loop at
;; an inter-round park. A round that produced tool calls ALWAYS
;; executes them first (so the final round at the turn cap still runs its tools and
;; leaves a valid `assistant(tool_calls) → tool_result` history, matching the blocking
;; path — never a dangling tool-call turn), then finishes if `:done` or recurses. Loop
;; bounds (max-turns, consecutive-error abort) are enforced in the Rust handle. The
;; synchronous path uses `__agent-run-blocking`.
(define (__agent-drive __h)
  (let ((__r0 (__agent-step __h)))
    ;; A streaming (:on-text) round hands back {:stream tok :on-text cb} instead
    ;; of running inline: drive the deltas in TASK context (the callback may
    ;; itself suspend; siblings interleave between delta batches), then apply the
    ;; assembled response to the loop state (usage was accounted by the stream
    ;; finalizer) to get the ordinary {:done :has-tools} map.
    (let ((__r (if (nil? (:stream __r0))
                   __r0
                   (begin (__stream-drive (:stream __r0) (:on-text __r0))
                          (__agent-stream-apply __h (:stream __r0))))))
      (if (:has-tools __r)
          (begin (__agent-exec-tools __h)
                 (if (:done __r) (__agent-finish __h) (__agent-drive __h)))
          (__agent-finish __h)))))

(define (agent/run __agent __input . __rest)
  (if (or (__async-context?) (__runtime-quantum?))
      (let ((__h (apply __agent-begin __agent __input __rest)))
        ;; Pass the unwinding error to finish so the agent span is closed carrying
        ;; the failure status (notably a cancellation, whose bytecode now runs this
        ;; catch), not ended "unset".
        (try (__agent-drive __h)
             (catch __e (begin (__agent-finish __h __e) (throw __e)))))
      (apply __agent-run-blocking __agent __input __rest)))

;; llm/chat: a thin dispatcher like `agent/run` above. A native cannot retain its
;; Rust loop across multiple provider-round suspensions, so runtime execution
;; reuses the same bytecode driver:
;; `__chat-begin` builds an ordinary agent-loop handle straight from the raw
;; messages + opts (llm/chat has no defagent/:session/:memory to unpack) and
;; `__agent-drive` runs it — tool rounds interleave with sibling tasks exactly like
;; an agent/run loop does. `__chat-begin` returns nil when the call has no
;; `:tools` (or `:tool-mode :none`) to loop over; that case, and the whole
;; sync/top-level path, falls through to `__llm-chat-blocking` — which already
;; offloads its own plain-completion case in async context (WP-LLM-SIMPLE), so
;; nothing agent-loop-shaped (span, conversation scope, slab entry) is created for
;; it. `__chat-begin` forces the loop state's `has_opts` false, so `__agent-finish`
;; returns llm/chat's bare completion-string contract, never the agent `{:response
;; ...}` envelope — the Sema-visible signature/return shape/error behavior of
;; `llm/chat` is unchanged either way.
;;
;; `__llm-chat-blocking` is dispatched through `__chat-call-blocking`, NOT `apply`:
;; `apply` invokes a native through its synchronous value ABI, which cannot
;; propagate the structural external wait used by the no-`:tools` branch in a
;; runtime task. A direct bytecode call selects the runtime ABI. `__chat-begin`
;; never suspends, so applying it above is safe regardless of argument count.
(define (llm/chat . __chat-args)
  (if (or (__async-context?) (__runtime-quantum?))
      (let ((__h (apply __chat-begin __chat-args)))
        (if (nil? __h)
            (__chat-call-blocking __chat-args)
            (try (__agent-drive __h)
                 (catch __e (begin (__agent-finish __h __e) (throw __e))))))
      (__chat-call-blocking __chat-args)))

;; Direct-call dispatch for `__llm-chat-blocking`'s 1-or-2-arg contract (see the
;; `apply` note above). A malformed call (0 or 3+ args) falls through to `apply`
;; instead — safe there, since the native's own arity check rejects it before
;; reaching an operation that could suspend.
(define (__chat-call-blocking __args)
  (cond
    ((null? __args) (apply __llm-chat-blocking __args))
    ((null? (cdr __args)) (__llm-chat-blocking (car __args)))
    ((null? (cddr __args)) (__llm-chat-blocking (car __args) (cadr __args)))
    (else (apply __llm-chat-blocking __args))))

;; llm/pmap: runtime tasks map the prompt builder sequentially, stringifying each
;; returned value before the next mapper call, then submit the complete prompt list
;; through llm/batch. Both calls use their structural runtime ABI, so a suspending
;; mapper parks the active task and cancellation stops before batch dispatch. Host
;; callback entry uses the guarded blocking compatibility native because it must
;; pass the caller's explicit EvalContext through every mapper call.
(define (__llm-pmap-map-prompts __mapper __items)
  (map (fn (__item) (str (__mapper __item))) __items))

(define (llm/pmap . __pmap-args)
  (if (__runtime-quantum?)
      (cond
        ((= (length __pmap-args) 2)
         (llm/batch
           (__llm-pmap-map-prompts
             (car __pmap-args)
             (__llm-pmap-validate-items (cadr __pmap-args)))))
        ((= (length __pmap-args) 3)
         (llm/batch
           (__llm-pmap-map-prompts
             (car __pmap-args)
             (__llm-pmap-validate-items (cadr __pmap-args)))
           (caddr __pmap-args)))
        (else (apply __llm-pmap-arity-error __pmap-args)))
      (cond
        ((= (length __pmap-args) 2)
         (__llm-pmap-blocking (car __pmap-args) (cadr __pmap-args)))
        ((= (length __pmap-args) 3)
         (__llm-pmap-blocking
           (car __pmap-args) (cadr __pmap-args) (caddr __pmap-args)))
        (else (apply __llm-pmap-arity-error __pmap-args)))))

;; ── Non-blocking streaming (llm/stream + agent :on-text rounds, ADR #68) ──────
;; Same pivotal constraint as the agent loop: a native cannot retain a Rust loop
;; across suspension, so the per-delta loop lives in bytecode. The wire side
;; streams on the I/O pool into a channel; `__stream-next` suspends on an external
;; wait and resolves each batch of deltas as
;; {:deltas [...] :done bool}; this driver calls the callback per delta IN TASK
;; CONTEXT — a callback that itself suspends (async/sleep, channel ops, await) is
;; legal, and sibling tasks run between batches.
(define (__stream-drive __tok __cb)
  (let ((__r (__stream-next __tok)))
    (begin
      (for-each __cb (:deltas __r))
      (if (:done __r) nil (__stream-drive __tok __cb)))))

;; llm/stream: inside any runtime quantum (the root included), route through the
;; non-blocking stream machinery so siblings interleave between deltas. Dispatch
;; valid begin arities directly: generic apply uses the host callback compatibility
;; context, outside the caller's task-local cassette/context/id scopes. With no
;; callback the deltas print to stdout, trailing newline included — matching the
;; blocking native's default display.
(define (llm/stream . __args)
  (if (and (or (__async-context?) (__runtime-quantum?)) (not (null? __args)))
      (let ((__cbs (filter procedure? (cdr __args)))
            (__tok (cond
                     ((= (length __args) 1)
                      (__stream-begin (car __args)))
                     ((= (length __args) 2)
                      (__stream-begin (car __args) (cadr __args)))
                     ((= (length __args) 3)
                      (__stream-begin (car __args) (cadr __args) (caddr __args)))
                     (else (apply __stream-begin __args)))))
        (let ((__cb (if (null? __cbs) (fn (__c) (display __c)) (car __cbs))))
          (__stream-drive __tok __cb)
          (let ((__out (__stream-finish __tok)))
            (if (null? __cbs) (begin (newline) __out) __out))))
      (apply __llm-stream-blocking __args)))

;; SRV-1: `http/serve`'s concurrent accept loop (`sema-stdlib/src/server.rs`,
;; the native registered as `__http-serve-run` below) needs one task per
;; connection, spawned via `async/spawn`'s runtime ABI, which requires a
;; compiled VM closure — a hand-built native fn is rejected (see
;; docs/deferred.md "SRV-1"). It also needs `async/spawn` itself called from
;; ORDINARY compiled bytecode, not re-issued as a bare `RuntimeRequest::Spawn`
;; from a Rust continuation: `spawn_via_registry`
;; (`sema-vm/src/runtime/state.rs`) has a `ReturnOwner::VmResume` fast path
;; that — correctly, for `async/spawn`'s own trivial default continuation —
;; injects the settled promise straight onto the parked VM's stack, but that
;; same fast path SILENTLY DISCARDS any other caller-supplied
;; `NativeContinuation` without ever invoking it. Every native-outcome hop
;; chained off a plain top-level call (Suspend/Call/Runtime resumed from a
;; continuation, never handed back to real bytecode in between) keeps
;; `owner == VmResume` the whole way, so a Rust continuation built to run
;; AFTER a raw `RuntimeRequest::Spawn` — e.g. one that re-arms the next accept
;; wait — is silently dropped, and the spawn's promise value pops out as if it
;; had settled the ENTIRE top-level call.
;;
;; `http/serve` is therefore defined HERE, as a thin Sema wrapper around the
;; native `__http-serve-run`, so the per-connection dispatch factory — the
;; closure that does the mint-and-spawn — is built fresh on EVERY call and
;; passed as a PLAIN ARGUMENT, exactly like `handler` itself. The first
;; version of this tried to compile the factory once and cache it (an
;; `EvalContext` seam, later a thread-local): both leak, because either store
;; outlives the specific `Interpreter`/global-env that compiled it — a
;; thread-local in particular is process-lifetime, so it pins that env's
;; `Rc` forever. `gc_stress_test`'s `zero_upvalue_env_cycle_collected_via_env_
;; candidate` and its cross-env sibling caught this the first time it was
;; tried: after the offending `Interpreter` dropped, its global env's
;; `bindings` `Rc` was still reachable through the stale cached closure.
;; Building the factory fresh per call has no such lifetime mismatch — its
;; only reference is the call's own argument, dropped normally with everything
;; else once the connection's task settles.
;; NOTE: calls `__http-serve-run` DIRECTLY (a fixed-arity call per branch),
;; deliberately NOT via `apply`. `apply`'s cooperative routing (`list.rs`)
;; only sends a callee through the `NativeOutcome::Call` path when it is
;; closure or a known runtime-only native; every OTHER native — including a
;; plain dual-ABI one like `__http-serve-run` — takes `apply`'s synchronous
;; `call_function` fallback unconditionally, on the assumption that a
;; dual-ABI native's plain value callback is a complete, equivalent implementation.
;; That assumption does not hold here: `__http-serve-run`'s plain value callback
;; is a serial `blocking_recv` loop for synchronous, non-quantum callers. Routing
;; through `apply` would select that path instead of the concurrent accept loop,
;; and any `async/spawn` inside a handler would fail with "requires runtime
;; invocation". A direct call
;; goes through the VM's normal native-dispatch (`dispatch_native`), which
;; correctly honors the runtime ABI.
(defn http/serve (handler . opts)
  (let ((factory (fn (h req responder) (async/spawn (fn () (responder (h req)))))))
    (if (null? opts)
        (__http-serve-run handler factory)
        (__http-serve-run handler factory (car opts)))))
"#;
