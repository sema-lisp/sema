use crate::print_cli_warning;
use crate::NotebookCommands;
use crate::{die, print_cli_error};

pub(crate) fn run_notebook_command(command: NotebookCommands) {
    match command {
        NotebookCommands::Serve { file, host, port } => {
            let path = file.map(std::path::PathBuf::from);
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(sema_notebook::serve(path, &host, port));
        }
        NotebookCommands::Run { file, cells } => {
            let path = std::path::Path::new(&file);
            let mut engine = match sema_notebook::Engine::from_file(path) {
                Ok(e) => e,
                Err(e) => {
                    die(e);
                }
            };

            // Collect the code cell IDs to evaluate, either specific indices
            // (--cells 1,3,5) or all code cells.
            let cell_ids: Vec<String> = if let Some(cell_spec) = &cells {
                let indices: Vec<usize> = cell_spec
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                engine
                    .notebook
                    .cells
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        if indices.contains(&(i + 1))
                            && c.cell_type == sema_notebook::format::CellType::Code
                        {
                            Some(c.id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                engine
                    .notebook
                    .cells
                    .iter()
                    .filter(|c| c.cell_type == sema_notebook::format::CellType::Code)
                    .map(|c| c.id.clone())
                    .collect()
            };

            let total = cell_ids.len();
            let mut had_error = false;

            for (i, id) in cell_ids.into_iter().enumerate() {
                match engine.eval_cell(&id) {
                    Ok(r) => {
                        if !r.stdout.is_empty() {
                            print!("[{}/{}] (stdout) {}", i + 1, total, r.stdout);
                        }
                        if !r.output.display.is_empty() {
                            println!("[{}/{}] {}", i + 1, total, r.output.display);
                        }
                        if let Some(u) = &r.output.usage {
                            let cost = r
                                .output
                                .cost_usd
                                .map(|c| format!("${c:.4}"))
                                .unwrap_or_else(|| "unpriced".to_string());
                            println!(
                                "[{}/{}] cost: {cost} ({} prompt + {} completion tok)",
                                i + 1,
                                total,
                                u.prompt_tokens,
                                u.completion_tokens
                            );
                        }
                        if r.output.output_type == sema_notebook::format::OutputType::Error {
                            had_error = true;
                        }
                    }
                    Err(e) => {
                        print_cli_error(format!("[{}/{}] {e}", i + 1, total));
                        had_error = true;
                    }
                }
            }

            let session_cost = sema_llm::builtins::session_cost_snapshot();
            if session_cost > 0.0 {
                println!("session cost: ${session_cost:.4}");
            }

            // Save updated outputs back to the file
            if let Err(e) = engine.notebook.save(path) {
                print_cli_warning(format!("could not save: {e}"));
            }

            if had_error {
                std::process::exit(1);
            }
        }
        NotebookCommands::Export {
            file,
            format,
            output,
        } => {
            let path = std::path::Path::new(&file);
            let notebook = match sema_notebook::Notebook::load(path) {
                Ok(nb) => nb,
                Err(e) => {
                    die(e);
                }
            };

            let content = match format.as_str() {
                "md" | "markdown" => sema_notebook::render::export_markdown(&notebook),
                other => {
                    die(format!(
                        "unknown export format: {other}; supported format: md"
                    ));
                }
            };

            match output {
                Some(out_path) => {
                    if let Err(e) = std::fs::write(&out_path, &content) {
                        die(format!("could not write {out_path}: {e}"));
                    }
                    eprintln!("Exported to {out_path}");
                }
                None => print!("{content}"),
            }
        }
        NotebookCommands::New { file, title } => {
            let path = std::path::Path::new(&file);
            let title = title.as_deref().unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled")
            });
            let mut notebook = sema_notebook::Notebook::new(title);
            // Add a starter code cell
            notebook.add_code_cell("; Welcome to your Sema notebook!\n(+ 1 2)");
            if let Err(e) = notebook.save(path) {
                die(e);
            }
            eprintln!("Created notebook: {file}");
        }
    }
}
