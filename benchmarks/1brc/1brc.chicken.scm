;;; 1 Billion Row Challenge — Chicken Scheme implementation
;;; Usage: csi -s 1brc.chicken.scm /path/to/measurements.txt

(import (chicken io)
        (chicken process-context)
        (scheme time)
        (chicken string)
        (chicken format)
        (chicken sort)
        (srfi-69))

(define (find-semicolon line)
  (let loop ((i 0))
    (cond
      ((= i (string-length line)) #f)
      ((char=? (string-ref line i) #\;) i)
      (else (loop (+ i 1))))))

(define (parse-temp-int10 str start)
  (let ((len (string-length str)))
    (if (char=? (string-ref str start) #\-)
        (- (parse-temp-int10 str (+ start 1)))
        (let loop ((i start) (acc 0))
          (if (>= i len)
              acc
              (let ((c (string-ref str i)))
                (if (char=? c #\.)
                    (loop (+ i 1) acc)
                    (loop (+ i 1) (+ (* acc 10) (- (char->integer c) 48))))))))))

(define (format-1dp n)
  (let* ((abs-n (abs n))
         (whole (quotient abs-n 10))
         (frac (remainder abs-n 10)))
    (cond
      ((and (< n 0) (= whole 0))
       (sprintf "-0.~A" frac))
      ((< n 0)
       (sprintf "-~A.~A" whole frac))
      (else
       (sprintf "~A.~A" whole frac)))))

(define (main)
  (let ((args (command-line-arguments)))
    (when (null? args)
      (fprintf (current-error-port) "Usage: csi -s 1brc.chicken.scm <file>~%")
      (exit 1))

    (let* ((file-path (car args))
           (table (make-hash-table string=? string-hash))
           (start-time (current-jiffy))
           (port (open-input-file file-path)))

      (let loop ((line (read-line port)))
        (unless (eof-object? line)
          (let* ((semi (find-semicolon line))
                 (name (substring line 0 semi))
                 (temp (parse-temp-int10 line (+ semi 1)))
                 (entry (hash-table-ref/default table name #f)))
            (if entry
                (begin
                  (vector-set! entry 0 (min (vector-ref entry 0) temp))
                  (vector-set! entry 1 (+ (vector-ref entry 1) temp))
                  (vector-set! entry 2 (max (vector-ref entry 2) temp))
                  (vector-set! entry 3 (+ (vector-ref entry 3) 1)))
                (hash-table-set! table name (vector temp temp temp 1))))
          (loop (read-line port))))

      (close-input-port port)

      (let* ((names (sort (hash-table-keys table) string<?))
             (parts (map (lambda (name)
                           (let* ((entry (hash-table-ref table name))
                                  (mn (vector-ref entry 0))
                                  (sum (vector-ref entry 1))
                                  (mx (vector-ref entry 2))
                                  (cnt (vector-ref entry 3))
                                  (mean (inexact->exact (round (/ (* sum 1.0) cnt)))))
                             (string-append name "="
                                            (format-1dp mn) "/"
                                            (format-1dp mean) "/"
                                            (format-1dp mx))))
                         names))
             (end-time (current-jiffy))
             (elapsed (quotient (* (- end-time start-time) 1000)
                                (jiffies-per-second))))

        (print "{" (string-intersperse parts ", ") "}")
        (fprintf (current-error-port) "Elapsed: ~A ms~%" elapsed)))))

(main)
