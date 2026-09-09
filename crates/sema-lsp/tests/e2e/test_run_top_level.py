"""Test `workspace/executeCommand sema.runTopLevel` and the `sema/evalResult` notification.

`test_code_lens.py` covers the lens *advertisement* — that each top-level form
gets a `sema.runTopLevel` command carrying `{uri, formIndex}`. It never invokes
the command, so nothing there notices if the eval subprocess path or the
notification payload breaks. This file covers the other half: actually running
a form and asserting on what comes back over `sema/evalResult`.

The contract under test is `EvalResultParams` in
`crates/sema-lsp/src/handlers/command.rs`, documented in `website/docs/lsp.md`:
`{uri, range, kind, value, stdout, stderr, ok, error, elapsedMs}`.

Two things make this an integration test rather than a unit test, and both are
the point:

1. The server shells out to its own `current_exe()` with
   `eval --stdin --json`, so a regression in the
   `sema eval --json` envelope (a renamed field, a changed error shape) breaks
   the lens even though nothing in `sema-lsp` changed.
2. `executeCommand` returns immediately — the work happens on the backend
   thread and the result arrives later as a *notification*. Tests must await
   the notification, never the command's own reply.
"""

import asyncio
import json

import pytest
from lsprotocol.types import ExecuteCommandParams

from pytest_lsp import LanguageClient

from helpers import open_doc

# The eval is a real subprocess spawn (cold binary, interpreter setup), so allow
# generously more than it needs. A wrong payload fails fast on the assert; only
# a genuinely missing notification burns the full budget.
EVAL_TIMEOUT = 30.0


@pytest.fixture
def eval_results(client: LanguageClient) -> asyncio.Queue:
    """Capture every `sema/evalResult` notification the server pushes.

    Registered once per test: pygls keys handlers by method name, so
    re-registering inside a helper that runs twice would silently replace the
    first handler (or raise, depending on version). One queue per test keeps
    multi-run tests honest about ordering.
    """
    queue: asyncio.Queue = asyncio.Queue()

    @client.feature("sema/evalResult")
    def _on_eval_result(params):  # pragma: no cover - driven by the server
        queue.put_nowait(params)

    return queue


async def run_top_level(client: LanguageClient, uri: str, form_index: int):
    """Fire the Run lens for one form. Does not wait for the result."""
    await client.workspace_execute_command_async(
        ExecuteCommandParams(
            command="sema.runTopLevel",
            arguments=[{"uri": uri, "formIndex": form_index}],
        )
    )


async def next_result(queue: asyncio.Queue, timeout: float = EVAL_TIMEOUT):
    return await asyncio.wait_for(queue.get(), timeout=timeout)


def field(params, name: str):
    """Read a payload field.

    pygls hands custom-notification params back as an attrs object when it can
    infer a type and as a plain object/dict otherwise, and it converts
    `elapsedMs` to `elapsed_ms` in the former case. Normalize both so the
    assertions read the same either way.
    """
    if isinstance(params, dict):
        return params.get(name)
    snake = "".join("_" + c.lower() if c.isupper() else c for c in name)
    for attr in (name, snake):
        if hasattr(params, attr):
            return getattr(params, attr)
    return None


@pytest.mark.asyncio
async def test_run_top_level_reports_value(client: LanguageClient, eval_results):
    """A successful run reports ok, the printed value, and an elapsed time."""
    uri = await open_doc(client, "(+ 1 2)")

    await run_top_level(client, uri, 0)
    result = await next_result(eval_results)

    assert field(result, "ok") is True, f"expected a successful eval, got {result}"
    assert field(result, "value") == "3"
    assert field(result, "kind") == "run"
    assert field(result, "error") is None
    # Not a timing assertion — just that the field is populated and plausible.
    # A missing/None elapsedMs is the regression this catches.
    elapsed = field(result, "elapsedMs")
    assert isinstance(elapsed, int) and elapsed >= 0


@pytest.mark.asyncio
async def test_run_top_level_targets_the_requested_form(
    client: LanguageClient, eval_results
):
    """The reported range is the form that was run, not the whole document."""
    uri = await open_doc(client, "(define x 1)\n(+ x 41)")

    await run_top_level(client, uri, 1)
    result = await next_result(eval_results)

    assert field(result, "ok") is True, f"expected a successful eval, got {result}"
    rng = field(result, "range")
    start_line = rng["start"]["line"] if isinstance(rng, dict) else rng.start.line
    assert start_line == 1, "range must point at form 1, not form 0"
    assert str(field(result, "uri")).endswith("test.sema")


@pytest.mark.asyncio
async def test_run_top_level_includes_preceding_forms(
    client: LanguageClient, eval_results
):
    """Running form N evaluates forms 0..=N, so earlier definitions are in scope.

    This is the behavior that makes the lens useful at all — running `(+ x 41)`
    alone would be an unbound-symbol error. It is easy to "optimize" into
    sending only the selected form, which is why it gets its own test.
    """
    uri = await open_doc(client, "(define x 1)\n(+ x 41)")

    await run_top_level(client, uri, 1)
    result = await next_result(eval_results)

    assert field(result, "ok") is True, f"expected x to be in scope, got {result}"
    assert field(result, "value") == "42"


@pytest.mark.asyncio
async def test_run_top_level_captures_stdout(client: LanguageClient, eval_results):
    """Output printed by the form is captured, not swallowed or merged into value."""
    uri = await open_doc(client, '(println "hello from the lens")')

    await run_top_level(client, uri, 0)
    result = await next_result(eval_results)

    assert field(result, "ok") is True, f"expected a successful eval, got {result}"
    assert "hello from the lens" in field(result, "stdout")


@pytest.mark.asyncio
async def test_run_top_level_allows_file_writes(
    client: LanguageClient, eval_results, tmp_path
):
    """Run permits file writes, matching the normal CLI defaults."""
    output = tmp_path / "lens-output.txt"
    uri = await open_doc(
        client, f'(file/write {json.dumps(str(output))} "written by the lens")'
    )

    await run_top_level(client, uri, 0)
    result = await next_result(eval_results)

    assert field(result, "ok") is True, f"expected file/write to succeed, got {result}"
    assert output.read_text() == "written by the lens"


@pytest.mark.asyncio
async def test_run_top_level_auto_configures_llm(client: LanguageClient, eval_results):
    """Run configures providers without requiring API keys or making LLM requests."""
    # Auto-configuration always registers Ollama, even when it is not running.
    uri = await open_doc(client, "(list/contains? (llm/providers) :ollama)")

    await run_top_level(client, uri, 0)
    result = await next_result(eval_results)

    assert field(result, "ok") is True, f"expected llm/providers to succeed, got {result}"
    assert field(result, "value") == "#t"


@pytest.mark.asyncio
async def test_run_top_level_reports_eval_error(client: LanguageClient, eval_results):
    """A failing form reports ok=false with a message, rather than no notification."""
    uri = await open_doc(client, "(this-symbol-does-not-exist)")

    await run_top_level(client, uri, 0)
    result = await next_result(eval_results)

    assert field(result, "ok") is False, f"expected a failed eval, got {result}"
    error = field(result, "error")
    assert error, "a failed eval must carry an error message"
    assert "this-symbol-does-not-exist" in error


@pytest.mark.asyncio
async def test_run_top_level_ignores_out_of_range_form(
    client: LanguageClient, eval_results
):
    """An out-of-range formIndex is dropped silently — no notification, no crash.

    Pinned because the alternative (panicking the backend thread on a stale
    lens from an edited document) would take the whole server down.
    """
    uri = await open_doc(client, "(+ 1 2)")

    await run_top_level(client, uri, 99)

    with pytest.raises(asyncio.TimeoutError):
        await next_result(eval_results, timeout=3.0)

    # The server is still alive and still serving the same document.
    await run_top_level(client, uri, 0)
    result = await next_result(eval_results)
    assert field(result, "ok") is True


@pytest.mark.asyncio
async def test_unknown_command_is_ignored(client: LanguageClient, eval_results):
    """An unrecognized command produces no evalResult and does not kill the server."""
    await client.workspace_execute_command_async(
        ExecuteCommandParams(command="sema.notARealCommand", arguments=[{}])
    )

    with pytest.raises(asyncio.TimeoutError):
        await next_result(eval_results, timeout=3.0)

    uri = await open_doc(client, "(+ 20 22)")
    await run_top_level(client, uri, 0)
    result = await next_result(eval_results)
    assert field(result, "value") == "42"
