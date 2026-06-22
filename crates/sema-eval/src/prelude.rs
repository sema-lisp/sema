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
"#;
