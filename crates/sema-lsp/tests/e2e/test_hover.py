"""Test textDocument/hover."""

import pytest
from lsprotocol.types import (
    HoverParams,
    Position,
    TextDocumentIdentifier,
)
from pytest_lsp import LanguageClient

from helpers import open_doc


@pytest.mark.asyncio
async def test_hover_builtin(client: LanguageClient):
    """Hovering over a builtin like 'map' should return documentation."""
    uri = await open_doc(client, "(map inc (list 1 2 3))")
    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=1),  # on 'map'
        )
    )
    assert result is not None
    assert result.contents is not None
    content = result.contents.value if hasattr(result.contents, "value") else str(result.contents)
    assert len(content) > 0


@pytest.mark.asyncio
async def test_hover_user_defined(client: LanguageClient):
    """Hovering over a user-defined function should show its signature."""
    uri = await open_doc(client, "(defun add (a b) (+ a b))\n(add 1 2)")
    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=1, character=1),  # on 'add'
        )
    )
    assert result is not None
    content = result.contents.value if hasattr(result.contents, "value") else str(result.contents)
    assert "add" in content
    assert "User-defined" in content


@pytest.mark.asyncio
async def test_hover_user_redefinition_shadows_builtin(client: LanguageClient):
    """Redefining a builtin name should hover the user's definition, not the builtin doc.

    Regression test for L1: hover checked builtin docs before user definitions,
    so a redefined `map` showed the builtin's doc instead of the local one.
    """
    uri = await open_doc(client, "(defun map (f xs) xs)\n(map inc (list 1 2 3))")
    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=1, character=1),  # on the redefined 'map'
        )
    )
    assert result is not None
    content = result.contents.value if hasattr(result.contents, "value") else str(result.contents)
    # The user's signature `(map f xs)`, flagged user-defined — not the builtin map doc.
    assert "User-defined" in content
    assert "f xs" in content


@pytest.mark.asyncio
async def test_hover_special_form(client: LanguageClient):
    """Hovering over a special form like 'if' should identify it."""
    uri = await open_doc(client, "(if #t 1 0)")
    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=1),  # on 'if'
        )
    )
    assert result is not None
    content = result.contents.value if hasattr(result.contents, "value") else str(result.contents)
    assert "if" in content


@pytest.mark.asyncio
async def test_hover_no_symbol(client: LanguageClient):
    """Hovering over whitespace should return None."""
    uri = await open_doc(client, "  (+ 1 2)")
    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=0),  # whitespace
        )
    )
    assert result is None
