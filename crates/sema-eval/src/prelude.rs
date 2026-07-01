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

;; Terminal setup/teardown guards. Each runs BODY and ALWAYS restores on exit —
;; even if BODY throws — so a crash can't leave the terminal broken. They mirror
;; with-stream's try/catch-rethrow-then-cleanup shape and return BODY's value.
;; Compose them (outermost restores last): typically
;;   (io/with-raw-mode (term/with-alt-screen (term/with-mouse ...body...)))

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
(defmacro defworkflow (name doc meta . body)
  `(workflow/run (symbol->string (quote ,name)) ,doc ,meta (lambda () ,@body)))

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
;; builtins.rs) that branches internally on `in_async_context()`: synchronous
;; inline outside a scheduler task, offloaded+overlapping inside one. Keeping it a
;; native (not a router macro) is what makes `(procedure? llm/embed)` true and lets
;; it be used as a value — `(map llm/embed …)`, `(async/pool-map llm/embed …)`.

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
;; Concurrency is bounded by a semaphore built from a capacity-`n` channel
;; pre-filled with `n` tokens: each spawned task first `(channel/recv sem)`
;; (acquire — yields/parks when the pool is full, which is what caps concurrency),
;; runs `(pool-f item)`, then releases its token on BOTH the success and error
;; paths (via try/catch — a throwing `f` must still release, or the pool
;; deadlocks). Errors are re-raised so a failing item surfaces. `async/all`
;; preserves spawn (i.e. input) order, so results line up with `items`.
(defmacro async/pool-map (f items n)
  `(let ((pool-f# ,f)
         (pool-items# ,items)
         (pool-sem# (channel/new ,n)))
     ;; Pre-fill the semaphore with n tokens (the available concurrency slots).
     (for-range (i# 0 ,n) (channel/send pool-sem# #t))
     (async/all
       (map (fn (item#)
              (async/spawn
                (fn ()
                  (channel/recv pool-sem#)            ; acquire a slot (parks if full)
                  (let ((result# (try {:ok (pool-f# item#)}
                                      (catch e# {:err e#}))))
                    (channel/send pool-sem# #t)        ; release on BOTH paths
                    (if (contains? result# :err)
                      (throw (:err result#))           ; re-raise so failures surface
                      (:ok result#))))))
            pool-items#))))

;; __fanout-tagged: the single bounded-concurrency fan-out engine shared by `parallel`
;; and `pipeline`. Applies worker `wf` to each item with at most `n` tasks running at
;; once (semaphore = capacity-`n` channel pre-filled with `n` tokens), results in INPUT
;; order. Each result is wrapped `{:ok v}` / `{:err e}` so the caller picks the failure
;; policy — a throwing worker never aborts the batch. Internal (the `#`-suffixed
;; bindings and the `__` name mark it as not-for-direct-use); `parallel`/`pipeline` are
;; the public surface.
(defmacro __fanout-tagged (wf items n)
  `(let ((fo-f# ,wf)
         (fo-items# ,items)
         (fo-sem# (channel/new ,n)))
     (for-range (i# 0 ,n) (channel/send fo-sem# #t))   ; n concurrency tokens
     (async/all
       (map (fn (item#)
              (async/spawn
                (fn ()
                  (channel/recv fo-sem#)                 ; acquire (parks when full)
                  (let ((r# (try {:ok (fo-f# item#)}
                                 (catch e# {:err e#}))))
                    (channel/send fo-sem# #t)            ; release on BOTH paths
                    r#))))                                ; tagged; caller decides policy
            fo-items#))))

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

;; async/spawn-all: spawn a list of zero-arg thunks as concurrent tasks and await
;; them all, returning results in INPUT order. The ergonomic form of the very common
;; `(async/all (map (fn (th) (async/spawn th)) thunks))`. Unbounded — every thunk gets
;; its own task at once; use `async/pool-map` to cap how many run concurrently.
;;
;;   (async/spawn-all (list (fn () (http/get a)) (fn () (http/get b))))  ; both at once
(defmacro async/spawn-all (thunks)
  `(async/all (map (fn (thunk#) (async/spawn thunk#)) ,thunks)))

;; async/map: concurrent map — apply `f` to each item in its OWN task, results in
;; INPUT order. The unbounded sibling of `async/pool-map` (no concurrency cap).
;;
;;   (async/map fetch urls)            ; fetch every url concurrently
;;   (async/map (fn (q) (llm/complete q)) prompts)
(defmacro async/map (f items)
  `(let ((amap-f# ,f))
     (async/all
       (map (fn (item#) (async/spawn (fn () (amap-f# item#)))) ,items))))
"#;
