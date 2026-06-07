package com.sema.intellij

import com.intellij.lexer.LexerBase
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType

class SemaLexer : LexerBase() {
    @Suppress("SpellCheckingInspection")
    private val SPECIAL_FORMS = setOf(
        "and", "begin", "case", "cond", "define", "define-record-type",
        "defmacro", "defmethod", "defmulti", "defun", "defn", "def",
        "catch", "delay", "do", "eval", "fn", "force", "if",
        "lambda", "let", "let*", "letrec", "macroexpand", "match",
        "or", "quasiquote", "quote", "set!", "throw", "try",
        "unless", "when", "while", "progn",
        "export", "import", "load", "module",
        "defagent", "deftool", "message", "prompt"
    )

    @Suppress("SpellCheckingInspection")
    private val DEFINITION_FORMS = setOf(
        "define", "defun", "defn", "def", "defmacro", "defmethod",
        "defmulti", "defagent", "deftool", "define-record-type"
    )

    private var buffer: CharSequence = ""
    private var startOffset = 0
    private var endOffset = 0
    private var pos = 0
    private var tokenStart = 0
    private var tokenEnd = 0
    private var tokenType: IElementType? = null

    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        this.startOffset = startOffset
        this.endOffset = endOffset
        this.pos = startOffset
        advance()
    }

    override fun getState(): Int = 0
    override fun getTokenType(): IElementType? = tokenType
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = endOffset

    override fun advance() {
        if (pos >= endOffset) {
            tokenType = null
            return
        }
        tokenStart = pos
        val ch = buffer[pos]

        tokenType = when {
            ch.isWhitespace() || (ch == ',' && peekNext() != '@') -> {
                while (pos < endOffset && (buffer[pos].isWhitespace() || (buffer[pos] == ',' && (pos + 1 >= endOffset || buffer[pos + 1] != '@')))) pos++
                TokenType.WHITE_SPACE
            }

            ch == ';' -> {
                while (pos < endOffset && buffer[pos] != '\n') pos++
                SemaTokenTypes.LINE_COMMENT
            }

            ch == '#' && peekNext() == '|' -> lexBlockComment()
            ch == '"' -> lexString()
            ch == 'f' && peekNext() == '"' -> {
                pos++; lexString()
            }

            ch == '#' && peekNext() == '"' -> {
                pos++; lexString()
            }

            ch == '(' -> {
                pos++; SemaTokenTypes.LPAREN
            }

            ch == ')' -> {
                pos++; SemaTokenTypes.RPAREN
            }

            ch == '[' -> {
                pos++; SemaTokenTypes.LBRACKET
            }

            ch == ']' -> {
                pos++; SemaTokenTypes.RBRACKET
            }

            ch == '{' -> {
                pos++; SemaTokenTypes.LBRACE
            }

            ch == '}' -> {
                pos++; SemaTokenTypes.RBRACE
            }

            ch == '\'' -> {
                pos++; SemaTokenTypes.QUOTE
            }

            ch == '`' -> {
                pos++; SemaTokenTypes.QUASIQUOTE
            }

            ch == ',' && peekNext() == '@' -> {
                pos += 2; SemaTokenTypes.SPLICE
            }

            ch == ',' -> {
                pos++; SemaTokenTypes.UNQUOTE
            }

            ch == '#' && peekNext() == '\\' -> lexCharacter()
            ch == '#' && (peekNext() == 't' || peekNext() == 'f') -> lexHashBoolean()
            ch == '#' && peekNext() == 'u' -> lexHashDispatch()
            ch == ':' && pos + 1 < endOffset && isSymbolChar(buffer[pos + 1]) -> lexKeyword()
            ch == '.' && (pos + 1 >= endOffset || !isSymbolChar(buffer[pos + 1])) -> {
                pos++; SemaTokenTypes.DOT
            }

            isDigit(ch) -> lexNumber()
            ch == '-' && pos + 1 < endOffset && isDigit(buffer[pos + 1]) -> lexNumber()
            isSymbolStart(ch) -> lexSymbol()
            else -> {
                pos++; TokenType.BAD_CHARACTER
            }
        }
        tokenEnd = pos
    }

    private fun peekNext(): Char? = if (pos + 1 < endOffset) buffer[pos + 1] else null

    private fun lexBlockComment(): IElementType {
        pos += 2 // skip #|
        var depth = 1
        while (pos < endOffset && depth > 0) {
            if (pos + 1 < endOffset && buffer[pos] == '#' && buffer[pos + 1] == '|') {
                depth++; pos += 2
            } else if (pos + 1 < endOffset && buffer[pos] == '|' && buffer[pos + 1] == '#') {
                depth--; pos += 2
            } else {
                pos++
            }
        }
        return SemaTokenTypes.BLOCK_COMMENT
    }

    private fun lexString(): IElementType {
        pos++ // skip opening "
        while (pos < endOffset) {
            when (buffer[pos]) {
                '\\' -> {
                    pos++; if (pos < endOffset) pos++
                } // skip escape
                '"' -> {
                    pos++; return SemaTokenTypes.STRING
                }

                else -> pos++
            }
        }
        return SemaTokenTypes.STRING // unterminated
    }

    private fun lexCharacter(): IElementType {
        pos += 2 // skip #\
        if (pos < endOffset) {
            for (name in arrayOf("space", "newline", "tab", "return", "nul")) {
                if (pos + name.length <= endOffset &&
                    buffer.subSequence(pos, pos + name.length).toString() == name &&
                    (pos + name.length >= endOffset || !isSymbolChar(buffer[pos + name.length]))
                ) {
                    pos += name.length
                    return SemaTokenTypes.CHARACTER
                }
            }
            pos++ // single char
        }
        return SemaTokenTypes.CHARACTER
    }

    private fun lexHashBoolean(): IElementType {
        pos += 2 // skip #t or #f
        return SemaTokenTypes.BOOLEAN
    }

    private fun lexHashDispatch(): IElementType {
        pos++ // skip #
        while (pos < endOffset && (buffer[pos].isLetterOrDigit() || buffer[pos] == '(')) {
            val ch = buffer[pos]
            pos++
            if (ch == '(') break
        }
        return SemaTokenTypes.HASH_DISPATCH
    }

    private fun lexKeyword(): IElementType {
        pos++ // skip :
        while (pos < endOffset && isSymbolChar(buffer[pos])) pos++
        return SemaTokenTypes.KEYWORD
    }

    private fun lexNumber(): IElementType {
        if (pos < endOffset && buffer[pos] == '-') pos++
        while (pos < endOffset && isDigit(buffer[pos])) pos++
        if (pos < endOffset && buffer[pos] == '.' && pos + 1 < endOffset && isDigit(buffer[pos + 1])) {
            pos++ // skip .
            while (pos < endOffset && isDigit(buffer[pos])) pos++
        }
        return SemaTokenTypes.NUMBER
    }

    private fun lexSymbol(): IElementType {
        while (pos < endOffset && isSymbolChar(buffer[pos])) pos++
        val text = buffer.subSequence(tokenStart, pos).toString()
        return when {
            text == "true" || text == "false" -> SemaTokenTypes.BOOLEAN
            text == "nil" -> SemaTokenTypes.NIL
            text in DEFINITION_FORMS -> SemaTokenTypes.DEFINITION_KEYWORD
            text in SPECIAL_FORMS -> SemaTokenTypes.SPECIAL_FORM
            else -> SemaTokenTypes.SYMBOL
        }
    }

    private fun isDigit(ch: Char) = ch in '0'..'9'
    private fun isSymbolStart(ch: Char) = ch.isLetter() || ch in "+-*/<>=_!?&%^~."
    private fun isSymbolChar(ch: Char) = isSymbolStart(ch) || isDigit(ch)
}
