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
"#;
