# Examples

Complete Sema Web examples using the current `state` / SIP / component APIs.

## Counter

```scheme
(def count (state 0))

(define (increment ev)
  (update! count (fn (n) (+ n 1))))

(define (decrement ev)
  (update! count (fn (n) (- n 1))))

(define (reset-count ev)
  (put! count 0))

(defcomponent app ()
  [:div {:class "counter"}
    [:h1 "Counter: " @count]
    [:div {:class "buttons"}
      [:button {:on-click "decrement"} "-"]
      [:button {:on-click "reset-count"} "Reset"]
      [:button {:on-click "increment"} "+"]]])

(mount! "#app" "app")
```

```html
<!DOCTYPE html>
<html>
<body>
  <div id="app"></div>
  <script type="text/sema" src="/counter.sema"></script>
  <script type="module">
    import { SemaWeb } from "@sema-lang/sema-web";
    await SemaWeb.init();
  </script>
</body>
</html>
```

## Todo List

```scheme
(def todos (state '()))
(def input-text (state ""))
(def next-id (state 1))

(define (set-input ev)
  (put! input-text (dom/event-value ev)))

(define (maybe-add-todo ev)
  (when (string=? (dom/event-key ev) "Enter")
    (add-todo ev)))

(define (add-todo ev)
  (let ((text @input-text))
    (when (not (string=? text ""))
      (let ((id @next-id))
        (update! todos (fn (items)
          (append items (list {:id id :text text :done false}))))
        (update! next-id (fn (n) (+ n 1)))
        (put! input-text "")))))

(defcomponent app ()
  [:div {:class "todo-app"}
    [:h1 "Todos"]
    [:div {:class "input-row"}
      [:input {:value @input-text
               :placeholder "What needs to be done?"
               :on-input "set-input"
               :on-keydown "maybe-add-todo"}]
      [:button {:on-click "add-todo"} "Add"]]
    [:ul
      (map (fn (todo)
        [:li (:text todo)])
        @todos)]])

(mount! "#app" "app")
```

## Streaming Chat

Requires a deployed [LLM proxy](./llm-proxy).

```scheme
(def messages (state '()))
(def input-text (state ""))
(def current-stream (state nil))

(define (set-input ev)
  (put! input-text (dom/event-value ev)))

(define (maybe-send ev)
  (when (string=? (dom/event-key ev) "Enter")
    (send-message ev)))

(define (send-message ev)
  (let ((text @input-text))
    (when (not (string=? text ""))
      (let ((next-messages (append @messages (list {:role "user" :content text}))))
        (put! messages next-messages)
        (put! input-text "")
        (put! current-stream
          (llm/chat-stream
            (map (fn (msg)
              (message (string->keyword (:role msg)) (:content msg)))
              next-messages)
            {:model "gpt-4o"}))))))

(defcomponent app ()
  [:div {:class "chat"}
    [:div {:class "messages"}
      (map (fn (msg)
        [:div {:class (string-append "message " (:role msg))}
          (:content msg)])
        @messages)
      (when @current-stream
        (let ((stream-state (deref @current-stream)))
          [:div {:class "message assistant"}
            (:text stream-state)]))]
    [:div {:class "input-bar"}
      [:input {:value @input-text
               :placeholder "Ask anything..."
               :on-input "set-input"
               :on-keydown "maybe-send"}]
      [:button {:on-click "send-message"} "Send"]]])

(mount! "#app" "app")
```

## Notes

- Use named handler strings in SIP attributes like `{:on-click "save"}`.
- Lower-level APIs like `dom/on!`, `watch`, and `on-mount` can accept function values directly.
- For production deploys, prefer compiled `.vfs` archives. See [Deployment](./deployment).
- For a fuller production-style example, see `examples/sema-web-app/` in the repository and [Building a Sema Web App](./building-apps).
