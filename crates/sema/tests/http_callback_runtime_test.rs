//! Cooperative callback coverage for HTTP router and server helpers.

#![cfg(not(target_arch = "wasm32"))]

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn router_handler_suspends_without_blocking_sibling() {
    let result = eval(
        r#"
        (let ((gate (channel/new 1)))
          (let ((router
                  (http/router
                    [[:get "/work"
                      (fn (_request)
                        (channel/recv gate)
                        (http/text "ok"))]])))
            (let ((response
                    (async/spawn
                      (fn ()
                        (router
                          {:method :get :path "/work" :headers {}
                           :query {} :params {} :body "" :remote "test"}))))
                  (sibling
                    (async/spawn
                      (fn ()
                        (channel/send gate "released")
                        "sibling"))))
              (list
                (async/await sibling)
                (:body (async/await response))))))
        "#,
    )
    .expect("router handler runs through the runtime call ABI");

    assert_eq!(
        result,
        Value::list(vec![Value::string("sibling"), Value::string("ok"),])
    );
}

#[test]
fn generated_tool_route_suspends_without_blocking_sibling() {
    let result = eval(
        r#"
        (let ((gate (channel/new 1)))
          (begin
            (deftool echo "echo a value" {:text {:type :string}}
              (fn (request)
                (channel/recv gate)
                (:text request)))
            (let ((router (http/router (route/from-tools [echo]))))
              (let ((response
                      (async/spawn
                        (fn ()
                          (router
                            {:method :post :path "/tools/echo" :headers {}
                             :query {} :params {} :body ""
                             :json {:text "hello"} :remote "test"}))))
                    (sibling
                      (async/spawn
                        (fn ()
                          (channel/send gate "released")
                          "sibling"))))
                (list
                  (async/await sibling)
                  (:body (async/await response)))))))
        "#,
    )
    .expect("generated tool handler runs through the runtime call ABI");

    assert_eq!(
        result,
        Value::list(vec![Value::string("sibling"), Value::string("\"hello\""),])
    );
}
