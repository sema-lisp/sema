use sema_eval::Interpreter;
use sema_mcp::run_mcp_server_on;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[tokio::test]
async fn test_mcp_initialize_and_tools_list() {
    let (client_read, mut server_write) = tokio::io::duplex(1024);
    let (mut server_read, client_write) = tokio::io::duplex(1024);

    let interpreter = Interpreter::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let server_task = tokio::task::spawn_local(async move {
                run_mcp_server_on(client_read, client_write, interpreter, None, None).await
            });

            // 1. Send initialize request
            let init_req = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "test-client",
                        "version": "1.0.0"
                    }
                }
            });
            let req_str = format!("{}\n", init_req);
            server_write.write_all(req_str.as_bytes()).await.unwrap();
            server_write.flush().await.unwrap();

            // Read response
            let mut reader = tokio::io::BufReader::new(&mut server_read);
            let mut resp_line = String::new();
            reader.read_line(&mut resp_line).await.unwrap();
            let init_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();

            assert_eq!(init_resp["jsonrpc"], "2.0");
            assert_eq!(init_resp["id"], 1);
            assert!(init_resp["result"].is_object());
            assert_eq!(init_resp["result"]["serverInfo"]["name"], "sema-mcp");

            // 2. Send tools/list request
            let list_req = json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            });
            let req_str = format!("{}\n", list_req);
            server_write.write_all(req_str.as_bytes()).await.unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let list_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();

            assert_eq!(list_resp["jsonrpc"], "2.0");
            assert_eq!(list_resp["id"], 2);
            let tools = list_resp["result"]["tools"].as_array().unwrap();

            // Verify standard tools are present
            let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
            assert!(tool_names.contains(&"run_file"));
            assert!(tool_names.contains(&"compile"));
            assert!(tool_names.contains(&"eval"));
            assert!(tool_names.contains(&"docs"));
            assert!(tool_names.contains(&"fmt"));
            assert!(tool_names.contains(&"disasm"));
            assert!(tool_names.contains(&"build"));
            assert!(tool_names.contains(&"info"));
            assert!(tool_names.contains(&"notebook/new"));

            // 3. Send tools/call for eval
            let eval_req = json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "eval",
                    "arguments": {
                        "code": "(+ 100 200)"
                    }
                }
            });
            let req_str = format!("{}\n", eval_req);
            server_write.write_all(req_str.as_bytes()).await.unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let eval_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
            assert_eq!(eval_resp["id"], 3);
            let content = eval_resp["result"]["content"].as_array().unwrap();
            let text = content[0]["text"].as_str().unwrap();
            assert!(
                text.contains("Result: 300"),
                "eval response did not contain 'Result: 300', got: {}",
                text
            );

            // Drop connection to stop the server
            drop(server_write);
            server_task.await.unwrap().unwrap();
        })
        .await;
}

#[tokio::test]
async fn test_mcp_notebook_state() {
    let (client_read, mut server_write) = tokio::io::duplex(4096);
    let (mut server_read, client_write) = tokio::io::duplex(4096);

    let interpreter = Interpreter::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let server_task = tokio::task::spawn_local(async move {
                run_mcp_server_on(client_read, client_write, interpreter, None, None).await
            });

            let mut reader = tokio::io::BufReader::new(&mut server_read);
            let mut resp_line = String::new();

            // Create a temporary notebook file
            let tmp_dir = std::env::temp_dir();
            let nb_path = tmp_dir.join(format!("test_nb_{}.sema-nb", uuid::Uuid::new_v4()));
            let nb_path_str = nb_path.to_str().unwrap();

            // 1. Call notebook/new
            let new_req = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "notebook/new",
                    "arguments": {
                        "path": nb_path_str,
                        "title": "Integration Test Notebook"
                    }
                }
            });
            server_write
                .write_all(format!("{}\n", new_req).as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let new_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
            assert!(
                !new_resp["result"]["isError"].as_bool().unwrap(),
                "notebook/new failed: {:?}",
                new_resp
            );

            // 2. Add code cell 1: (def x 123)
            let add1_req = json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "notebook/add_cell",
                    "arguments": {
                        "path": nb_path_str,
                        "type": "code",
                        "source": "(def x 123)"
                    }
                }
            });
            server_write
                .write_all(format!("{}\n", add1_req).as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let add1_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
            assert!(!add1_resp["result"]["isError"].as_bool().unwrap());

            // 3. Read notebook to get cell ID
            let read_req = json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "notebook/read",
                    "arguments": {
                        "path": nb_path_str
                    }
                }
            });
            server_write
                .write_all(format!("{}\n", read_req).as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let read_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
            let read_text = read_resp["result"]["content"][0]["text"].as_str().unwrap();
            let notebook_json: serde_json::Value = serde_json::from_str(read_text).unwrap();
            let cells = notebook_json["cells"].as_array().unwrap();
            assert_eq!(cells.len(), 1);
            let cell1_id = cells[0]["id"].as_str().unwrap();

            // 4. Eval cell 1
            let eval1_req = json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "notebook/eval_cell",
                    "arguments": {
                        "path": nb_path_str,
                        "id": cell1_id
                    }
                }
            });
            server_write
                .write_all(format!("{}\n", eval1_req).as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let eval1_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
            assert!(!eval1_resp["result"]["isError"].as_bool().unwrap());

            // 5. Add cell 2: (+ x 111)
            let add2_req = json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "notebook/add_cell",
                    "arguments": {
                        "path": nb_path_str,
                        "type": "code",
                        "source": "(+ x 111)"
                    }
                }
            });
            server_write
                .write_all(format!("{}\n", add2_req).as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let add2_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
            assert!(!add2_resp["result"]["isError"].as_bool().unwrap());

            // Read notebook again to get cell 2 ID
            server_write
                .write_all(format!("{}\n", read_req).as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let read_resp2: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
            let read_text2 = read_resp2["result"]["content"][0]["text"].as_str().unwrap();
            let notebook_json2: serde_json::Value = serde_json::from_str(read_text2).unwrap();
            let cells2 = notebook_json2["cells"].as_array().unwrap();
            assert_eq!(cells2.len(), 2);
            let cell2_id = cells2[1]["id"].as_str().unwrap();

            // 6. Eval cell 2 (should find variable x = 123)
            let eval2_req = json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "notebook/eval_cell",
                    "arguments": {
                        "path": nb_path_str,
                        "id": cell2_id
                    }
                }
            });
            server_write
                .write_all(format!("{}\n", eval2_req).as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            resp_line.clear();
            reader.read_line(&mut resp_line).await.unwrap();
            let eval2_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
            assert!(!eval2_resp["result"]["isError"].as_bool().unwrap());
            let eval2_text = eval2_resp["result"]["content"][0]["text"].as_str().unwrap();
            assert!(
                eval2_text.contains("234"),
                "Expected cell evaluation to return '234' (123+111), got: {}",
                eval2_text
            );

            // Clean up temporary notebook file
            std::fs::remove_file(nb_path).ok();

            // Stop server
            drop(server_write);
            server_task.await.unwrap().unwrap();
        })
        .await;
}
