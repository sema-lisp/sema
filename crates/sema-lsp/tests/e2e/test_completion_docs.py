"""Test that completion items include documentation."""

import pytest
from lsprotocol.types import (
    CompletionItemKind,
    CompletionList,
    CompletionParams,
    Position,
    TextDocumentIdentifier,
)
from pytest_lsp import LanguageClient

from helpers import open_doc


@pytest.mark.asyncio
async def test_completion_builtin_has_documentation(client: LanguageClient):
    """Completion items for builtins should include documentation."""
    uri = await open_doc(client, "(ma")
    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=3),  # after 'ma'
        )
    )
    assert result is not None
    items = result.items if isinstance(result, CompletionList) else result
    map_items = [i for i in items if i.label == "map"]
    assert len(map_items) >= 1
    item = map_items[0]
    assert item.documentation is not None


@pytest.mark.asyncio
async def test_completion_resolve_uses_redefined_builtin_documentation(client: LanguageClient):
    uri = await open_doc(client, '(defun map (item) "Custom map documentation." item)\n(map 1)')
    result = await client.text_document_completion_async(
        CompletionParams(text_document=TextDocumentIdentifier(uri=uri),
                         position=Position(line=1, character=3))
    )
    items = result.items if isinstance(result, CompletionList) else result
    matching = [item for item in items if item.label == "map"]
    assert len(matching) == 1
    assert matching[0].kind == CompletionItemKind.Function
    resolved = await client.completion_item_resolve_async(matching[0])
    assert "(map item)" in resolved.documentation.value
    assert "Custom map documentation." in resolved.documentation.value


@pytest.mark.asyncio
async def test_completion_resolve_does_not_document_a_local_as_a_builtin(client: LanguageClient):
    uri = await open_doc(client, '(let ((map list))\n  (map 1))')
    result = await client.text_document_completion_async(
        CompletionParams(text_document=TextDocumentIdentifier(uri=uri),
                         position=Position(line=1, character=5))
    )
    items = result.items if isinstance(result, CompletionList) else result
    matching = [item for item in items if item.label == "map"]
    assert len(matching) == 1
    assert matching[0].kind == CompletionItemKind.Variable
    resolved = await client.completion_item_resolve_async(matching[0])
    assert resolved.documentation is None


@pytest.mark.asyncio
async def test_completion_special_form_has_documentation(client: LanguageClient):
    """Completion items for special forms should include documentation."""
    uri = await open_doc(client, "(def")
    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=4),
        )
    )
    assert result is not None
    items = result.items if isinstance(result, CompletionList) else result
    define_items = [i for i in items if i.label == "define"]
    assert len(define_items) >= 1
    item = define_items[0]
    assert item.documentation is not None
