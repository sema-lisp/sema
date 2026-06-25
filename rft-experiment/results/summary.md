# Benchmark Results

Models: glm-5.2-serverless, qwen3-8b-rft

## Overall Scores

| Model | Overall | L1 | L2 | L3 | L4 | L5 |
|-------|:-:|:-:|:-:|:-:|:-:|:-:|
| GLM 5.2 (serverless, no FT) | 49% | 58% | 61% | 65% | 13% | 12% |
| Qwen3-8B (RFT fine-tuned) | 36% | 58% | 37% | 23% | 26% | 26% |

## By Category

| Model | arithmetic | async | cond | data-pipeline | data-transform | destructure | error-handling | f-string | fibonacci | function | higher-order | interpreter | json | json-transform | keyword | lazy | let | let* | list | macro | map | match | math | matrix | memoize | multimethod | parser | pattern | predicate | quasiquote | record | recursion | reduce | short-lambda | state-machine | string | threading | web-server |
|-------|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| GLM 5.2 (serverless, no FT) | 65% | 0% | 30% | 0% | 100% | 0% | 100% | 30% | 100% | 100% | 100% | 0% | 50% | 30% | 30% | 0% | 30% | 30% | 72% | 32% | 72% | 50% | 100% | 0% | 0% | 100% | 0% | 100% | 30% | 30% | 30% | 100% | 100% | 30% | 0% | 43% | 30% | 0% |
| Qwen3-8B (RFT fine-tuned) | 30% | 30% | 30% | 0% | 0% | 0% | 100% | 100% | 100% | 100% | 100% | 0% | 50% | 30% | 100% | 0% | 30% | 30% | 43% | 0% | 12% | 15% | 100% | 100% | 0% | 0% | 0% | 0% | 30% | 100% | 0% | 0% | 100% | 100% | 0% | 33% | 30% | 30% |

## Token Usage

| Model | Total Input | Total Output | Avg Latency |
|-------|---:|---:|---:|
| GLM 5.2 (serverless, no FT) | 78,763 | 45,375 | 12.1s |
| Qwen3-8B (RFT fine-tuned) | 79,172 | 6,764 | 0.9s |

## Detailed Results


### GLM 5.2 (serverless, no FT)

| Task | Level | Category | Score | Detail |
|------|--------|----------|:-:|------|
| L1-001 | L1 | arithmetic | 0.3 | wrong: expected '3', got '(+ 1 2)' |
| L1-002 | L1 | arithmetic | 1.0 | correct |
| L1-003 | L1 | list | 0.3 | wrong: expected '(2 4 6)', got "(map (fn (x) (* x 2)) '(1 2 3))" |
| L1-004 | L1 | list | 1.0 | correct |
| L1-005 | L1 | map | 1.0 | correct |
| L1-006 | L1 | string | 1.0 | correct |
| L1-007 | L1 | string | 1.0 | correct |
| L1-008 | L1 | let | 0.3 | wrong: expected '30', got '(let ((x 10) (y 20)) (+ x y))' |
| L1-009 | L1 | cond | 0.3 | wrong: expected 'b', got "(cond ((= 1 2) 'a) ((= 1 1) 'b) (else 'c))" |
| L1-010 | L1 | list | 1.0 | correct |
| L1-011 | L1 | predicate | 0.3 | wrong: expected 'true', got '#t' |
| L1-012 | L1 | keyword | 0.3 | wrong: expected '42', got ':a' |
| L1-013 | L1 | short-lambda | 0.3 | wrong: expected '(1 4 9)', got "(map #(* % %) '(1 2 3))" |
| L1-014 | L1 | f-string | 0.3 | wrong: expected 'Hello World', got 'f"Hello ${"World"}"' |
| L1-015 | L1 | map | 0.3 | wrong: expected '{:a 11}', got '(map/update {:a 1} :a #(+ % 10))' |
| L2-001 | L2 | function | 1.0 | correct |
| L2-002 | L2 | function | 1.0 | correct |
| L2-003 | L2 | list | — | error: Command '['/Users/helge/code/sema-rft-experiment/target/debug/sema', 'eva |
| L2-004 | L2 | list | 1.0 | correct |
| L2-005 | L2 | string | 0.3 | wrong output: expected 'true', got '#t' |
| L2-006 | L2 | math | 1.0 | correct |
| L2-007 | L2 | fibonacci | 1.0 | correct |
| L2-008 | L2 | map | 0.3 | wrong output: expected '{:bird 1 :cat 1 :dog 1 :the 3}', got '{"bird" 1 "cat" 1  |
| L2-009 | L2 | list | 1.0 | correct |
| L2-010 | L2 | string | — | error: Unbound variable: string/substring |
| L2-011 | L2 | predicate | 0.3 | wrong output: expected 'true', got '#t' |
| L2-012 | L2 | map | 1.0 | correct |
| L2-013 | L2 | threading | 0.3 | wrong: expected '165', got '(->> (range 1 11)\n     (filter odd?)\n     (map #(* |
| L2-014 | L2 | match | 1.0 | correct |
| L2-015 | L2 | destructure | — | error: Eval error: match: no clause matched value: (1 2) |
| L3-001 | L3 | data-transform | 1.0 | correct |
| L3-002 | L3 | string | — | error: Eval error: let: expected a list |
| L3-003 | L3 | record | 0.3 | wrong output: expected '5', got '5.0' |
| L3-004 | L3 | macro | 1.0 | correct |
| L3-005 | L3 | json | 0.5 | runs without error |
| L3-006 | L3 | multimethod | 1.0 | correct |
| L3-007 | L3 | match | — | error: Eval error: match: each clause must be a list or vector |
| L3-008 | L3 | error-handling | 1.0 | correct |
| L3-009 | L3 | threading | 0.3 | wrong: expected 'HELLO, WORLD, FOO, BAR', got '(->> "Hello World Foo Bar"\n      |
| L3-010 | L3 | recursion | 1.0 | correct |
| L3-011 | L3 | map | 1.0 | correct |
| L3-012 | L3 | pattern | 1.0 | correct |
| L3-013 | L3 | higher-order | 1.0 | correct |
| L3-014 | L3 | string | 0.3 | wrong output: expected 'Alice,30,admin', got '"Alice,30,admin"' |
| L3-015 | L3 | let* | 0.3 | wrong output: expected '(3 2)', got '[3.0 2.0]' |
| L4-001 | L4 | data-pipeline | — | error: Type error: expected list or vector, got native-fn in sort |
| L4-002 | L4 | interpreter | — | error: Eval error: match: no clause matched value: (+ (* 2 3) (- 10 4)) |
| L4-003 | L4 | state-machine | — | error: Eval error: match: each clause must be a list or vector |
| L4-004 | L4 | web-server | — | error: Eval error: let: expected a list |
| L4-005 | L4 | macro | — | error: Reader error at 6:11: unexpected character: '@' |
| L4-006 | L4 | reduce | 1.0 | correct |
| L4-007 | L4 | matrix | — | error: Unbound variable: matrix-transpose |
| L4-008 | L4 | parser | — | error: Unbound variable: ... |
| L4-009 | L4 | json-transform | 0.3 | wrong output: expected '{:Alice 30 :Bob 25}', got '{"Alice" 30 "Bob" 25}' |
| L4-010 | L4 | memoize | — | error: Eval error: let: expected a list |
| L5-001 | L5 | macro | — | error: Unbound variable: second |
| L5-002 | L5 | macro | 0.3 | wrong output: expected '5', got 'None' |
| L5-003 | L5 | async | — | error: Eval error: let: expected a list |
| L5-004 | L5 | lazy | — | error: Type error: expected thunk, got list |
| L5-005 | L5 | quasiquote | 0.3 | wrong output: expected '10', got 'None' |

### Qwen3-8B (RFT fine-tuned)

| Task | Level | Category | Score | Detail |
|------|--------|----------|:-:|------|
| L1-001 | L1 | arithmetic | 0.3 | wrong: expected '3', got '(+ 1 2)' |
| L1-002 | L1 | arithmetic | 0.3 | wrong: expected '42', got '(* 6 7)' |
| L1-003 | L1 | list | 1.0 | correct |
| L1-004 | L1 | list | 0.3 | wrong: expected '(1 3 5)', got "'(1 3 5)" |
| L1-005 | L1 | map | 0.3 | wrong: expected '1', got '(get {:a 1 :b 2} :a)' |
| L1-006 | L1 | string | 1.0 | correct |
| L1-007 | L1 | string | 1.0 | correct |
| L1-008 | L1 | let | 0.3 | wrong: expected '30', got 'let' |
| L1-009 | L1 | cond | 0.3 | wrong: expected 'b', got "(cond ((= 1 2) 'a) ((= 1 1) 'b) (else 'c))" |
| L1-010 | L1 | list | 0.3 | wrong: expected '15', got "(foldl + 0 '(1 2 3 4 5))" |
| L1-011 | L1 | predicate | 0.3 | wrong: expected 'true', got '#t' |
| L1-012 | L1 | keyword | 1.0 | correct |
| L1-013 | L1 | short-lambda | 1.0 | correct |
| L1-014 | L1 | f-string | 1.0 | correct |
| L1-015 | L1 | map | 0.3 | wrong: expected '{:a 11}', got '(map/update {:a 1} :a #(+ % 10))' |
| L2-001 | L2 | function | 1.0 | correct |
| L2-002 | L2 | function | 1.0 | correct |
| L2-003 | L2 | list | — | error: Eval error: stack overflow: maximum call depth exceeded |
| L2-004 | L2 | list | 1.0 | correct |
| L2-005 | L2 | string | — | error: Unbound variable: string/equals? |
| L2-006 | L2 | math | 1.0 | correct |
| L2-007 | L2 | fibonacci | 1.0 | correct |
| L2-008 | L2 | map | — | error: Unbound variable: count-words |
| L2-009 | L2 | list | — | error: Unbound variable: -inf.0 |
| L2-010 | L2 | string | — | error: Unbound variable: title-case |
| L2-011 | L2 | predicate | 0.3 | wrong output: expected 'true', got '#t' |
| L2-012 | L2 | map | — | error: Unbound variable: map-keys-only |
| L2-013 | L2 | threading | 0.3 | wrong: expected '165', got '(->> (range 10)' |
| L2-014 | L2 | match | — | error: Eval error: match: no clause matched value: -5 |
| L2-015 | L2 | destructure | — | error: Eval error: define: expected a symbol |
| L3-001 | L3 | data-transform | — | error: Unbound variable: group-by-parity |
| L3-002 | L3 | string | — | error: Unbound variable: word-freq |
| L3-003 | L3 | record | — | error: Eval error: define-record-type: requires at least type name, constructor, |
| L3-004 | L3 | macro | — | error: Arity error: unless expects 3 args, got 2 |
| L3-005 | L3 | json | 0.5 | runs without error |
| L3-006 | L3 | multimethod | — | error: Arity error: defmethod expects 3 args, got 4 |
| L3-007 | L3 | match | 0.3 | wrong output: expected '{:body "ok" :status 200}', got '{:body "not found" :stat |
| L3-008 | L3 | error-handling | 1.0 | correct |
| L3-009 | L3 | threading | 0.3 | wrong: expected 'HELLO, WORLD, FOO, BAR', got '(->> "Hello World Foo Bar"' |
| L3-010 | L3 | recursion | — | error: Arity error: reduce expects 2 args, got 3 |
| L3-011 | L3 | map | — | error: Unbound variable: map/merge |
| L3-012 | L3 | pattern | — | error: Unbound variable: add1 |
| L3-013 | L3 | higher-order | 1.0 | correct |
| L3-014 | L3 | string | — | error: Unbound variable: csv-row |
| L3-015 | L3 | let* | 0.3 | wrong output: expected '(3 2)', got '[-1.0 6.0]' |
| L4-001 | L4 | data-pipeline | — | error: Arity error: name expects 1 args, got 0 |
| L4-002 | L4 | interpreter | — | error: Unbound variable: second |
| L4-003 | L4 | state-machine | — | error: Unbound variable: run-states |
| L4-004 | L4 | web-server | 0.3 | wrong output: expected '{:body "ok" :status 200}', got '<native-fn http/router/d |
| L4-005 | L4 | macro | — | error: Arity error: my-when-let expects 3 args, got 2 |
| L4-006 | L4 | reduce | 1.0 | correct |
| L4-007 | L4 | matrix | 1.0 | correct |
| L4-008 | L4 | parser | — | error: Reader error at 26:43: unknown character name: backslash |
| L4-009 | L4 | json-transform | 0.3 | wrong output: expected '{:Alice 30 :Bob 25}', got '{"Alice" 30 "Bob" 25}' |
| L4-010 | L4 | memoize | — | error: Unbound variable: map/empty |
| L5-001 | L5 | macro | — | error: Arity error: for-list expects 1 args, got 2 |
| L5-002 | L5 | macro | — | error: Arity error: defn-with-logging expects 4 args, got 3 |
| L5-003 | L5 | async | 0.3 | wrong output: expected '42', got '<async-promise rejected: Unbound variable: ch> |
| L5-004 | L5 | lazy | — | error: Type error: expected thunk, got list |
| L5-005 | L5 | quasiquote | 1.0 | correct |