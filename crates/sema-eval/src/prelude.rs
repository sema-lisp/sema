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
;; to launch further `agent` leaves; the run ends {:status :failed :reason "budget
;; exceeded"}. Under a concurrent fan-out the cap still trips, but per-agent token
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
;; opens "Audit"; every `agent`/`checkpoint` that follows attributes to it until the
;; next `(phase …)` or the run end (which closes the last open phase). Returns nil.
(defmacro phase (label)
  `(workflow/phase ,label))

;; agent: a journaled LLM leaf (workflow.js `agent(prompt, {schema})`). Runs the prompt
;; through the configured provider and returns TYPED DATA when `:schema` is supplied
;; (validated via `llm/extract`), or the completion text otherwise. The optional opts
;; map carries `:name` (the agent role label shown in the dashboard, default "agent")
;; and `:schema`. The call is wrapped by `workflow/agent`, which emits
;; agent.started/agent.result + a per-agent budget event.
;;
;;   (agent "List the auth-relevant files under src/." {:name "scout" :schema [:list :string]})
;;   (agent "Summarize the changelog.")                  ; no schema -> returns text
;; When `:tools [...]` is present the leaf runs the REAL tool loop (via llm/chat,
;; which owns run_tool_loop) and journals each genuine tool call as an agent.tool_call
;; event through the `:on-tool-call` callback. v1 returns the loop's final TEXT —
;; `:schema` does NOT compose with `:tools` yet (deferred). Per-agent budget for a
;; multi-round tool agent is best-effort (the Budget event reflects the final round's
;; usage; LAST_USAGE is a single slot — same caveat as fan-out accounting).
(defmacro agent (prompt . rest)
  (let ((opts-form (if (null? rest) {} (car rest))))
    `(let ((ag-opts# ,opts-form)
           (ag-prompt# ,prompt))
       ;; Inject the resolved prompt + a stable schema repr so workflow/agent can
       ;; compute this leaf's resume content-key (these `:__`-prefixed keys are
       ;; internal — read by the runtime, ignored by the dashboard).
       (workflow/agent (assoc ag-opts#
                         :__prompt (str ag-prompt#)
                         :__schema-repr (str (get ag-opts# :schema)))
         (fn ()
           (let ((ag-schema# (get ag-opts# :schema))
                 (ag-tools# (get ag-opts# :tools)))
             (if (not (nil? ag-tools#))
               ;; tools branch: real run_tool_loop, one agent.tool_call per call.
               (llm/chat (prompt (user ag-prompt#))
                 {:tools ag-tools#
                  :on-tool-call (fn (ev#)
                                  (when (= (:event ev#) "start")
                                    (workflow/tool-call (:tool ev#) (:args ev#))))})
               (if (nil? ag-schema#)
                 (llm/complete ag-prompt#)
                 (llm/extract ag-schema# ag-prompt#)))))))))

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
;;     (fn (f) (agent (str "Audit " f) {:schema finding}))
;;     (fn (x) (agent (str "Verify " (:claim x)) {:schema verdict})))
(defmacro pipeline (items . stages)
  `(map (fn (pp#) (if (contains? pp# :err) nil (:ok pp#)))
        (__fanout-tagged
          (fn (it#) (foldl (fn (acc# st#) (st# acc#)) it# (list ,@stages)))
          ,items 8)))

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
